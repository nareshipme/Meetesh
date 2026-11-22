//! Does generating a hint slow down live transcription? Plays speech through live transcription in
//! real time while hints are generated back to back, and measures how far behind the speech each
//! transcript segment arrives. Needs both models, so it is opt-in:
//!
//!   MEETESH_TEST_MODEL=/path/ggml-base.en.bin MEETESH_TEST_ASSIST_MODEL=/path/gemma.gguf \
//!     cargo test -p meetesh-core --features "whisper llm" --test hint_vs_live -- --nocapture
#![cfg(all(feature = "whisper", feature = "llm"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meetesh_core::asr::live::{live_chunker_config, LiveTranscriber};
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::asr::{TranscriptSegment, Transcriber};
use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::prompt::{build_hint_messages, Budget, HintContext};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::audio::wav::read_segment;
use meetesh_core::audio::RecordingSession;

fn speech() -> Vec<f32> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech");
    let mut audio = vec![0.0f32; 16_000]; // 1 s lead-in
    for _round in 0..2 {
        for i in 1..=6 {
            audio.extend(read_segment(&dir.join(format!("s{i}.wav"))).unwrap());
            audio.extend(vec![0.0f32; 16_000]); // 1 s pause
        }
    }
    audio.extend(vec![0.0f32; 24_000]);
    audio
}

fn canned_context() -> HintContext {
    let seg = |start_ms: i64, text: &str| TranscriptSegment::new(start_ms, start_ms + 3000, text);
    HintContext {
        agenda: "Decide the launch date. Confirm who owns pricing.".into(),
        recent: vec![
            seg(2_000, "Let's start with the launch date. Engineering thinks the fifteenth is realistic."),
            seg(9_000, "Support wants another week to finish the training material."),
            seg(15_000, "Priya, can you confirm who owns the pricing page?"),
        ],
    }
}

#[derive(Debug)]
struct Result {
    label: &'static str,
    segments: usize,
    mean_lag: f64,
    max_lag: f64,
    dropped_chunks: u64,
    hints: usize,
    mean_hint_s: f64,
    mean_yield_s: f64,
}

