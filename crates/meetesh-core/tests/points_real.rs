//! Real-model check of live points: a scripted call in three stretches; prints the list after each
//! update and how long it took. Opt-in:
//!
//!   MEETESH_TEST_ASSIST_MODEL=/path/to/gemma-4-E2B-it-Q4_K_M.gguf \
//!     cargo test -p meetesh-core --features llm --test points_real -- --nocapture
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::assist::points::{update_points, Points};
use meetesh_core::assist::summary::SummaryRun;
use meetesh_core::{Source, TranscriptSegment};

fn seg(i: usize, other: bool, t: &str) -> TranscriptSegment {
    let s = TranscriptSegment::new(i as i64 * 7_000, i as i64 * 7_000 + 5_000, t);
    if other { s.from_source(Source::System) } else { s }
}

#[test]
fn the_list_follows_the_conversation() {
    let Some(path) = std::env::var("MEETESH_TEST_ASSIST_MODEL").ok().map(PathBuf::from) else {
        return eprintln!("skipped: MEETESH_TEST_ASSIST_MODEL not set");
    };
    let a = LlamaAssistant::load("m", &path, 4096).expect("load");
    let stretches: Vec<Vec<TranscriptSegment>> = vec![
        vec![
            seg(0, true, "Let's start with the budget. Marketing wants to spend sixty thousand on the spring campaign."),
            seg(1, false, "That is over what we agreed. The cap from last quarter was forty thousand."),
            seg(2, true, "Fair. Sam, can you get a second quote from another vendor before we decide?"),
            seg(3, false, "Yes, I can get that by Wednesday."),
            seg(4, true, "Good. So the decision on the campaign budget waits for the second quote."),
        ],
        vec![
            seg(5, true, "Next topic, the launch date. Engineering says the fifteenth is realistic."),
            seg(6, false, "Support needs another week to finish the training material though."),
            seg(7, true, "Then let's say the twenty second, and Priya confirms with support by Friday."),
            seg(8, false, "Agreed, twenty second it is. I will message support today."),
        ],
        vec![
            seg(9, true, "Last thing, who owns the pricing page?"),
            seg(10, false, "I thought design owned it, but nobody has picked it up since Dana left."),
            seg(11, true, "Okay, that is an open question. Let's find an owner by next week."),
        ],
    ];
    let run = SummaryRun::plain(Arc::new(AtomicBool::new(false)));
    let mut cur = Points::default();
    for (i, s) in stretches.iter().enumerate() {
        let t = Instant::now();
        cur = update_points(&a, &cur, s, &run).expect("update");
        eprintln!("--- update {} ({:.1}s)\nNOW: {}\n{}", i + 1, t.elapsed().as_secs_f32(), cur.now, cur.points.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"));
        assert!(!cur.now.is_empty() && !cur.points.is_empty());
    }
    let all = format!("{} {}", cur.now, cur.points.join(" ")).to_lowercase();
    assert!(all.contains("pricing") || all.contains("owner"), "the latest topic should be in the list: {all}");
    assert!(all.contains("launch") || all.contains("22") || all.contains("twenty"), "an earlier decision should still be there: {all}");
}
