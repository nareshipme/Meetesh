//! Live transcription with a real model, fed at real-time pace, measuring how far behind the
//! speech the text arrives (NFR-05: under 10 s). Opt-in like the other whisper test:
//!
//!   MEETESH_TEST_MODEL=/path/to/ggml-tiny.en.bin cargo test -p meetesh-core --features whisper --test whisper_live -- --nocapture
#![cfg(feature = "whisper")]

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meetesh_core::asr::live::{live_chunker_config, LiveTranscriber};
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::asr::{TranscriptSegment, Transcriber};
use meetesh_core::audio::wav::read_segment;
use meetesh_core::audio::RecordingSession;

#[test]
fn live_text_arrives_within_ten_seconds_of_speech() {
    let Some(model) = std::env::var_os("MEETESH_TEST_MODEL").map(PathBuf::from) else {
        eprintln!("MEETESH_TEST_MODEL not set; skipping");
        return;
    };
    let speech = read_segment(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech.wav")).unwrap();

    let start = Instant::now();
    let arrivals: Arc<Mutex<Vec<(f64, TranscriptSegment)>>> = Arc::new(Mutex::new(vec![]));
    let a = arrivals.clone();
    let (done_tx, done_rx) = channel();
    let live = LiveTranscriber::start(
        live_chunker_config(),
        8,
        Box::new(move || Ok(Arc::new(WhisperTranscriber::load("test", &model, Some("en"))?) as Arc<dyn Transcriber>)),
        move |segs| {
            let t = start.elapsed().as_secs_f64();
            a.lock().unwrap().extend(segs.into_iter().map(|s| (t, s)));
        },
        move |r| {
            let _ = done_tx.send(r);
        },
    );

    let session = RecordingSession::start(vec![Box::new(live)], 512);
    let mut pusher = session.pusher(16_000, 1);

    // 1 s silence, the speech, 1.5 s silence, paced like a microphone (10 ms per callback).
    let mut audio = vec![0.0f32; 16_000];
    audio.extend(&speech);
    audio.extend(vec![0.0f32; 24_000]);
    let t0 = Instant::now();
    for (i, c) in audio.chunks(160).enumerate() {
        pusher.push_interleaved(c);
        let due = Duration::from_millis(10 * (i as u64 + 1));
        if let Some(wait) = due.checked_sub(t0.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    let report = session.stop();
    let live = done_rx.recv_timeout(Duration::from_secs(30)).expect("live worker finished");

    let arrivals = arrivals.lock().unwrap();
    let text = arrivals.iter().map(|(_, s)| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    eprintln!("live transcript: {text:?}");
    let mut worst = 0f64;
    for (arrived, seg) in arrivals.iter() {
        let lag = arrived - seg.end_ms as f64 / 1000.0;
        worst = worst.max(lag);
        eprintln!("  [{:>5.1}s-{:>5.1}s] arrived at {:>5.1}s (lag {:>4.1}s): {}", seg.start_ms as f64 / 1e3, seg.end_ms as f64 / 1e3, arrived, lag, seg.text);
    }
    eprintln!("worst lag {worst:.1}s; dropped frames {}, dropped chunks {}", report.dropped_frames, live.dropped_chunks);

    for needle in ["alice", "report", "friday"] {
        assert!(text.contains(needle), "missing {needle:?} in {text:?}");
    }
    assert!(worst < 10.0, "live text lagged {worst:.1}s behind speech");
    assert_eq!(report.dropped_frames, 0);
    assert!(!live.needs_full_pass(), "{live:?}");
}