/// `hints`: None = no hints at all; Some(false) = hints that ignore transcription; Some(true) = hints that yield to it.
fn run(label: &'static str, whisper: &Arc<WhisperTranscriber>, assistant: &Arc<LlamaAssistant>, hints: Option<bool>) -> Result {
    let audio = speech();
    let pending = Arc::new(AtomicUsize::new(0));
    let arrivals: Arc<Mutex<Vec<(f64, TranscriptSegment)>>> = Arc::new(Mutex::new(vec![]));
    let start = Instant::now();
    let (a, w) = (arrivals.clone(), whisper.clone());
    let (done_tx, done_rx) = channel();
    let live = LiveTranscriber::start_with_load(
        live_chunker_config(),
        8,
        Box::new(move || Ok(w as Arc<dyn Transcriber>)),
        move |segs| {
            let t = start.elapsed().as_secs_f64();
            a.lock().unwrap().extend(segs.into_iter().map(|s| (t, s)));
        },
        move |r| {
            let _ = done_tx.send(r);
        },
        pending.clone(),
    );
    let session = RecordingSession::start(vec![Box::new(live)], 4096);
    let mut pusher = session.pusher(16_000, 1);

    // Hints back to back from t = 6 s until the audio ends: the worst case for contention.
    let stop_hints = Arc::new(AtomicBool::new(false));
    let hint_thread = hints.map(|yielding| {
        let (assistant, pending, stop) = (assistant.clone(), pending.clone(), stop_hints.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(6));
            let messages = build_hint_messages(&canned_context(), Budget::default());
            let mut done: Vec<(f64, f64)> = vec![];
            while !stop.load(Ordering::Relaxed) {
                let mut o = GenOptions::hint(stop.clone());
                if yielding {
                    o = o.yielding_to(pending.clone());
                }
                let t = Instant::now();
                if let Ok(c) = assistant.complete(&messages, &o, &mut |_| true) {
                    if !c.stats.cancelled {
                        done.push((t.elapsed().as_secs_f64(), c.stats.yielded_ms as f64 / 1000.0));
                    }
                }
            }
            done
        })
    });

    // Play the audio in real time, like a microphone (10 ms per callback).
    let t0 = Instant::now();
    for (i, c) in audio.chunks(160).enumerate() {
        pusher.push_interleaved(c);
        if let Some(wait) = Duration::from_millis(10 * (i as u64 + 1)).checked_sub(t0.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    stop_hints.store(true, Ordering::Relaxed);
    let hint_runs = hint_thread.map(|h| h.join().unwrap()).unwrap_or_default();
    session.stop();
    let report = done_rx.recv_timeout(Duration::from_secs(120)).expect("live worker finished");

    let arrivals = arrivals.lock().unwrap();
    let lags: Vec<f64> = arrivals.iter().map(|(arrived, s)| arrived - s.end_ms as f64 / 1000.0).collect();
    let n = lags.len().max(1) as f64;
    let hn = hint_runs.len().max(1) as f64;
    Result {
        label,
        segments: lags.len(),
        mean_lag: lags.iter().sum::<f64>() / n,
        max_lag: lags.iter().cloned().fold(0.0, f64::max),
        dropped_chunks: report.dropped_chunks,
        hints: hint_runs.len(),
        mean_hint_s: hint_runs.iter().map(|r| r.0).sum::<f64>() / hn,
        mean_yield_s: hint_runs.iter().map(|r| r.1).sum::<f64>() / hn,
    }
}

#[test]
fn hints_versus_live_transcription() {
    let (Some(asr), Some(llm)) = (
        std::env::var_os("MEETESH_TEST_MODEL").map(PathBuf::from),
        std::env::var_os("MEETESH_TEST_ASSIST_MODEL").map(PathBuf::from),
    ) else {
        eprintln!("model env vars not set; skipping");
        return;
    };
    let whisper = Arc::new(WhisperTranscriber::load("asr", &asr, Some("en")).unwrap());
    let assistant = Arc::new(LlamaAssistant::load("assist", &llm, 4096).unwrap());
    assistant.warm_up().unwrap();
    whisper.transcribe(&vec![0.0f32; 16_000]).unwrap(); // warm up too

    let only = std::env::var("BENCH_ONLY").ok();
    let all = [("no hints (reference)", None), ("hints, ignoring transcription (before)", Some(false)), ("hints, yielding to transcription (after)", Some(true))];
    eprintln!("\nBENCH {:<42} {:>5} {:>9} {:>8} {:>8} {:>6} {:>10} {:>10}", "variant", "segs", "mean lag", "max lag", "dropped", "hints", "hint time", "held back");
    let mut results = vec![];
    for (label, hints) in all {
        if only.as_deref().is_some_and(|o| !label.contains(o)) {
            continue;
        }
        let r = run(label, &whisper, &assistant, hints);
        eprintln!(
            "BENCH {:<42} {:>5} {:>8.1}s {:>7.1}s {:>8} {:>6} {:>9.1}s {:>9.1}s",
            r.label, r.segments, r.mean_lag, r.max_lag, r.dropped_chunks, r.hints, r.mean_hint_s, r.mean_yield_s
        );
        results.push(r);
    }
    // When all three ran: yielding must keep transcription close to the no-hints reference and must
    // not be worse than ignoring transcription.
    if results.len() == 3 {
        let (reference, before, after) = (&results[0], &results[1], &results[2]);
        assert_eq!(after.dropped_chunks, 0, "yielding hints must not make live chunks get dropped");
        assert!(after.max_lag <= before.max_lag * 0.7 + 1.0, "max lag {:.1}s should be clearly below {:.1}s", after.max_lag, before.max_lag);
        assert!(after.mean_lag <= reference.mean_lag * 2.0 + 3.0, "mean lag {:.1}s should stay near the no-hints {:.1}s", after.mean_lag, reference.mean_lag);
        assert!(after.hints >= 1, "hints still have to complete");
    }
}
