//! Real-model check of hints with a memory of earlier meetings: an earlier meeting decided a cap, the
//! current call asks to go over it. Prints the hint with and without the earlier notes. Opt-in:
//!
//!   MEETESH_TEST_ASSIST_MODEL=/path/to/gemma-4-E2B-it-Q4_K_M.gguf \
//!     cargo test -p meetesh-core --features llm --test hint_memory_real -- --nocapture
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::memory::{keywords, plain_excerpt, Related};
use meetesh_core::assist::prompt::{build_hint_messages, clean_hint, Budget, HintContext};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::{Db, Source, TranscriptSegment};

fn call() -> Vec<TranscriptSegment> {
    let o = |ms: i64, t: &str| TranscriptSegment::new(ms, ms + 4000, t).from_source(Source::System);
    let me = |ms: i64, t: &str| TranscriptSegment::new(ms, ms + 3000, t);
    vec![
        o(0, "Thanks for joining. We need to finalize the vendor contract for the spring campaign."),
        me(6_000, "Right, I have been going back and forth with them."),
        o(11_000, "They are asking for sixty thousand dollars. Can we sign the vendor contract today?"),
    ]
}

fn hint(a: &LlamaAssistant, related: Vec<Related>) -> (String, f32) {
    let ctx = HintContext { agenda: String::new(), recent: call(), related };
    let t = Instant::now();
    let c = a
        .complete(&build_hint_messages(&ctx, Budget::default()), &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |_| true)
        .expect("hint");
    (clean_hint(&c.text).unwrap_or_else(|| "(no hint)".into()), t.elapsed().as_secs_f32())
}

#[test]
fn an_earlier_decision_shows_up_in_the_hint() {
    let Some(path) = std::env::var("MEETESH_TEST_ASSIST_MODEL").ok().map(PathBuf::from) else {
        return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set");
    };
    // An earlier meeting, an unrelated one, and the one in progress.
    let db = Db::open_in_memory().unwrap();
    let old = db.create_meeting(Some("Budget review")).unwrap();
    db.set_summary(old.id, "## Summary\nThe team reviewed the marketing budget for the year.\n\n## Decisions\n- The vendor contract for the spring campaign is capped at 40 thousand dollars\n- Sam owns the vendor contract\n\n## Action items\n- Sam: Get a second quote from another vendor", "m").unwrap();
    let other = db.create_meeting(Some("Team lunch")).unwrap();
    db.replace_transcript(other.id, "m", &[TranscriptSegment::new(0, 1000, "We talked about where to eat on Friday")]).unwrap();
    let now = db.create_meeting(Some("Vendor call")).unwrap();

    let words = keywords(&call(), 4);
    let found = db.related(now.id, &words, 3).unwrap();
    eprintln!("keywords: {words:?}");
    assert!(found.iter().any(|r| r.meeting_id == old.id), "the budget meeting must be found: {found:?}");
    assert!(!found.iter().any(|r| r.meeting_id == other.id || r.meeting_id == now.id));
    let related: Vec<Related> = found
        .iter()
        .map(|r| {
            let m = r.matches.iter().find(|m| m.kind == meetesh_core::db::MatchKind::Notes).unwrap_or(&r.matches[0]);
            Related { meeting_id: r.meeting_id, title: r.title.clone(), day: db.day_label(r.created_at).unwrap(), excerpt: plain_excerpt(&m.snippet, 240) }
        })
        .collect();
    eprintln!("excerpt: {:?}", related[0].excerpt);

    let a = LlamaAssistant::load("m", &path, 4096).expect("load");
    let (without, t0) = hint(&a, vec![]);
    let (with, t1) = hint(&a, related);
    eprintln!("WITHOUT memory ({t0:.1}s): {without}");
    eprintln!("WITH memory    ({t1:.1}s): {with}");
    let lower = with.to_lowercase();
    assert!(lower.contains("40") || lower.contains("cap") || lower.contains("budget review") || lower.contains("forty"), "the hint should use the earlier decision: {with}");
}
