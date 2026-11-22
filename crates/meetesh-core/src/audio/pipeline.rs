//! Real-time-safe fan-out of audio frames.
//!
//! The capture callback calls `Pusher::push_interleaved`, which never blocks: if the consumer
//! thread falls behind, the frame is counted as dropped and discarded (NFR-06). A dedicated
//! thread then feeds each `FrameSink` (disk writer, level meter, VAD/transcription, ...), so slow
//! work such as transcription can never stall capture.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crate::audio::{Resampler, TARGET_RATE};

/// Consumer of 16 kHz mono frames. Runs on the pipeline thread.
pub trait FrameSink: Send {
    /// `start_sample` is the index of `samples[0]` since the start of the recording.
    fn on_samples(&mut self, samples: &[f32], start_sample: u64) -> crate::Result<()>;
    /// Called once when recording stops. Flush and finalize here.
    fn finish(&mut self) -> crate::Result<()> {
        Ok(())
    }
}

enum Msg {
    Frame(Vec<f32>),
    Stop,
}

#[derive(Debug, Clone, Default)]
pub struct SessionReport {
    pub samples: u64,
    pub dropped_frames: u64,
    /// Errors raised by sinks. A failing sink is disabled; the others keep running.
    pub errors: Vec<String>,
}

pub struct RecordingSession {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    join: JoinHandle<SessionReport>,
}

/// Some capture APIs (Windows loopback) deliver nothing while nothing is playing. A track made
/// only from what arrives would be shorter than real time, and everything after a quiet stretch
/// would be stamped too early. `GapFiller` says how much silence to insert so the track keeps
/// real time. Pure and testable: the caller supplies the elapsed time.
#[derive(Debug, Clone, Default)]
pub struct GapFiller {
    produced: u64,
}

impl GapFiller {
    /// Gaps shorter than this are ordinary delivery jitter and are left alone.
    const TOLERANCE_SAMPLES: u64 = TARGET_RATE as u64 / 10;

    /// `elapsed_ms`: time since the track started, now. `incoming`: 16 kHz samples that just
    /// arrived (they ended about now). Returns how many silent samples to put before them.
    pub fn gap_before(&mut self, elapsed_ms: u64, incoming: usize) -> usize {
        let expected_start = (elapsed_ms * TARGET_RATE as u64 / 1000).saturating_sub(incoming as u64);
        let gap = expected_start.saturating_sub(self.produced);
        let gap = if gap > Self::TOLERANCE_SAMPLES { gap } else { 0 };
        self.produced += gap + incoming as u64;
        gap as usize
    }
}

/// Handed to the capture backend. Owns the resampler for the device's native format.
pub struct Pusher {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    resampler: Resampler,
    scratch: Vec<f32>,
    gaps: Option<(Instant, GapFiller)>,
}

/// Cloneable, 'static source of `Pusher`s, so a capture thread can create its own once it
/// knows the device's native sample rate and channel count.
#[derive(Clone)]
pub struct PusherFactory {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
}

impl PusherFactory {
    pub fn make(&self, sample_rate: u32, channels: u16) -> Pusher {
        Pusher {
            tx: self.tx.clone(),
            dropped: self.dropped.clone(),
            resampler: Resampler::new(sample_rate, channels),
            scratch: Vec::new(),
            gaps: None,
        }
    }
}

impl Pusher {
    /// Keep this track in real time: when the source delivers nothing for a while, insert
    /// silence. `origin` is when the track's clock started (the moment capture began).
    pub fn fill_gaps_from(mut self, origin: Instant) -> Self {
        self.gaps = Some((origin, GapFiller::default()));
        self
    }

