//! Live transcription as a pipeline sink.
//!
//! The VAD chunker runs on the audio pipeline thread (it is cheap). Finished speech chunks go
//! through a bounded queue to a worker thread that runs the (slow) transcriber, so inference can
//! never block capture (NFR-06). If transcription falls behind, chunks are dropped from the live
//! view and counted; the audio on disk is untouched, so a full pass can fill the gaps afterwards.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;

use crate::asr::chunker::{Chunk, Chunker, ChunkerConfig};
use crate::asr::run::transcribe_chunk;
use crate::asr::{TranscriptSegment, Transcriber};
use crate::audio::pipeline::FrameSink;
use crate::Result;

#[derive(Debug, Clone, Default)]
pub struct LiveReport {
    /// Chunks the worker processed.
    pub chunks: u64,
    /// Chunks skipped because the transcriber was behind (or never loaded).
    pub dropped_chunks: u64,
    pub errors: Vec<String>,
}

impl LiveReport {
    /// True if the live transcript may have gaps, so a full pass over the audio is worthwhile.
    pub fn needs_full_pass(&self) -> bool {
        self.dropped_chunks > 0 || !self.errors.is_empty()
    }
}

/// Live transcription finishes on its own thread, either before or after the recording is
/// stopped. This hands the final report to exactly one caller: whichever event happens second.
#[derive(Default)]
pub struct LiveSettle {
    recording_stopped: bool,
    report: Option<LiveReport>,
}

impl LiveSettle {
    /// The recording has stopped and its metadata is saved. Returns the report if the worker
    /// has already finished.
    pub fn recording_stopped(&mut self) -> Option<LiveReport> {
        self.recording_stopped = true;
        self.take()
    }

    /// The live worker has finished. Returns the report if the recording has already stopped.
    pub fn worker_finished(&mut self, report: LiveReport) -> Option<LiveReport> {
        self.report = Some(report);
        self.take()
    }

    fn take(&mut self) -> Option<LiveReport> {
        if self.recording_stopped {
            self.report.take()
        } else {
            None
        }
    }
}

/// Settings tuned for latency: shorter pauses and a shorter forced cut than the offline default,
/// so text appears within a few seconds even for continuous speech (NFR-05).
pub fn live_chunker_config() -> ChunkerConfig {
    ChunkerConfig { min_silence_ms: 450, max_chunk_ms: 6_000, ..ChunkerConfig::default() }
}

type Loader = Box<dyn FnOnce() -> Result<Arc<dyn Transcriber>> + Send>;

pub struct LiveTranscriber {
    chunker: Chunker,
    tx: Option<SyncSender<Chunk>>,
    dropped: Arc<AtomicU64>,
    /// Chunks queued or being transcribed right now. Other work (hint generation) watches this and
    /// stays out of the way while it is above zero.
    pending: Arc<AtomicUsize>,
}

impl LiveTranscriber {
    /// `loader` runs on the worker thread, so a slow model load never delays the microphone.
    /// `on_segments` is called on the worker thread for every batch of new segments.
    /// `on_done` is called once, after the recording has stopped and the queue is drained.
    pub fn start(
        cfg: ChunkerConfig,
        queue_len: usize,
        loader: Loader,
        on_segments: impl Fn(Vec<TranscriptSegment>) + Send + 'static,
        on_done: impl FnOnce(LiveReport) + Send + 'static,
    ) -> Self {
        Self::start_with_load(cfg, queue_len, loader, on_segments, on_done, Arc::new(AtomicUsize::new(0)))
    }

