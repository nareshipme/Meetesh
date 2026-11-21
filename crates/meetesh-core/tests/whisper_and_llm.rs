//! Whisper (whisper.cpp) and the assist model (llama.cpp) both bundle their own copy of ggml. This
//! checks they can live in one process: transcribe with Whisper, then generate with the LLM.
//! Opt-in:
//!
//!   MEETESH_TEST_MODEL=/path/ggml-tiny.en.bin MEETESH_TEST_ASSIST_MODEL=/path/gemma.gguf \
//!     cargo test -p meetesh-core --features "whisper llm" --test whisper_and_llm -- --nocapture
#![cfg(all(feature = "whisper", feature = "llm"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use meetesh_core::asr::run::transcribe_recording;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::prompt::{build_hint_messages, Budget, HintContext};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::audio::wav::{read_segment, SegmentWriter};

#[test]
fn whisper_then_llm_in_one_process() {
    let (Some(asr), Some(llm)) = (
        std::env::var_os("MEETESH_TEST_MODEL").map(PathBuf::from),
        std::env::var_os("MEETESH_TEST_ASSIST_MODEL").map(PathBuf::from),
    ) else {
        eprintln!("model env vars not set; skipping");
        return;
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech.wav");
    let dir = std::env::temp_dir().join(format!("meetesh-both-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = SegmentWriter::new(&dir).unwrap();
    w.push(&read_segment(&fixture).unwrap()).unwrap();
    w.finish().unwrap();

    let whisper = WhisperTranscriber::load("asr", &asr, Some("en")).unwrap();
    let segs = transcribe_recording(&dir, &whisper, &AtomicBool::new(false), |_| {}, |_| {}).unwrap();
    assert!(!segs.is_empty());
    eprintln!("whisper heard: {:?}", segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>());

    let a = LlamaAssistant::load("assist", &llm, 4096).unwrap();
    let msgs = build_hint_messages(&HintContext { agenda: String::new(), recent: segs.clone() }, Budget::default());
    let out = a.complete(&msgs, &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |_| true).unwrap();
    eprintln!("assist said: {:?}", out.text);
    assert!(!out.text.is_empty());

    // and Whisper still works after the LLM ran
    let again = transcribe_recording(&dir, &whisper, &AtomicBool::new(false), |_| {}, |_| {}).unwrap();
    assert_eq!(again.len(), segs.len());
    std::fs::remove_dir_all(dir).ok();
}
