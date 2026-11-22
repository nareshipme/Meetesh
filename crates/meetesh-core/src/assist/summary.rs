//! Meeting notes from the transcript: a short summary, decisions and action items, written by the
//! same on-device model that gives hints.
//!
//! The model has a small context window (4k tokens), so a long meeting is summarized in steps:
//! the transcript is cut into parts that fit, each part becomes a few bullet notes, and the notes
//! (merged first if there are many) become the final summary. Short meetings take one step.
//! Prompts and parsing are pure and tested with a scripted fake model.

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;

use crate::assist::prompt::{has_others, strip_thinking, transcript_lines, truncate_chars, SPEAKER_NOTE};
use crate::assist::gate::{ModelGate, PauseReason};
use crate::assist::{Assistant, ChatMessage, ChatRole, GenOptions};
use crate::{Error, Result, TranscriptSegment};

/// Transcript text per step. About 2,000 tokens, leaving room for the instructions and the reply
/// in a 4k context.
pub const CHUNK_CHARS: usize = 8_000;
/// Notes from earlier steps that may be handed to one later step.
const NOTES_BUDGET_CHARS: usize = 7_000;
const AGENDA_CHARS: usize = 800;
const NOTES_TOKENS: usize = 260;
const SUMMARY_TOKENS: usize = 420;

const FINAL_INSTRUCTIONS: &str = "You write meeting notes. You are given a meeting transcript, or notes taken from it. \
It may contain recognition mistakes. Use only what was said: never invent facts, names, numbers or dates. \
Reply in exactly this format and nothing else:\n\
SUMMARY: <two to four sentences on what the meeting was about and how it ended>\n\
DECISIONS:\n\
- <one decision per line>\n\
ACTION ITEMS:\n\
- <who does what, and by when if it was said>\n\
Write \"- None\" under a heading if there is nothing for it.";

const NOTES_INSTRUCTIONS: &str = "You take notes on one part of a longer meeting. List the key points, decisions and \
action items said in this part as short bullet points, at most eight, each starting with \"- \". Use only what was \
said; never invent anything. Reply with the bullets only.";

const MERGE_INSTRUCTIONS: &str = "You merge notes taken from consecutive parts of one meeting. Combine them into at most \
ten short bullet points, each starting with \"- \", keeping every decision and action item and dropping repeats. \
Reply with the bullets only.";

/// The finished notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub overview: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<String>,
}

impl Summary {
    pub fn is_empty(&self) -> bool {
        self.overview.is_empty() && self.decisions.is_empty() && self.action_items.is_empty()
    }

