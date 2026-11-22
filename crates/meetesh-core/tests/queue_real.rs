//! Real-model check of the background rules: while meeting notes are being written, a hint asked
//! for takes the processor at once (the notes step is stopped, waits, and is redone afterwards), and
//! the notes still finish. Opt-in:
//!
//!   MEETESH_TEST_ASSIST_MODEL=/path/to/model.gguf \
//!     cargo test -p meetesh-core --features llm --test queue_real -- --nocapture
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meetesh_core::assist::gate::{ModelGate, PauseReason};
use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::prompt::{build_hint_messages, Budget, HintContext};
use meetesh_core::assist::summary::{summarize, SummaryRun};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::{Source, TranscriptSegment};

const CALL: &[(bool, &str)] = &[
    (true, "Okay everyone, the goal today is to decide when we launch the new onboarding flow."),
    (true, "Priya, where are we with the testing?"),
    (false, "Testing is mostly done. We fixed two bugs in email verification. I would like one more day on Android."),
    (true, "So can we still launch on Friday?"),
    (false, "Friday works if the Android run is clean on Thursday. If not, we slip to Monday."),
    (true, "Fine. We launch on Friday unless Android fails on Thursday. Marcus, please write the release notes."),
    (false, "I will have the release notes ready by Thursday noon."),
];

fn segments() -> Vec<TranscriptSegment> {
    CALL.iter()
        .enumerate()
        .map(|(i, (other, t))| {
            let s = TranscriptSegment::new(i as i64 * 8_000, i as i64 * 8_000 + 5_000, *t);
            if *other { s.from_source(Source::System) } else { s }
        })
        .collect()
}

fn hint(a: &LlamaAssistant) -> (Duration, String) {
    let ctx = HintContext { agenda: String::new(), recent: segments() };
    let t = Instant::now();
    let c = a
        .complete(&build_hint_messages(&ctx, Budget::default()), &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |_| true)
        .expect("hint");
    (t.elapsed(), c.text)
}

#[test]
fn a_hint_takes_the_processor_from_running_notes_and_the_notes_still_finish() {
    let Some(path) = std::env::var("MEETESH_TEST_ASSIST_MODEL").ok().map(PathBuf::from) else {
        return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set");
    };
    let a = Arc::new(LlamaAssistant::load("m", &path, 4096).expect("load"));
    let (idle, _) = hint(&a);
    eprintln!("hint on an idle model: {:.1}s", idle.as_secs_f32());

    let gate = ModelGate::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let run = SummaryRun {
        cancel: Arc::new(AtomicBool::new(false)),
        yield_to: Some(Arc::new(std::sync::atomic::AtomicUsize::new(0))),
        gate: Some(gate.clone()),
        cpu_bound: true,
        on_pause: Some(Arc::new(move |r| ev.lock().unwrap().push((Instant::now(), r)))),
    };
    let (a2, started) = (a.clone(), Instant::now());
    let notes = std::thread::spawn(move || summarize(a2.as_ref(), &segments(), "", &run, |_, _| {}));

    // Let the notes get well into their request, then ask for a hint.
    std::thread::sleep(Duration::from_secs(4));
    let asked = Instant::now();
    let guard = gate.begin_interactive();
    let (during, text) = hint(&a);
    drop(guard);
    eprintln!("hint asked 4 s into the notes: {:.1}s ({} chars)", during.as_secs_f32(), text.len());

    let summary = notes.join().unwrap().expect("the notes must finish after the hint");
    eprintln!("notes finished {:.1}s after they started; {} decisions", started.elapsed().as_secs_f32(), summary.decisions.len());
    let ev = events.lock().unwrap();
    eprintln!("pause events: {:?}", ev.iter().map(|(t, r)| (t.duration_since(asked).as_secs_f32(), *r)).collect::<Vec<_>>());
    assert!(ev.iter().any(|(_, r)| *r == Some(PauseReason::Hint)), "the notes paused for the hint");
    assert!(ev.iter().any(|(_, r)| r.is_none()), "and carried on afterwards");
    assert!(during < idle + Duration::from_secs(4), "a hint should not wait for the whole notes step: idle {idle:?}, during {during:?}");
    assert!(!summary.is_empty());
}