    /// Like `start`, publishing the number of chunks queued or in progress in `pending`.
    pub fn start_with_load(
        cfg: ChunkerConfig,
        queue_len: usize,
        loader: Loader,
        on_segments: impl Fn(Vec<TranscriptSegment>) + Send + 'static,
        on_done: impl FnOnce(LiveReport) + Send + 'static,
        pending: Arc<AtomicUsize>,
    ) -> Self {
        let pending_worker = pending.clone();
        let (tx, rx) = sync_channel::<Chunk>(queue_len.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_worker = dropped.clone();
        std::thread::Builder::new()
            .name("meetesh-live-asr".into())
            .spawn(move || {
                let mut report = LiveReport::default();
                match loader() {
                    Ok(t) => {
                        for chunk in rx {
                            match transcribe_chunk(t.as_ref(), &chunk) {
                                Ok(segs) if !segs.is_empty() => on_segments(segs),
                                Ok(_) => {}
                                Err(e) => report.errors.push(e.to_string()),
                            }
                            report.chunks += 1;
                            pending_worker.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        report.errors.push(e.to_string());
                        // Keep draining so the producer never sees a dead channel mid-recording.
                        for _ in rx {
                            dropped_worker.fetch_add(1, Ordering::Relaxed);
                            pending_worker.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
                report.dropped_chunks = dropped_worker.load(Ordering::Relaxed);
                on_done(report);
            })
            .expect("spawn live transcription thread");
        LiveTranscriber { chunker: Chunker::new(cfg), tx: Some(tx), dropped, pending }
    }

    fn submit(&mut self, chunk: Chunk) {
        if let Some(tx) = &self.tx {
            self.pending.fetch_add(1, Ordering::Relaxed);
            match tx.try_send(chunk) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    self.pending.fetch_sub(1, Ordering::Relaxed);
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

impl FrameSink for LiveTranscriber {
    fn on_samples(&mut self, samples: &[f32], _start: u64) -> Result<()> {
        for chunk in self.chunker.push(samples) {
            self.submit(chunk);
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if let Some(chunk) = self.chunker.flush() {
            self.submit(chunk);
        }
        // Closing the queue lets the worker drain what is left and report. We do not wait for it,
        // so stopping a recording stays instant even if inference is behind.
        self.tx = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::pipeline::RecordingSession;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;
    use std::time::Duration;

    struct Fake {
        delay: Duration,
    }
    impl Transcriber for Fake {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
            std::thread::sleep(self.delay);
            Ok(vec![TranscriptSegment { start_ms: 0, end_ms: (samples.len() / 16) as i64, text: "words".into() }])
        }
    }

    fn talk(ms: usize) -> Vec<f32> {
        (0..ms * 16).map(|i| (i as f32 * 0.3).sin() * 0.3).collect()
    }
    fn quiet(ms: usize) -> Vec<f32> {
        vec![0.0; ms * 16]
    }

    type Collected = Arc<Mutex<Vec<TranscriptSegment>>>;

    fn live(delay_ms: u64, queue: usize) -> (LiveTranscriber, Collected, std::sync::mpsc::Receiver<LiveReport>) {
        let got: Collected = Arc::new(Mutex::new(vec![]));
        let g = got.clone();
        let (done_tx, done_rx) = channel();
        let l = LiveTranscriber::start(
            live_chunker_config(),
            queue,
            Box::new(move || Ok(Arc::new(Fake { delay: Duration::from_millis(delay_ms) }) as Arc<dyn Transcriber>)),
            move |s| g.lock().unwrap().extend(s),
            move |r| {
                let _ = done_tx.send(r);
            },
        );
        (l, got, done_rx)
    }

    #[test]
    fn text_appears_during_the_recording_not_only_after_it() {
        let (l, got, done_rx) = live(0, 8);
        let session = RecordingSession::start(vec![Box::new(l)], 256);
        let mut p = session.pusher(16_000, 1);
        let mut audio = quiet(300);
        audio.extend(talk(1_000));
        audio.extend(quiet(700)); // pause: chunk completes here
        for c in audio.chunks(320) {
            p.push_interleaved(c);
        }
        // Recording is still running; the first segment must already arrive.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while got.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.lock().unwrap().len(), 1, "segment should arrive before stop()");
        let seg = got.lock().unwrap()[0].clone();
        assert!((seg.start_ms - 100).abs() <= 40, "timestamp offset {}", seg.start_ms); // 300 ms lead-in minus preroll
        session.stop();
        let report = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!report.needs_full_pass());
    }

    #[test]
    fn multiple_utterances_arrive_in_order_with_absolute_timestamps() {
        let (l, got, done_rx) = live(0, 8);
        // The test pushes ~530 frames back to back, so the queue must be larger than that: by design
        // a full queue drops frames rather than block capture, which would make this test depend on
        // how busy the machine is.
        let session = RecordingSession::start(vec![Box::new(l)], 4096);
        let mut p = session.pusher(16_000, 1);
        let mut audio = quiet(200);
        for _ in 0..3 {
            audio.extend(talk(800));
            audio.extend(quiet(900));
        }
        for c in audio.chunks(160) {
            p.push_interleaved(c);
        }
        assert_eq!(session.stop().dropped_frames, 0, "test input must not overflow the queue");
        let report = done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let segs = got.lock().unwrap().clone();
        assert_eq!(segs.len(), 3);
        assert!(segs.windows(2).all(|w| w[0].start_ms + 1_000 < w[1].start_ms + 200));
        assert!(segs[2].start_ms > 3_000, "{}", segs[2].start_ms);
        assert_eq!(report.chunks, 3);
    }

    #[test]
    fn slow_transcriber_never_drops_audio_and_flags_a_full_pass() {
        // 30 utterances, a transcriber that needs 200 ms each, queue of 2: it cannot keep up.
        let (l, got, done_rx) = live(200, 2);
        let session = RecordingSession::start(vec![Box::new(l)], 4096);
        let mut p = session.pusher(16_000, 1);
        let t = std::time::Instant::now();
        for _ in 0..30 {
            let mut u = talk(700);
            u.extend(quiet(600));
            for c in u.chunks(160) {
                p.push_interleaved(c);
            }
        }
        let push_time = t.elapsed();
        let report_pipeline = session.stop();
        assert_eq!(report_pipeline.dropped_frames, 0, "audio capture must not be affected");
        assert!(push_time < Duration::from_secs(3), "capture was blocked: {push_time:?}");
        let live_report = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(live_report.dropped_chunks > 0, "expected the live view to fall behind");
        assert!(live_report.needs_full_pass());
        assert!(!got.lock().unwrap().is_empty());
        assert_eq!(live_report.chunks + live_report.dropped_chunks, 30);
    }

    #[test]
    fn stop_returns_immediately_even_with_a_backlog() {
        let (l, _got, done_rx) = live(300, 8);
        let session = RecordingSession::start(vec![Box::new(l)], 1024);
        let mut p = session.pusher(16_000, 1);
        for _ in 0..6 {
            let mut u = talk(700);
            u.extend(quiet(600));
            for c in u.chunks(160) {
                p.push_interleaved(c);
            }
        }
        let t = std::time::Instant::now();
        session.stop();
        assert!(t.elapsed() < Duration::from_millis(500), "stop() waited for inference: {:?}", t.elapsed());
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap(); // backlog still completes in the background
    }

    #[test]
    fn pending_counter_is_above_zero_while_chunks_are_queued_or_running_and_returns_to_zero() {
        let pending = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = channel();
        let l = LiveTranscriber::start_with_load(
            live_chunker_config(),
            8,
            Box::new(|| Ok(Arc::new(Fake { delay: Duration::from_millis(200) }) as Arc<dyn Transcriber>)),
            |_| {},
            move |r| {
                let _ = done_tx.send(r);
            },
            pending.clone(),
        );
        let session = RecordingSession::start(vec![Box::new(l)], 4096);
        let mut p = session.pusher(16_000, 1);
        for _ in 0..3 {
            let mut u = talk(700);
            u.extend(quiet(600));
            for c in u.chunks(160) {
                p.push_interleaved(c);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(pending.load(Ordering::Relaxed) > 0, "work is queued or running, so others must yield");
        session.stop();
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(pending.load(Ordering::Relaxed), 0, "everything finished, so nothing is pending");
    }

    #[test]
    fn settle_fires_once_in_either_order() {
        let r = || LiveReport { dropped_chunks: 1, ..Default::default() };
        // worker first
        let mut a = LiveSettle::default();
        assert!(a.worker_finished(r()).is_none());
        assert!(a.recording_stopped().unwrap().needs_full_pass());
        assert!(a.recording_stopped().is_none(), "must not fire twice");
        // recording first
        let mut b = LiveSettle::default();
        assert!(b.recording_stopped().is_none());
        assert!(b.worker_finished(r()).is_some());
        assert!(b.recording_stopped().is_none(), "must not fire twice");
    }

    #[test]
    fn model_load_failure_is_reported_and_recording_is_unaffected() {
        let (done_tx, done_rx) = channel();
        let l = LiveTranscriber::start(
            live_chunker_config(),
            4,
            Box::new(|| Err(crate::Error::Other("model missing".into()))),
            |_| {},
            move |r| {
                let _ = done_tx.send(r);
            },
        );
        let session = RecordingSession::start(vec![Box::new(l)], 256);
        let mut p = session.pusher(16_000, 1);
        let mut a = talk(800);
        a.extend(quiet(900));
        for c in a.chunks(160) {
            p.push_interleaved(c);
        }
        let r = session.stop();
        assert_eq!(r.dropped_frames, 0);
        let live = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(live.needs_full_pass() && live.errors[0].contains("model missing"));
    }
}
