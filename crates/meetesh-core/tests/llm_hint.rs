//! Real-model check for live assist: load the GGUF, stream a hint, cancel, and report timing and
//! memory. Opt-in (needs a downloaded model and a lot of RAM):
//!
//!   MEETESH_TEST_ASSIST_MODEL=/path/to/gemma-4-E2B-it-Q4_K_M.gguf \
//!     cargo test -p meetesh-core --features llm --test llm_hint -- --nocapture
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::prompt::{build_hint_messages, clean_hint, Budget, HintContext};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::TranscriptSegment;

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(str::to_string))
        .and_then(|l| l.split_whitespace().nth(1).and_then(|n| n.parse::<u64>().ok()))
        .map_or(0, |kb| kb / 1024)
}

fn seg(start_ms: i64, text: &str) -> TranscriptSegment {
    TranscriptSegment { start_ms, end_ms: start_ms + 3000, text: text.into() }
}

fn model_path() -> Option<PathBuf> {
    std::env::var_os("MEETESH_TEST_ASSIST_MODEL").map(PathBuf::from)
}

#[test]
fn streams_a_useful_hint_and_can_be_cancelled() {
    let Some(path) = model_path() else {
        eprintln!("MEETESH_TEST_ASSIST_MODEL not set; skipping");
        return;
    };
    let rss0 = rss_mb();
    let t = Instant::now();
    let a = LlamaAssistant::load("test-assist", &path, 4096).expect("model loads");
    let load_ms = t.elapsed().as_millis();
    let rss_loaded = rss_mb();

    let ctx = HintContext {
        agenda: "Decide the launch date. Confirm who owns the pricing page.".into(),
        recent: vec![
            seg(5_000, "So the design is basically done, we are just waiting on legal for the terms page."),
            seg(14_000, "The engineering side thinks the 15th is realistic, but support wants another week."),
            seg(26_000, "What is your view on moving the release to the 15th? Are you comfortable with that?"),
        ],
    };
    let messages = build_hint_messages(&ctx, Budget::default());

    // 1. a normal hint, streamed
    let mut pieces = 0usize;
    let mut streamed = String::new();
    let done = a
        .complete(&messages, &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |p| {
            pieces += 1;
            streamed.push_str(p);
            true
        })
        .expect("completion");
    let rss_after = rss_mb();
    let tps = if done.stats.total_ms > done.stats.first_token_ms {
        done.stats.generated_tokens as f64 * 1000.0 / (done.stats.total_ms - done.stats.first_token_ms) as f64
    } else {
        0.0
    };
    eprintln!("load {load_ms} ms | RSS before {rss0} MB, loaded {rss_loaded} MB, after generation {rss_after} MB");
    eprintln!(
        "prompt {} tokens, generated {} tokens | first token {} ms, total {} ms, ~{:.1} tokens/s",
        done.stats.prompt_tokens, done.stats.generated_tokens, done.stats.first_token_ms, done.stats.total_ms, tps
    );
    eprintln!("raw hint   : {:?}", done.text);
    eprintln!("cleaned    : {:?}", clean_hint(&done.text));

    assert_eq!(streamed, done.text, "streamed pieces add up to the full text");
    assert!(pieces > 1, "text is streamed in several pieces");
    assert!(done.stats.generated_tokens > 0 && !done.stats.cancelled);
    assert!(clean_hint(&done.text).is_some(), "a direct question should get a hint, got {:?}", done.text);
    assert!(done.text.len() < 600, "hints are short");

    // 2. cancel after the first piece
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let cancelled = a
        .complete(&messages, &GenOptions::hint(cancel.clone()), &mut |_| {
            c2.store(true, Ordering::Relaxed);
            true
        })
        .expect("cancelled completion");
    eprintln!("cancelled run generated {} tokens", cancelled.stats.generated_tokens);
    assert!(cancelled.stats.cancelled && cancelled.stats.generated_tokens <= 2);

    // 3. a second request on the same model works (context is recreated per request)
    let again = a
        .complete(&messages, &GenOptions { temperature: 0.0, ..GenOptions::hint(Arc::new(AtomicBool::new(false))) }, &mut |_| true)
        .expect("second completion");
    eprintln!("second run: first token {} ms, total {} ms", again.stats.first_token_ms, again.stats.total_ms);
    assert!(!again.text.is_empty());
}

#[test]
fn reports_load_progress_warms_up_and_recovers_from_a_gpu_failure() {
    let Some(path) = model_path() else {
        eprintln!("MEETESH_TEST_ASSIST_MODEL not set; skipping");
        return;
    };
    // 1. real loading progress from llama.cpp: increasing, ending at 1.0
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<f32>::new()));
    let s2 = seen.clone();
    let a = LlamaAssistant::load_with_progress("test-assist", &path, 4096, move |p| s2.borrow_mut().push(p)).expect("loads");
    let seen = seen.borrow();
    eprintln!("load progress: {} updates, first {:?}, last {:?}", seen.len(), seen.first(), seen.last());
    assert!(seen.len() > 3, "expected several progress updates");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "progress never goes backwards");
    assert!(*seen.last().unwrap() > 0.99);

    // 2. warm-up works and reports which backend ran
    let w = a.warm_up().expect("warm-up");
    eprintln!("warm-up: {} ms, gpu={}, fallback={}", w.total_ms, w.on_gpu, w.cpu_fallback);
    assert!(!w.cpu_fallback);

    // 3. a GPU compute failure is recovered transparently on the CPU
    a.simulate_gpu_failure_once();
    assert!(a.uses_gpu(), "the hook pretends the GPU is in use");
    let ctx = HintContext {
        agenda: String::new(),
        recent: vec![seg(1_000, "Can you send me the report by Friday?")],
    };
    let messages = build_hint_messages(&ctx, Budget::default());
    let mut pieces = String::new();
    let done = a
        .complete(&messages, &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |p| {
            pieces.push_str(p);
            true
        })
        .expect("falls back and succeeds");
    eprintln!("after injected GPU failure: cpu_fallback={} on_gpu={} text={:?}", done.stats.cpu_fallback, done.stats.on_gpu, done.text);
    assert!(done.stats.cpu_fallback && !done.stats.on_gpu);
    assert!(done.stats.fallback_reason.as_deref().is_some_and(|r| r.contains("decode failed")), "{:?}", done.stats.fallback_reason);
    assert!(!done.text.is_empty());
    assert_eq!(pieces, done.text, "no text was duplicated by the retry");
    assert!(!a.uses_gpu(), "later requests stay on the CPU");

    // 4. and the next request works normally
    let again = a.complete(&messages, &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |_| true).expect("next request");
    assert!(!again.stats.cpu_fallback && !again.text.is_empty());
}
