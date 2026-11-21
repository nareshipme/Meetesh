//! Real transcription of a spoken fixture. Needs a whisper model, so it is opt-in:
//!
//!   MEETESH_TEST_MODEL=/path/to/ggml-tiny.en.bin cargo test -p meetesh-core --features whisper --test whisper_fixture
//!
//! Without MEETESH_TEST_MODEL the test is skipped (CI sets it after downloading a model).
#![cfg(feature = "whisper")]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use meetesh_core::asr::run::transcribe_recording;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::audio::wav::{read_segment, SegmentWriter};

#[test]
fn transcribes_spoken_fixture_end_to_end() {
    let Some(model) = std::env::var_os("MEETESH_TEST_MODEL").map(PathBuf::from) else {
        eprintln!("MEETESH_TEST_MODEL not set; skipping");
        return;
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech.wav");
    let samples = {
        // fixture is a plain 16 kHz mono WAV: reuse the crash-tolerant reader
        read_segment(&fixture).unwrap()
    };

    // Store it like a real recording (5 s segments), with silence around it.
    let dir = std::env::temp_dir().join(format!("meetesh-whisper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = SegmentWriter::new(&dir).unwrap();
    w.push(&vec![0.0; 16_000]).unwrap();
    w.push(&samples).unwrap();
    w.push(&vec![0.0; 16_000]).unwrap();
    w.finish().unwrap();

    let t = WhisperTranscriber::load("test-model", &model, Some("en")).unwrap();
    let t0 = std::time::Instant::now();
    let segs = transcribe_recording(&dir, &t, &AtomicBool::new(false), |_| {}, |_| {}).unwrap();
    let elapsed = t0.elapsed();

    let text = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    eprintln!("transcript: {text:?}  ({} segments, {:?})", segs.len(), elapsed);
    for needle in ["alice", "report", "friday", "budget"] {
        assert!(text.contains(needle), "missing {needle:?} in {text:?}");
    }
    assert!(segs.windows(2).all(|p| p[0].start_ms <= p[1].start_ms));
    assert!(segs[0].start_ms >= 800, "timestamps include the 1 s lead-in: {}", segs[0].start_ms);
    std::fs::remove_dir_all(dir).ok();
}
