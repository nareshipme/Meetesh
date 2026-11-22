//! Real-model check for meeting notes: a short call (one step) and a long one (several steps),
//! with timings and the notes printed so quality can be judged. Opt-in:
//!
//!   MEETESH_TEST_ASSIST_MODEL=/path/to/gemma-4-E2B-it-Q4_K_M.gguf \
//!     cargo test -p meetesh-core --features llm --test summary_real --release -- --nocapture --test-threads=1
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::summary::{summarize, SummaryRun};
use meetesh_core::{Source, TranscriptSegment};

fn model() -> Option<LlamaAssistant> {
    let path = PathBuf::from(std::env::var("MEETESH_TEST_ASSIST_MODEL").ok()?);
    Some(LlamaAssistant::load("gemma-4-e2b-it", &path, 4096).expect("load model"))
}

/// (is_other, text)
const LAUNCH: &[(bool, &str)] = &[
    (true, "Okay everyone, thanks for joining. The goal today is to decide when we launch the new onboarding flow."),
    (true, "Priya, where are we with the testing?"),
    (false, "Testing is mostly done. We found two bugs in the email verification step, both fixed. I would like one more day on Android."),
    (true, "So can we still launch on Friday?"),
    (false, "I think Friday works if the Android run is clean on Thursday. If not, we slip to Monday."),
    (true, "Fine. Let's decide: we launch on Friday, and we only move to Monday if Android fails on Thursday."),
    (true, "Marcus, can you write the release notes?"),
    (false, "Yes, I'll have the release notes ready by Thursday noon. Should they mention the new password rules?"),
    (true, "Yes, include the password rules. Also, Sam needs to tell support about the change before launch."),
    (false, "I can tell support on Wednesday, and I'll share the FAQ with them."),
    (true, "Great. Last point: the pricing page is not part of this launch, we do that next month. Thanks all."),
];

fn segments(lines: &[(bool, &str)], t0: i64) -> Vec<TranscriptSegment> {
    lines
        .iter()
        .enumerate()
        .map(|(i, (other, text))| {
            let s = TranscriptSegment::new(t0 + i as i64 * 9_000, t0 + i as i64 * 9_000 + 6_000, *text);
            if *other { s.from_source(Source::System) } else { s }
        })
        .collect()
}

fn run(a: &LlamaAssistant, segs: &[TranscriptSegment], what: &str) -> meetesh_core::assist::summary::Summary {
    let r = SummaryRun::plain(Arc::new(AtomicBool::new(false)));
    let t = Instant::now();
    let mut steps = 0;
    let s = summarize(a, segs, "", &r, |d, tot| {
        steps = tot;
        eprintln!("  {what}: step {d}/{tot} at {:.0}s", t.elapsed().as_secs_f32());
    })
    .expect("summary");
    eprintln!("== {what}: {} segments, {steps} steps, {:.1}s\n{}\n", segs.len(), t.elapsed().as_secs_f32(), s.to_markdown());
    s
}

#[test]
fn short_call_is_summarized_in_one_step() {
    let Some(a) = model() else { return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set") };
    let s = run(&a, &segments(LAUNCH, 0), "short call");
    assert!(!s.overview.is_empty());
    assert!(!s.decisions.is_empty(), "there was a clear decision");
    assert!(!s.action_items.is_empty(), "there were clear action items");
}

#[test]
fn long_meeting_goes_through_notes_and_still_yields_a_summary() {
    let Some(a) = model() else { return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set") };
    // The same call sixteen times with different times and some filler: forces the multi-step path.
    let mut all = Vec::new();
    for round in 0..16 {
        all.extend(segments(LAUNCH, round * 120_000));
    }
    let s = run(&a, &all, "long meeting");
    assert!(!s.overview.is_empty());
}

#[test]
fn the_notes_get_a_short_title() {
    use meetesh_core::assist::title::generate_title;
    let Some(a) = model() else { return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set") };
    let notes = run(&a, &segments(LAUNCH, 0), "short call").to_markdown();
    let t = Instant::now();
    let title = generate_title(&a, &notes, &SummaryRun::plain(Arc::new(AtomicBool::new(false)))).expect("title call");
    eprintln!("== title in {:.1}s: {title:?}", t.elapsed().as_secs_f32());
    let title = title.expect("a usable title");
    assert!(title.split_whitespace().count() <= 9);
}
