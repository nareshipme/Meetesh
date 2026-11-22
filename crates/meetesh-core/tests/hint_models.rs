//! Compares language models for live hints on the same scripted meeting moments: speed, memory,
//! whether they follow the "at most two short sentences, plain text" rule, and what they actually say.
//! Opt-in. Several GGUF files, comma separated:
//!
//!   MEETESH_TEST_HINT_MODELS=/a/model1.gguf,/b/model2.gguf \
//!     cargo test --release -p meetesh-core --features llm --test hint_models -- --nocapture
//!
//! The dialogue is scripted, so this says how a model behaves on these cases, not how good it is in
//! general. Whether a hint is *useful* is for a person to judge from the printed text.
#![cfg(feature = "llm")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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

fn segs(lines: &[&str]) -> Vec<TranscriptSegment> {
    lines.iter().enumerate().map(|(i, t)| TranscriptSegment::new(i as i64 * 8_000, i as i64 * 8_000 + 4_000, *t)).collect()
}

struct Case {
    name: &'static str,
    agenda: &'static str,
    lines: &'static [&'static str],
    /// Words a good hint would plausibly mention (any one counts as "on topic").
    topical: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "asked a direct question",
        agenda: "Decide the launch date. Confirm who owns the pricing page.",
        lines: &[
            "So the design is basically done, we are just waiting on legal for the terms page.",
            "The engineering side thinks the 15th is realistic, but support wants another week.",
            "What is your view on moving the release to the 15th? Are you comfortable with that?",
        ],
        topical: &["15th", "support", "week", "release", "date", "legal", "risk", "launch"],
    },
    Case {
        name: "a commitment with no owner",
        agenda: "",
        lines: &[
            "We should really send the revised budget to finance before the end of the week.",
            "Yes, someone needs to do that, otherwise the quarter closes without it.",
            "Okay, so, let's move on to the hiring plan then.",
        ],
        topical: &["budget", "finance", "who", "owner", "assign", "week", "deadline"],
    },
    Case {
        name: "a number worth checking",
        agenda: "Vendor contract renewal.",
        lines: &[
            "The vendor wants sixty thousand a year, which is up from forty last year.",
            "They said that's because of the new support tier, but we never asked for that tier.",
            "I think we should just sign it, we are running out of time.",
        ],
        topical: &["support tier", "tier", "negotiat", "forty", "sixty", "increase", "60", "40", "price", "cost", "asked"],
    },
    Case {
        name: "small talk (a hint is not needed)",
        agenda: "",
        lines: &["Hi everyone, can you hear me okay?", "Yes, loud and clear. How was your weekend?", "Pretty good, we went hiking. Let me share my screen."],
        topical: &[],
    },
    Case {
        name: "a vague claim",
        agenda: "Quarterly planning.",
        lines: &[
            "Customers are generally pretty happy with the new onboarding flow.",
            "Right, I think churn has probably gone down because of it.",
            "So we can assume that the onboarding project was a success?",
        ],
        topical: &["data", "evidence", "churn", "metric", "measure", "number", "confirm", "assume", "verify"],
    },
];

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

fn sentences(s: &str) -> usize {
    s.split(['.', '?', '!']).filter(|p| !p.trim().is_empty()).count()
}

#[test]
fn compare_models_on_scripted_hints() {
    let Some(list) = std::env::var("MEETESH_TEST_HINT_MODELS").ok() else {
        return eprintln!("MEETESH_TEST_HINT_MODELS not set; skipping");
    };
    let mut summary = vec![];
    for path in list.split(',').map(PathBuf::from) {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let rss0 = rss_mb();
        let t = Instant::now();
        let a = match LlamaAssistant::load(&name, &path, 4096) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("\n=== {name}: DID NOT LOAD: {e}");
                summary.push(format!("{name:<22} did not load: {e}"));
                continue;
            }
        };
        let load_ms = t.elapsed().as_millis();
        let rss = rss_mb().saturating_sub(rss0);
        // warm-up, like the app does
        let warm = [meetesh_core::assist::ChatMessage::new(meetesh_core::assist::ChatRole::User, "Say hi")];
        let _ = a.complete(&warm, &GenOptions { max_tokens: 4, ..GenOptions::hint(Arc::new(AtomicBool::new(false))) }, &mut |_| true);
        eprintln!("\n=== {name}  (load {load_ms} ms, +{rss} MB resident)");

        let (mut total_ms, mut first_ms, mut n, mut rule_ok, mut on_topic, mut topical_cases, mut tokens, mut gen_ms) = (0u64, 0u64, 0u32, 0u32, 0u32, 0u32, 0usize, 0u64);
        let mut quiet_ok = false;
        for case in CASES {
            let ctx = HintContext { related: vec![], agenda: case.agenda.into(), recent: segs(case.lines) };
            let msgs = build_hint_messages(&ctx, Budget::default());
            let c = a.complete(&msgs, &GenOptions::hint(Arc::new(AtomicBool::new(false))), &mut |_| true).expect("completion");
            let cleaned = clean_hint(&c.text);
            let shown = cleaned.clone().unwrap_or_default();
            let w = words(&shown);
            let follows = cleaned.is_none() || (sentences(&shown) <= 2 && w <= 45 && !shown.contains('*') && !shown.contains('#') && !shown.contains("<think>"));
            eprintln!(
                "  [{}] {:>5} ms (first {:>4} ms) {:>2} words {}\n      {}",
                case.name,
                c.stats.total_ms,
                c.stats.first_token_ms,
                w,
                if follows { "" } else { "BREAKS THE RULE" },
                if cleaned.is_some() { shown.clone() } else { format!("(no hint; raw: {:?})", c.text.chars().take(80).collect::<String>()) }
            );
            n += 1;
            total_ms += c.stats.total_ms;
            first_ms += c.stats.first_token_ms;
            tokens += c.stats.generated_tokens;
            gen_ms += c.stats.total_ms.saturating_sub(c.stats.first_token_ms);
            rule_ok += u32::from(follows);
            if case.topical.is_empty() {
                // small talk: staying quiet (NO_HINT) is the good answer
                quiet_ok = cleaned.is_none();
            } else {
                topical_cases += 1;
                let low = shown.to_lowercase();
                on_topic += u32::from(case.topical.iter().any(|k| low.contains(k)));
            }
        }
        let tps = if gen_ms > 0 { tokens as f64 * 1000.0 / gen_ms as f64 } else { 0.0 };
        summary.push(format!(
            "{name:<22} load {load_ms:>5} ms  +{rss:>4} MB  hint {:>5} ms (first {:>4})  {tps:>5.1} tok/s  rule {rule_ok}/{n}  on-topic {on_topic}/{topical_cases}  quiet on small talk: {}",
            total_ms / n as u64,
            first_ms / n as u64,
            if quiet_ok { "yes" } else { "no" }
        ));
    }
    eprintln!("\n--- summary (mean per hint; on-topic is a crude keyword check, read the text above) ---");
    for s in summary {
        eprintln!("{s}");
    }
}
