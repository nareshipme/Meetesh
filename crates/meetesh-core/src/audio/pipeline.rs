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

use crate::audio::Resampler;

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

/// Handed to the capture backend. Owns the resampler for the device's native format.
pub struct Pusher {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    resampler: Resampler,
    scratch: Vec<f32>,
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
        }
    }
}

impl Pusher {
    /// Call from the audio callback. Never blocks.
    pub fn push_interleaved(&mut self, input: &[f32]) {
        self.scratch.clear();
        self.resampler.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        let frame = std::mem::take(&mut self.scratch);
        match self.tx.try_send(Msg::Frame(frame)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
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
}
