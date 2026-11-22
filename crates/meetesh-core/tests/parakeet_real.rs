//! Parakeet on real model files. Opt-in: needs the model folder and an ONNX Runtime library.
//!
//!   MEETESH_TEST_PARAKEET=/path/to/folder ORT_DYLIB_PATH=/path/libonnxruntime.so \
//!     cargo test -p meetesh-core --features parakeet-dynamic --test parakeet_real -- --nocapture
//!
//! (Builds with the `parakeet` feature link ONNX Runtime in; ORT_DYLIB_PATH is then not needed.)
//! The speech is synthetic (espeak-ng); this checks the engine's contract, not accuracy on real voices.
#![cfg(feature = "parakeet-engine")]

use std::path::{Path, PathBuf};
use std::time::Instant;

use meetesh_core::asr::parakeet::ParakeetTranscriber;
use meetesh_core::asr::Transcriber;
use meetesh_core::audio::wav::read_segment;

fn folder() -> Option<PathBuf> {
    std::env::var_os("MEETESH_TEST_PARAKEET").map(PathBuf::from)
}

fn fixture(i: usize) -> Vec<f32> {
    read_segment(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/speech/s{i}.wav"))).unwrap()
}

fn text(t: &dyn Transcriber, a: &[f32]) -> String {
    t.transcribe(a).unwrap().into_iter().map(|s| s.text).collect::<Vec<_>>().join(" ")
}

#[test]
fn transcribes_speech_with_timestamps_and_survives_odd_input() {
    let Some(dir) = folder() else {
        eprintln!("MEETESH_TEST_PARAKEET not set; skipping");
        return;
    };
    let t0 = Instant::now();
    let t = ParakeetTranscriber::load("parakeet-test", &dir).unwrap();
    eprintln!("load: {:?}", t0.elapsed());
    assert_eq!(t.model_id(), "parakeet-test");

    // Speech comes back as text, with punctuation, and timestamps inside the clip.
    let a = fixture(4);
    let secs = a.len() as f32 / 16_000.0;
    let t1 = Instant::now();
    let segs = t.transcribe(&a).unwrap();
    eprintln!("{secs:.1} s of speech in {:?}: {:?}", t1.elapsed(), segs);
    let joined = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    assert!(joined.contains("budget") && joined.contains("friday"), "heard: {joined}");
    assert!(segs.iter().all(|s| s.start_ms >= 0 && s.end_ms >= s.start_ms && s.end_ms <= (secs * 1000.0) as i64 + 1), "{segs:?}");

    // Chunks of any length are accepted: empty, very short, silence, and a long one.
    assert!(t.transcribe(&[]).unwrap().is_empty());
    assert!(text(&t, &vec![0.0; 800]).is_empty());
    assert!(text(&t, &vec![0.0; 16_000 * 3]).is_empty(), "silence must not turn into words");
    let short = &a[..a.len().min(4_000)]; // a quarter second of speech
    t.transcribe(short).unwrap();
    let long: Vec<f32> = (1..=6).flat_map(fixture).collect(); // about 26 s
    let heard = text(&t, &long).to_lowercase();
    assert!(heard.contains("launch") && heard.contains("customer feedback"), "{heard}");
}
