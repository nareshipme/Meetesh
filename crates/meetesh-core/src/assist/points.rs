//! Live points: a short, self-updating list of what is being discussed in the meeting right now.
//!
//! Every minute or so, while recording, the lines said since the last update and the current list are
//! handed to the model, which returns an updated list (what is being discussed now, plus the points,
//! decisions and open questions so far). Updates are incremental, so the prompt stays small however
//! long the meeting gets. Prompt, parsing and scheduling are pure and tested with a fake model.

use crate::assist::prompt::{has_others, strip_thinking, transcript_lines, SPEAKER_NOTE};
use crate::assist::summary::{ask, SummaryRun};
use crate::assist::{Assistant, ChatMessage, ChatRole};
use crate::{Error, Result, TranscriptSegment};

/// What the list can hold.
pub const MAX_POINTS: usize = 6;
const MAX_POINT_CHARS: usize = 140;
/// New transcript handed to one update. About 900 tokens; a busier minute keeps its newest lines.
pub const MAX_NEW_CHARS: usize = 3_500;
const POINTS_TOKENS: usize = 220;

const INSTRUCTIONS: &str = "You keep a live list of what is being discussed in a meeting, for someone who is also taking part. \
You get the current list and the new lines of the transcript. The transcript may contain recognition mistakes. \
Update the list using only what was said: never invent anything. Reply in exactly this format and nothing else:\n\
NOW: <what is being discussed right now, in one short line>\n\
- <a point, decision or open question so far, short>\n\
Keep at most six bullets. Keep earlier bullets that still matter, merge repeats, drop what is settled and no longer matters, \
and add what is new. Write a bullet for a decision as \"Decided: ...\" and for an open question as \"Open: ...\".";

/// The list as it stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Points {
    /// One line: what is being discussed right now.
    pub now: String,
    pub points: Vec<String>,
}

impl Points {
    pub fn is_empty(&self) -> bool {
        self.now.is_empty() && self.points.is_empty()
    }

    fn render(&self) -> String {
        if self.is_empty() {
            return "(nothing yet)".to_string();
        }
        let mut s = format!("NOW: {}\n", if self.now.is_empty() { "(unclear)" } else { &self.now });
        for p in &self.points {
            s.push_str(&format!("- {p}\n"));
        }
        s.trim_end().to_string()
    }

    /// Read the model's reply. Tolerant of markdown and numbering; at most `MAX_POINTS` bullets, each
    /// cut to a sensible length. `None` if there is nothing usable.
    pub fn parse(raw: &str) -> Option<Points> {
        let cleaned = strip_thinking(raw);
        let mut p = Points::default();
        for line in cleaned.lines() {
            let t = line.trim().trim_start_matches(['#', '*', '_']).trim();
            if t.is_empty() {
                continue;
            }
            let lower = t.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("now") {
                if rest.trim_start_matches(['*', '_', ' ']).starts_with(':') {
                    let text = t[3..].trim_start_matches(['*', '_', ' ']).trim_start_matches(':').trim_start_matches(['*', '_', ' ']);
                    p.now = clip(text.trim(), MAX_POINT_CHARS);
                    continue;
                }
            }
            let item = t.trim_start_matches(['-', '*', '•', '–']).trim();
            let item = match item.split_once(['.', ')']) {
                Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest.trim(),
                _ => item,
            };
            if item.is_empty() || matches!(item.to_ascii_lowercase().trim_end_matches('.'), "none" | "nothing yet" | "n/a") {
                continue;
            }
            if t.starts_with(['-', '*', '•', '–']) || item.len() != t.len() || !p.now.is_empty() {
                if p.points.len() < MAX_POINTS {
                    p.points.push(clip(item, MAX_POINT_CHARS));
                }
            } else if p.now.is_empty() {
                p.now = clip(item, MAX_POINT_CHARS); // a reply without the "NOW:" label: its first line
            }
        }
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) if i > max / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

/// The newest transcript lines that fit in `MAX_NEW_CHARS`, oldest first.
fn new_lines(segments: &[TranscriptSegment]) -> String {
    let lines = transcript_lines(segments);
    let mut kept: Vec<&String> = Vec::new();
    let mut used = 0;
    for l in lines.iter().rev() {
        if used + l.len() + 1 > MAX_NEW_CHARS && !kept.is_empty() {
            break;
        }
        used += l.len() + 1;
        kept.push(l);
    }
    kept.reverse();
    kept.into_iter().map(String::as_str).collect::<Vec<_>>().join("\n")
}

pub fn points_messages(current: &Points, new_segments: &[TranscriptSegment]) -> Vec<ChatMessage> {
    let system = if has_others(new_segments) { format!("{INSTRUCTIONS}\n{SPEAKER_NOTE}") } else { INSTRUCTIONS.to_string() };
    let user = format!("Current list:\n{}\n\nNew lines:\n{}\n\nWrite the updated list.", current.render(), new_lines(new_segments));
    vec![ChatMessage::new(ChatRole::System, system), ChatMessage::new(ChatRole::User, user)]
}

/// Update the list from the lines said since the last update. Runs under the same gate rules as the
/// other model work (`run.gate`): it steps aside for a hint and is redone afterwards. `Err(Cancelled)`
/// if `run.cancel` is set.
pub fn update_points(a: &dyn Assistant, current: &Points, new_segments: &[TranscriptSegment], run: &SummaryRun) -> Result<Points> {
    if transcript_lines(new_segments).is_empty() {
        return Ok(current.clone());
    }
    let raw = ask(a, run, &points_messages(current, new_segments), POINTS_TOKENS)?;
    Points::parse(&raw).ok_or_else(|| Error::Other("The model did not return usable points".into()))
}

/// When to update. Pure: the caller supplies the clock.
///
/// An update needs enough new talk and a pause since the last one; the pause grows on a slow machine
/// (a multiple of how long the last update took), so on a CPU the list refreshes less often instead of
/// taking the processor all the time.
#[derive(Debug, Clone)]
pub struct Scheduler {
    pub min_words: usize,
    pub min_gap_ms: u64,
    /// Idle time to leave after an update, as a multiple of how long it took.
    pub rest_factor: u64,
    last_end_ms: Option<u64>,
    last_took_ms: u64,
    running: bool,
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler { min_words: 40, min_gap_ms: 60_000, rest_factor: 2, last_end_ms: None, last_took_ms: 0, running: false }
    }
}