    fn send(&self, frame: Vec<f32>) {
        match self.tx.try_send(Msg::Frame(frame)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Call from the audio callback. Never blocks.
    pub fn push_interleaved(&mut self, input: &[f32]) {
        self.scratch.clear();
        self.resampler.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        if let Some((origin, filler)) = self.gaps.as_mut() {
            let mut gap = filler.gap_before(origin.elapsed().as_millis() as u64, self.scratch.len());
            while gap > 0 {
                let n = gap.min(TARGET_RATE as usize); // at most one second per frame
                self.send(vec![0.0; n]);
                gap -= n;
            }
        }
        let frame = std::mem::take(&mut self.scratch);
        self.send(frame);
    }
}

impl RecordingSession {
    /// `queue_len` frames (typically 10 to 20 ms each) of slack before frames are dropped.
    pub fn start(mut sinks: Vec<Box<dyn FrameSink>>, queue_len: usize) -> Self {
        let (tx, rx): (SyncSender<Msg>, Receiver<Msg>) = sync_channel(queue_len.max(8));
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_thread = dropped.clone();
        let join = std::thread::Builder::new()
            .name("meetesh-audio-pipeline".into())
            .spawn(move || {
                let mut report = SessionReport::default();
                let mut alive = vec![true; sinks.len()];
                let mut pos = 0u64;
                while let Ok(Msg::Frame(frame)) = rx.recv() {
                    for (i, sink) in sinks.iter_mut().enumerate() {
                        if !alive[i] {
                            continue;
                        }
                        if let Err(e) = sink.on_samples(&frame, pos) {
                            report.errors.push(e.to_string());
                            alive[i] = false;
                        }
                    }
                    pos += frame.len() as u64;
                }
                for sink in sinks.iter_mut() {
                    if let Err(e) = sink.finish() {
                        report.errors.push(e.to_string());
                    }
                }
                report.samples = pos;
                report.dropped_frames = dropped_thread.load(Ordering::Relaxed);
                report
            })
            .expect("spawn audio pipeline thread");
        RecordingSession { tx, dropped, join }
    }

    pub fn pusher_factory(&self) -> PusherFactory {
        PusherFactory { tx: self.tx.clone(), dropped: self.dropped.clone() }
    }

    pub fn pusher(&self, sample_rate: u32, channels: u16) -> Pusher {
        self.pusher_factory().make(sample_rate, channels)
    }

    /// Stop capturing first, then call this: it drains queued frames and finalizes the sinks.
    pub fn stop(self) -> SessionReport {
        let _ = self.tx.send(Msg::Stop); // blocking send is fine off the audio thread
        self.join.join().unwrap_or_else(|_| SessionReport {
            errors: vec!["audio pipeline thread panicked".into()],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Collect(Arc<Mutex<Vec<f32>>>);
    impl FrameSink for Collect {
        fn on_samples(&mut self, s: &[f32], _pos: u64) -> crate::Result<()> {
            self.0.lock().unwrap().extend_from_slice(s);
            Ok(())
        }
    }

    struct Failing;
    impl FrameSink for Failing {
        fn on_samples(&mut self, _s: &[f32], _pos: u64) -> crate::Result<()> {
            Err(crate::Error::Other("boom".into()))
        }
    }

    struct Slow;
    impl FrameSink for Slow {
        fn on_samples(&mut self, _s: &[f32], _pos: u64) -> crate::Result<()> {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok(())
        }
    }

    #[test]
    fn delivers_all_frames_in_order() {
        let got = Arc::new(Mutex::new(vec![]));
        let session = RecordingSession::start(vec![Box::new(Collect(got.clone()))], 64);
        let mut p = session.pusher(16_000, 1);
        let input: Vec<f32> = (0..1600).map(|i| i as f32).collect();
        for c in input.chunks(160) {
            p.push_interleaved(c);
        }
        let report = session.stop();
        assert_eq!(report.samples, 1600);
        assert_eq!(report.dropped_frames, 0);
        assert_eq!(*got.lock().unwrap(), input);
    }

    #[test]
    fn failing_sink_is_isolated() {
        let got = Arc::new(Mutex::new(vec![]));
        let session = RecordingSession::start(vec![Box::new(Failing), Box::new(Collect(got.clone()))], 64);
        let mut p = session.pusher(16_000, 1);
        for _ in 0..5 {
            p.push_interleaved(&[0.1; 160]);
        }
        let report = session.stop();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(got.lock().unwrap().len(), 800);
    }

    #[test]
    fn slow_sink_never_blocks_the_audio_callback() {
        let session = RecordingSession::start(vec![Box::new(Slow)], 8);
        let mut p = session.pusher(16_000, 1);
        let t = std::time::Instant::now();
        for _ in 0..200 {
            p.push_interleaved(&[0.1; 160]);
        }
        // 200 pushes against a 50 ms/frame consumer would take 10 s if push blocked.
        assert!(t.elapsed() < std::time::Duration::from_millis(500), "push blocked: {:?}", t.elapsed());
        let report = session.stop();
        assert!(report.dropped_frames > 0, "expected drops to be counted");
    }

    #[test]
    fn gap_filler_leaves_steady_audio_alone_and_fills_silence_gaps() {
        let mut g = GapFiller::default();
        // 10 ms blocks arriving on time: never a gap.
        for i in 1..=50u64 {
            assert_eq!(g.gap_before(i * 10, 160), 0, "block {i}");
        }
        // Nothing arrives for 2 s, then a 10 ms block: 2 s of silence belong before it.
        let gap = g.gap_before(500 + 2_000 + 10, 160);
        assert!((31_000..=32_100).contains(&gap), "gap {gap}");
        // And we are back in step afterwards.
        assert_eq!(g.gap_before(2_520 + 10, 160), 0);
    }

    #[test]
    fn gap_filler_ignores_jitter_and_a_late_start_is_silence_too() {
        let mut g = GapFiller::default();
        assert_eq!(g.gap_before(60, 160), 0, "50 ms late is jitter");
        let mut g = GapFiller::default();
        // The first audio arrives after 3 s of nothing: the track starts with that silence.
        assert!(g.gap_before(3_000, 1_600) > 40_000);
    }

    #[test]
    fn a_pusher_with_gap_filling_keeps_real_time() {
        let got = Arc::new(Mutex::new(vec![]));
        let session = RecordingSession::start(vec![Box::new(Collect(got.clone()))], 256);
        let mut p = session.pusher(16_000, 1).fill_gaps_from(Instant::now() - std::time::Duration::from_millis(1_500));
        // Audio first arrives 1.5 s after the track's clock started.
        p.push_interleaved(&[0.5; 1_600]);
        let report = session.stop();
        let n = report.samples as i64;
        assert!((n - 24_000).abs() < 2_000, "about 1.5 s in total, got {n}");
        assert!(got.lock().unwrap()[..16_000].iter().all(|s| *s == 0.0), "the quiet start is silence");
    }
}