    /// Markdown, as stored with the meeting and used for copying and export.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if !self.overview.is_empty() {
            out.push_str("## Summary\n");
            out.push_str(&self.overview);
            out.push_str("\n\n");
        }
        for (title, items) in [("Decisions", &self.decisions), ("Action items", &self.action_items)] {
            if items.is_empty() {
                continue;
            }
            out.push_str(&format!("## {title}\n"));
            for i in items {
                out.push_str(&format!("- {i}\n"));
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    /// Read the model's reply. Tolerant of markdown, numbering and heading spelling; anything
    /// before the first heading counts as the summary. `None` if there is nothing usable.
    pub fn parse(raw: &str) -> Option<Summary> {
        #[derive(Clone, Copy, PartialEq)]
        enum Part {
            Overview,
            Decisions,
            Actions,
        }
        let cleaned = strip_thinking(raw);
        let mut part = Part::Overview;
        let mut s = Summary::default();
        let mut overview: Vec<String> = Vec::new();

        for line in cleaned.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let (head, rest) = split_heading(t);
            if let Some(h) = head {
                part = match h {
                    "summary" => Part::Overview,
                    "decisions" => Part::Decisions,
                    _ => Part::Actions,
                };
                if !rest.is_empty() {
                    push_line(&mut s, &mut overview, part == Part::Overview, part == Part::Decisions, rest);
                }
                continue;
            }
            push_line(&mut s, &mut overview, part == Part::Overview, part == Part::Decisions, t);
        }
        s.overview = overview.join(" ").trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn push_line(s: &mut Summary, overview: &mut Vec<String>, is_overview: bool, is_decisions: bool, line: &str) {
    let item = strip_bullet(line);
    if item.is_empty() || is_none(&item) {
        return;
    }
    if is_overview {
        overview.push(item);
    } else if is_decisions {
        s.decisions.push(item);
    } else {
        s.action_items.push(item);
    }
}

/// `SUMMARY: text`, `**Decisions**`, `## Action items:` ... -> (canonical heading, text after it).
/// A line that merely starts with a heading word ("Actions were postponed") is not a heading.
fn split_heading(line: &str) -> (Option<&'static str>, &str) {
    const MARKUP: [char; 4] = ['#', '*', '_', ' '];
    let l = line.trim_start_matches(MARKUP);
    let lower = l.to_ascii_lowercase();
    for (name, canon) in [
        ("action items", "actions"),
        ("action item", "actions"),
        ("decisions", "decisions"),
        ("decision", "decisions"),
        ("actions", "actions"),
        ("summary", "summary"),
    ] {
        if lower.starts_with(name) {
            let tail = l[name.len()..].trim_start_matches(MARKUP);
            if tail.is_empty() {
                return (Some(canon), "");
            }
            if let Some(text) = tail.strip_prefix(':') {
                return (Some(canon), text.trim_start_matches(MARKUP).trim());
            }
        }
    }
    (None, line)
}

fn strip_bullet(line: &str) -> String {
    let t = line.trim();
    let t = t.trim_start_matches(|c: char| matches!(c, '-' | '*' | '•' | '–' | '—')).trim_start();
    // "1. text" / "2) text"
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    let t = if digits > 0 && matches!(t[digits..].chars().next(), Some('.') | Some(')')) { t[digits + 1..].trim_start() } else { t };
    t.trim().to_string()
}

fn is_none(item: &str) -> bool {
    matches!(item.trim_end_matches('.').to_ascii_lowercase().as_str(), "none" | "n/a" | "none noted" | "no decisions" | "no action items" | "nothing")
}

/// Cut transcript lines into parts of at most `max_chars`. Whole lines only; a single line longer
/// than the limit gets a part to itself.
pub fn split_lines(lines: &[String], max_chars: usize) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for l in lines {
        if !cur.is_empty() && cur.len() + l.len() + 1 > max_chars {
            parts.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(l);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn system(instructions: &str, labelled: bool) -> ChatMessage {
    let text = if labelled { format!("{instructions}\n{SPEAKER_NOTE}") } else { instructions.to_string() };
    ChatMessage::new(ChatRole::System, text)
}

fn final_messages(body: &str, from_notes: bool, agenda: &str, labelled: bool) -> Vec<ChatMessage> {
    let mut user = String::new();
    let agenda = agenda.trim();
    if !agenda.is_empty() {
        user.push_str("Meeting goals / agenda:\n");
        user.push_str(truncate_chars(agenda, AGENDA_CHARS));
        user.push_str("\n\n");
    }
    user.push_str(if from_notes { "Notes from the meeting, in order:\n" } else { "Meeting transcript:\n" });
    user.push_str(body);
    user.push_str("\n\nWrite the meeting notes.");
    vec![system(FINAL_INSTRUCTIONS, labelled), ChatMessage::new(ChatRole::User, user)]
}

fn notes_messages(part: &str, index: usize, total: usize, labelled: bool) -> Vec<ChatMessage> {
    let user = format!("Part {} of {} of the meeting transcript:\n{}\n\nList the notes.", index + 1, total, part);
    vec![system(NOTES_INSTRUCTIONS, labelled), ChatMessage::new(ChatRole::User, user)]
}

fn merge_messages(notes: &str) -> Vec<ChatMessage> {
    vec![system(MERGE_INSTRUCTIONS, false), ChatMessage::new(ChatRole::User, format!("Notes:\n{notes}\n\nMerge them."))]
}

/// Settings for one summary run.
pub struct SummaryRun {
    pub cancel: Arc<AtomicBool>,
    /// Pause between small steps while live transcription has work (see `GenOptions`).
    pub yield_to: Option<Arc<AtomicUsize>>,
    /// Background rules: wait (not cancel) while a hint is written, or, if `cpu_bound`, while a
    /// recording runs. `None` runs without ever waiting.
    pub gate: Option<Arc<ModelGate>>,
    /// The notes model runs on the processor (not a GPU), so it competes with live transcription.
    pub cpu_bound: bool,
    /// Told when the run starts waiting (`Some(why)`) and when it carries on (`None`).
    pub on_pause: Option<Arc<dyn Fn(Option<PauseReason>) + Send + Sync>>,
}

impl SummaryRun {
    /// A run with no gate and no waiting (tests, command line tools).
    pub fn plain(cancel: Arc<AtomicBool>) -> Self {
        SummaryRun { cancel, yield_to: None, gate: None, cpu_bound: false, on_pause: None }
    }

    fn opts(&self, max_tokens: usize) -> GenOptions {
        let mut o = GenOptions::notes(self.cancel.clone(), max_tokens);
        if let Some(p) = &self.yield_to {
            o = o.yielding_to(p.clone());
        }
        if let Some(g) = &self.gate {
            o = o.preempted_by(g.preempt_check(self.cpu_bound));
        }
        o
    }
}

/// One model request of the run. If a hint (or a recording) takes the processor part-way through,
/// the request stops, waits, and is run again from its start; the run as a whole is never cancelled
/// by that, only by the user.
pub(crate) fn ask(a: &dyn Assistant, run: &SummaryRun, messages: &[ChatMessage], max_tokens: usize) -> Result<String> {
    loop {
        if let Some(g) = &run.gate {
            let notify = |r: Option<PauseReason>| {
                if let Some(f) = &run.on_pause {
                    f(r)
                }
            };
            if !g.wait_for_turn(run.cpu_bound, &run.cancel, notify) {
                return Err(Error::Cancelled);
            }
        }
        let c = a.complete(messages, &run.opts(max_tokens), &mut |_| true)?;
        if c.stats.preempted {
            continue; // stopped to make room; wait for the way to clear and redo this request
        }
        if c.stats.cancelled {
            return Err(Error::Cancelled);
        }
        return Ok(strip_thinking(&c.text).trim().to_string());
    }
}

/// Summarize a meeting. `on_progress(done, total)` is called after every step (`total` is
/// the number of steps expected; it can grow by a merge step on very long meetings).
/// Returns `Error::Cancelled` if `run.cancel` is set.
pub fn summarize(
    a: &dyn Assistant,
    segments: &[TranscriptSegment],
    agenda: &str,
    run: &SummaryRun,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Summary> {
    let lines = transcript_lines(segments);
    if lines.is_empty() {
        return Err(Error::Other("Nothing was said, so there is nothing to summarize".into()));
    }
    let labelled = has_others(segments);
    let parts = split_lines(&lines, CHUNK_CHARS);

    let raw = if parts.len() == 1 {
        on_progress(0, 1);
        let r = ask(a, run, &final_messages(&parts[0], false, agenda, labelled), SUMMARY_TOKENS)?;
        on_progress(1, 1);
        r
    } else {
        let mut total = parts.len() + 1;
        on_progress(0, total);
        let mut done = 0;
        let mut notes: Vec<String> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            notes.push(ask(a, run, &notes_messages(part, i, parts.len(), labelled), NOTES_TOKENS)?);
            done += 1;
            on_progress(done, total);
        }
        // Many notes: merge neighbours until they fit in one final step.
        while notes.iter().map(|n| n.len() + 1).sum::<usize>() > NOTES_BUDGET_CHARS && notes.len() > 1 {
            let mut merged: Vec<String> = Vec::new();
            let mut group = String::new();
            let mut flush = |group: &mut String, merged: &mut Vec<String>, done: &mut usize, total: &mut usize| -> Result<()> {
                if !group.is_empty() {
                    merged.push(ask(a, run, &merge_messages(group), NOTES_TOKENS)?);
                    group.clear();
                    *done += 1;
                    *total += 1;
                    on_progress(*done, *total);
                }
                Ok(())
            };
            for n in &notes {
                if !group.is_empty() && group.len() + n.len() + 1 > NOTES_BUDGET_CHARS {
                    flush(&mut group, &mut merged, &mut done, &mut total)?;
                }
                group.push_str(n);
                group.push('\n');
            }
            flush(&mut group, &mut merged, &mut done, &mut total)?;
            if merged.len() >= notes.len() {
                break; // not shrinking: stop rather than loop forever
            }
            notes = merged;
        }
        let r = ask(a, run, &final_messages(&notes.join("\n"), true, agenda, labelled), SUMMARY_TOKENS)?;
        on_progress(total, total);
        r
    };

    Summary::parse(&raw).ok_or_else(|| Error::Other("The model did not return usable meeting notes. Try again.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::{Completion, GenStats};
    use crate::Source;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    /// Answers by looking at the prompt, and remembers every prompt it was given.
    struct Fake {
        calls: Mutex<Vec<Vec<ChatMessage>>>,
        reply: Box<dyn Fn(&[ChatMessage]) -> String + Send + Sync>,
        cancel_after: Option<usize>,
    }

    impl Fake {
        fn new(reply: impl Fn(&[ChatMessage]) -> String + Send + Sync + 'static) -> Self {
            Fake { calls: Mutex::new(vec![]), reply: Box::new(reply), cancel_after: None }
        }
        fn calls(&self) -> Vec<Vec<ChatMessage>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Assistant for Fake {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn complete(&self, messages: &[ChatMessage], opts: &GenOptions, _on_piece: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
            let n = {
                let mut c = self.calls.lock().unwrap();
                c.push(messages.to_vec());
                c.len()
            };
            if Some(n) == self.cancel_after {
                opts.cancel.store(true, Ordering::Relaxed);
            }
            let cancelled = opts.cancel.load(Ordering::Relaxed);
            Ok(Completion { text: (self.reply)(messages), stats: GenStats { cancelled, ..Default::default() } })
        }
    }

    const GOOD: &str = "SUMMARY: The team agreed to ship on Friday.\nDECISIONS:\n- Ship on Friday\nACTION ITEMS:\n- Sam writes the release notes by Thursday";

    fn run() -> SummaryRun {
        SummaryRun::plain(Arc::new(AtomicBool::new(false)))
    }

    fn seg(ms: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment::new(ms, ms + 1000, text)
    }

    fn long_transcript(lines: usize) -> Vec<TranscriptSegment> {
        (0..lines).map(|i| seg(i as i64 * 5_000, &format!("This is sentence number {i} and it is padded with a lot of words so the transcript becomes long enough to need several steps."))).collect()
    }

    #[test]
    fn parses_the_requested_format() {
        let s = Summary::parse(GOOD).unwrap();
        assert_eq!(s.overview, "The team agreed to ship on Friday.");
        assert_eq!(s.decisions, ["Ship on Friday"]);
        assert_eq!(s.action_items, ["Sam writes the release notes by Thursday"]);
    }

    #[test]
    fn parses_markdown_numbering_and_none() {
        let raw = "**SUMMARY:** A short chat.\n\n**DECISIONS**\n1. Use Rust\n2) Skip the demo\n\n## Action items:\n* None\n";
        let s = Summary::parse(raw).unwrap();
        assert_eq!(s.overview, "A short chat.");
        assert_eq!(s.decisions, ["Use Rust", "Skip the demo"]);
        assert!(s.action_items.is_empty(), "\"None\" is not an item");
    }

    #[test]
    fn text_without_headings_becomes_the_summary_and_thinking_is_removed() {
        let s = Summary::parse("<|channel>thought\nhmm<channel|>They talked about lunch.").unwrap();
        assert_eq!(s.overview, "They talked about lunch.");
        assert!(Summary::parse("   \n ").is_none());
        assert!(Summary::parse("SUMMARY:\nDECISIONS:\n- None\nACTION ITEMS:\n- None").is_none(), "all empty is unusable");
    }

    #[test]
    fn a_sentence_that_starts_with_a_heading_word_is_not_a_heading() {
        let s = Summary::parse("Summarizes the budget talk.\nDECISIONS:\n- Actions were postponed").unwrap();
        assert_eq!(s.overview, "Summarizes the budget talk.");
        assert_eq!(s.decisions, ["Actions were postponed"]);
    }

    #[test]
    fn markdown_has_only_the_sections_that_have_content() {
        let s = Summary { overview: "Ok.".into(), decisions: vec![], action_items: vec!["Do it".into()] };
        assert_eq!(s.to_markdown(), "## Summary\nOk.\n\n## Action items\n- Do it");
        let round = Summary::parse(&Summary::parse(GOOD).unwrap().to_markdown()).unwrap();
        assert_eq!(round, Summary::parse(GOOD).unwrap(), "the stored markdown reads back the same");
    }

    #[test]
    fn split_keeps_whole_lines_and_oversized_lines_alone() {
        let lines: Vec<String> = ["aaaa", "bbbb", "cccc", &"d".repeat(30)].iter().map(|s| s.to_string()).collect();
        let parts = split_lines(&lines, 10);
        assert_eq!(parts, ["aaaa\nbbbb", "cccc", &"d".repeat(30)]);
        assert!(split_lines(&[], 10).is_empty());
    }

    #[test]
    fn a_short_meeting_takes_one_step_with_the_agenda() {
        let fake = Fake::new(|_| GOOD.to_string());
        let mut progress = vec![];
        let s = summarize(&fake, &[seg(0, "Can we ship Friday?"), seg(4_000, "Yes.")], "Decide the launch date", &run(), |d, t| progress.push((d, t))).unwrap();
        assert_eq!(s.decisions, ["Ship on Friday"]);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        let user = &calls[0][1].content;
        assert!(user.contains("Decide the launch date") && user.contains("[0:00] Can we ship Friday?") && user.contains("[0:04] Yes."));
        assert_eq!(progress, [(0, 1), (1, 1)]);
    }

    #[test]
    fn a_long_meeting_is_noted_part_by_part_and_the_summary_is_written_from_the_notes() {
        let segs = long_transcript(200);
        let expect_parts = split_lines(&transcript_lines(&segs), CHUNK_CHARS).len();
        assert!(expect_parts >= 3, "test input must need several parts, got {expect_parts}");
        let fake = Fake::new(|m| if m[0].content.contains("one part of a longer meeting") { "- a note".into() } else { GOOD.to_string() });
        let mut progress = vec![];
        let s = summarize(&fake, &segs, "", &run(), |d, t| progress.push((d, t))).unwrap();
        assert_eq!(s.overview, "The team agreed to ship on Friday.");
        let calls = fake.calls();
        assert_eq!(calls.len(), expect_parts + 1, "one note step per part, then the summary");
        // Every transcript line went to exactly one note step, in order.
        assert!(calls[0][1].content.contains("Part 1 of"));
        let last = &calls.last().unwrap()[1].content;
        assert!(last.contains("Notes from the meeting") && last.contains("- a note") && !last.contains("sentence number"), "the summary sees notes, not the transcript");
        assert!(calls.iter().all(|c| c.iter().map(|m| m.content.len()).sum::<usize>() < CHUNK_CHARS + 2_500), "every prompt fits the context window");
        assert!(progress.windows(2).all(|w| w[0].0 <= w[1].0) && progress.last().unwrap().0 == progress.last().unwrap().1);
    }

    #[test]
    fn very_long_notes_are_merged_before_the_final_step() {
        let segs = long_transcript(400);
        // Each part answers with a large blob of notes, so the combined notes exceed the budget.
        let big = format!("- {}", "x".repeat(2_500));
        let fake = Fake::new(move |m| {
            if m[0].content.contains("one part of a longer meeting") {
                big.clone()
            } else if m[0].content.contains("merge notes") {
                "- merged".into()
            } else {
                GOOD.to_string()
            }
        });
        let s = summarize(&fake, &segs, "", &run(), |_, _| {}).unwrap();
        assert!(!s.is_empty());
        let calls = fake.calls();
        assert!(calls.iter().any(|c| c[0].content.contains("merge notes")), "a merge step ran");
        let last = &calls.last().unwrap()[1].content;
        assert!(last.len() < NOTES_BUDGET_CHARS + 600, "the final prompt stays inside the budget: {}", last.len());
    }

    #[test]
    fn both_sides_of_a_call_are_labelled_and_the_model_is_told_what_that_means() {
        let segs = [seg(0, "Hello"), TranscriptSegment::new(2_000, 3_000, "Hi, can you ship Friday?").from_source(Source::System)];
        let fake = Fake::new(|_| GOOD.to_string());
        summarize(&fake, &segs, "", &run(), |_, _| {}).unwrap();
        let c = &fake.calls()[0];
        assert!(c[1].content.contains("[0:02] Others: Hi, can you ship Friday?") && c[1].content.contains("[0:00] You: Hello"));
        assert!(c[0].content.contains("\"Others:\""));
    }

    #[test]
    fn cancelling_stops_the_run() {
        let segs = long_transcript(200);
        let mut fake = Fake::new(|_| "- note".into());
        fake.cancel_after = Some(1);
        let r = summarize(&fake, &segs, "", &run(), |_, _| {});
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(fake.calls().len(), 1, "no further steps after the cancel");
    }

    #[test]
    fn an_empty_transcript_and_an_unusable_reply_are_errors() {
        let fake = Fake::new(|_| GOOD.to_string());
        assert!(summarize(&fake, &[], "", &run(), |_, _| {}).is_err());
        assert!(summarize(&fake, &[seg(0, "   ")], "", &run(), |_, _| {}).is_err());
        let blank = Fake::new(|_| "   ".to_string());
        assert!(summarize(&blank, &[seg(0, "Hi")], "", &run(), |_, _| {}).is_err());
        assert_eq!(fake.calls().len(), 0, "nothing is sent to the model for an empty transcript");
    }

    /// A fake model whose requests honour `GenOptions::preempt` like the real one: while the check says
    /// stop, a request returns at once, marked preempted, with no text.
    struct Polite {
        inner: Fake,
        preempted_calls: Mutex<usize>,
    }
    impl Assistant for Polite {
        fn model_id(&self) -> &str {
            "polite"
        }
        fn complete(&self, m: &[ChatMessage], o: &GenOptions, p: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
            if o.should_stop() {
                *self.preempted_calls.lock().unwrap() += 1;
                return Ok(Completion { text: String::new(), stats: GenStats { cancelled: true, preempted: !o.cancel.load(Ordering::Relaxed), ..Default::default() } });
            }
            self.inner.complete(m, o, p)
        }
    }

    fn gated(gate: &Arc<ModelGate>, cpu_bound: bool, events: Arc<Mutex<Vec<Option<PauseReason>>>>) -> SummaryRun {
        SummaryRun {
            cancel: Arc::new(AtomicBool::new(false)),
            yield_to: None,
            gate: Some(gate.clone()),
            cpu_bound,
            on_pause: Some(Arc::new(move |r| events.lock().unwrap().push(r))),
        }
    }

    #[test]
    fn notes_wait_for_a_hint_and_then_finish_instead_of_being_cancelled() {
        let gate = ModelGate::new();
        let events = Arc::new(Mutex::new(vec![]));
        let hint = gate.begin_interactive(); // a hint is being written when the notes start
        let model = Arc::new(Polite { inner: Fake::new(|_| GOOD.to_string()), preempted_calls: Mutex::new(0) });
        let run = gated(&gate, false, events.clone());
        let m2 = model.clone();
        let t = std::thread::spawn(move || summarize(m2.as_ref(), &[seg(0, "Can we ship Friday?")], "", &run, |_, _| {}));
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!t.is_finished(), "waiting while the hint runs");
        assert_eq!(model.inner.calls().len(), 0, "no model request was made yet");
        drop(hint);
        let s = t.join().unwrap().unwrap();
        assert_eq!(s.decisions, ["Ship on Friday"], "the notes complete once the hint is done");
        assert_eq!(*events.lock().unwrap(), vec![Some(PauseReason::Hint), None], "the UI hears about the pause and the resume");
    }

    /// Stops its first request as if a hint had just taken the processor, then behaves.
    struct StopsOnce {
        inner: Fake,
        stopped: Mutex<bool>,
    }
    impl Assistant for StopsOnce {
        fn model_id(&self) -> &str {
            "stops-once"
        }
        fn complete(&self, m: &[ChatMessage], o: &GenOptions, p: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
            let mut done = self.stopped.lock().unwrap();
            if !*done {
                *done = true;
                return Ok(Completion { text: "half a sent".into(), stats: GenStats { cancelled: true, preempted: true, ..Default::default() } });
            }
            drop(done);
            self.inner.complete(m, o, p)
        }
    }

    #[test]
    fn a_request_stopped_halfway_by_a_hint_is_redone_not_lost() {
        let model = StopsOnce { inner: Fake::new(|_| GOOD.to_string()), stopped: Mutex::new(false) };
        let gate = ModelGate::new();
        let s = summarize(&model, &[seg(0, "Can we ship Friday?")], "", &gated(&gate, false, Arc::new(Mutex::new(vec![]))), |_, _| {}).unwrap();
        assert_eq!(s.decisions, ["Ship on Friday"], "the partial text was thrown away and the request run again");
        assert_eq!(model.inner.calls().len(), 1, "exactly one complete request reached the end");
        // Without the stop being the user's, nothing reports a cancel.
        let user_cancel = StopsOnce { inner: Fake::new(|_| GOOD.to_string()), stopped: Mutex::new(true) };
        let run = SummaryRun::plain(Arc::new(AtomicBool::new(true)));
        assert!(matches!(summarize(&user_cancel, &[seg(0, "Hi there")], "", &run, |_, _| {}), Err(Error::Cancelled)));
    }

    #[test]
    fn a_recording_pauses_notes_only_on_a_processor_bound_model() {
        let gate = ModelGate::new();
        gate.set_recording(true);
        // GPU notes model: runs straight through the recording.
        let events = Arc::new(Mutex::new(vec![]));
        let model = Polite { inner: Fake::new(|_| GOOD.to_string()), preempted_calls: Mutex::new(0) };
        let r = summarize(&model, &[seg(0, "Hello there everyone")], "", &gated(&gate, false, events.clone()), |_, _| {});
        assert!(r.is_ok());
        assert!(events.lock().unwrap().is_empty(), "a GPU model never waits for a recording");
        // CPU notes model: waits until the recording stops.
        let events = Arc::new(Mutex::new(vec![]));
        let run = gated(&gate, true, events.clone());
        let g2 = gate.clone();
        let t = std::thread::spawn(move || summarize(&Polite { inner: Fake::new(|_| GOOD.to_string()), preempted_calls: Mutex::new(0) }, &[seg(0, "Hello there everyone")], "", &run, |_, _| {}));
        std::thread::sleep(std::time::Duration::from_millis(120));
        assert!(!t.is_finished(), "paused while recording");
        g2.set_recording(false);
        assert!(t.join().unwrap().is_ok(), "resumes by itself after the recording");
        assert_eq!(*events.lock().unwrap(), vec![Some(PauseReason::Recording), None]);
    }

    #[test]
    fn cancelling_while_paused_ends_the_run_promptly() {
        let gate = ModelGate::new();
        let _hint = gate.begin_interactive();
        let run = gated(&gate, false, Arc::new(Mutex::new(vec![])));
        let cancel = run.cancel.clone();
        let t = std::thread::spawn(move || summarize(&Fake::new(|_| GOOD.to_string()), &[seg(0, "Hello there")], "", &run, |_, _| {}));
        std::thread::sleep(std::time::Duration::from_millis(80));
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(t.join().unwrap(), Err(Error::Cancelled)));
    }
}