impl Scheduler {
    /// Should an update start now? `new_words`: words said since the last update.
    pub fn should_start(&self, now_ms: u64, new_words: usize) -> bool {
        if self.running || new_words < self.min_words {
            return false;
        }
        match self.last_end_ms {
            None => true, // the first one as soon as there is enough to say
            Some(end) => now_ms >= end + self.min_gap_ms.max(self.last_took_ms * self.rest_factor),
        }
    }

    pub fn started(&mut self) {
        self.running = true;
    }

    /// The update ended (successfully or not), `took_ms` after it started.
    pub fn finished(&mut self, now_ms: u64, took_ms: u64) {
        self.running = false;
        self.last_end_ms = Some(now_ms);
        self.last_took_ms = took_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::gate::ModelGate;
    use crate::assist::{Completion, GenOptions, GenStats};
    use crate::Source;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    struct Say(&'static str, Mutex<Vec<Vec<ChatMessage>>>);
    impl Say {
        fn new(t: &'static str) -> Self {
            Say(t, Mutex::new(vec![]))
        }
    }
    impl Assistant for Say {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn complete(&self, m: &[ChatMessage], _o: &GenOptions, _p: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
            self.1.lock().unwrap().push(m.to_vec());
            Ok(Completion { text: self.0.to_string(), stats: GenStats::default() })
        }
    }

    fn seg(ms: i64, t: &str) -> TranscriptSegment {
        TranscriptSegment::new(ms, ms + 1000, t)
    }
    fn run() -> SummaryRun {
        SummaryRun::plain(Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn parses_the_requested_format_and_variants() {
        let p = Points::parse("NOW: Choosing the launch date\n- Decided: ship Friday\n- Open: who tells support?\n- Android testing is Thursday").unwrap();
        assert_eq!(p.now, "Choosing the launch date");
        assert_eq!(p.points, ["Decided: ship Friday", "Open: who tells support?", "Android testing is Thursday"]);
        let q = Points::parse("**NOW:** Budget\n1. First\n2) Second\n* Third").unwrap();
        assert_eq!((q.now.as_str(), q.points.len()), ("Budget", 3));
        assert_eq!(q.points[1], "Second");
        // no label: the first plain line is the topic
        let r = Points::parse("Talking about hiring\n- Two open roles").unwrap();
        assert_eq!((r.now.as_str(), r.points.as_slice()), ("Talking about hiring", &["Two open roles".to_string()][..]));
        assert!(Points::parse("  \n").is_none());
        assert!(Points::parse("<|channel>thought\nhmm<channel|>NOW: Pricing").unwrap().now == "Pricing");
        assert!(Points::parse("NOW: \n- None").is_none(), "nothing usable");
    }

    #[test]
    fn the_list_is_bounded() {
        let many: String = (0..12).map(|i| format!("- point {i}\n")).collect();
        let p = Points::parse(&format!("NOW: x\n{many}")).unwrap();
        assert_eq!(p.points.len(), MAX_POINTS);
        let long = format!("NOW: {}", "word ".repeat(100));
        assert!(Points::parse(&long).unwrap().now.chars().count() <= MAX_POINT_CHARS + 1);
    }

    #[test]
    fn the_prompt_has_the_current_list_and_only_the_newest_lines() {
        let cur = Points { now: "Budget".into(), points: vec!["Cap is 40k".into()] };
        let m = points_messages(&cur, &[seg(0, "Hello"), seg(65_000, "Now the launch date")]);
        assert!(m[1].content.contains("NOW: Budget") && m[1].content.contains("- Cap is 40k") && m[1].content.contains("[1:05] Now the launch date"));
        assert!(points_messages(&Points::default(), &[seg(0, "Hi")])[1].content.contains("(nothing yet)"));
        let huge: Vec<TranscriptSegment> = (0..400).map(|i| seg(i * 1000, &format!("line number {i} with some words in it to make it long enough"))).collect();
        let m = points_messages(&Points::default(), &huge);
        assert!(m[1].content.len() < MAX_NEW_CHARS + 1_500, "bounded: {}", m[1].content.len());
        assert!(m[1].content.contains("line number 399") && !m[1].content.contains("line number 0 "), "the newest lines are kept");
        // both sides of a call: labelled, and the model is told what the labels mean
        let call = [seg(0, "Hi").from_source(Source::System)];
        let m = points_messages(&Points::default(), &call);
        assert!(m[1].content.contains("Others:") && m[0].content.contains("\"Others:\""));
    }

    #[test]
    fn an_update_returns_the_parsed_list_and_does_nothing_for_silence() {
        let model = Say::new("NOW: Launch\n- Decided: Friday");
        let p = update_points(&model, &Points::default(), &[seg(0, "We launch Friday")], &run()).unwrap();
        assert_eq!(p.now, "Launch");
        let before = Points { now: "Old".into(), points: vec![] };
        let same = update_points(&model, &before, &[seg(0, "   ")], &run()).unwrap();
        assert_eq!(same, before);
        assert_eq!(model.1.lock().unwrap().len(), 1, "no model request for blank lines");
        assert!(update_points(&Say::new("   "), &Points::default(), &[seg(0, "Hi there")], &run()).is_err());
    }

    #[test]
    fn an_update_waits_for_a_hint_and_still_completes() {
        let gate = ModelGate::new();
        let hint = gate.begin_interactive();
        let r = SummaryRun { gate: Some(gate.clone()), cpu_bound: false, ..run() };
        let t = std::thread::spawn(move || update_points(&Say::new("NOW: Launch"), &Points::default(), &[seg(0, "We launch Friday")], &r));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!t.is_finished());
        drop(hint);
        assert_eq!(t.join().unwrap().unwrap().now, "Launch");
        // and a recording never holds live points back (they are for the recording)
        gate.set_recording(true);
        let r = SummaryRun { gate: Some(gate.clone()), cpu_bound: false, ..run() };
        assert!(update_points(&Say::new("NOW: Launch"), &Points::default(), &[seg(0, "We launch Friday")], &r).is_ok());
    }

    #[test]
    fn the_scheduler_needs_enough_talk_and_a_rest_that_grows_on_a_slow_machine() {
        let mut s = Scheduler::default();
        assert!(!s.should_start(5_000, 10), "not enough said yet");
        assert!(s.should_start(65_000, 45), "the first update as soon as there is enough");
        s.started();
        assert!(!s.should_start(70_000, 200), "one at a time");
        s.finished(80_000, 10_000); // took 10 s: rest max(60 s, 20 s)
        assert!(!s.should_start(100_000, 100) && s.should_start(140_001, 100), "60 s gap after a quick update");
        s.started();
        s.finished(200_000, 50_000); // a slow machine: rest 100 s
        assert!(!s.should_start(299_000, 100), "rests twice as long as the update took");
        assert!(s.should_start(300_001, 100));
        assert!(!s.should_start(400_000, 3), "and still needs new talk");
    }
}
