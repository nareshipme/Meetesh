//! Builds the model input for a hint. Pure functions, no model needed, so prompt behavior is
//! unit-tested and easy to iterate on.

use crate::assist::{ChatMessage, ChatRole};
use crate::{Source, TranscriptSegment};

/// Returned by the model when it has nothing worth saying; the UI shows a neutral message.
pub const NO_HINT: &str = "NO_HINT";

const SYSTEM_PROMPT: &str = "You are a quiet assistant sitting beside the user during a live meeting. \
You see the most recent part of the meeting transcript. The transcript has no speaker names and may contain \
recognition mistakes. Give the user ONE brief, useful hint they can act on right now: a suggested reply if \
someone seems to have asked them something, a clarifying question worth asking, or a relevant point to raise. \
Write at most two short sentences, in plain text, with no preamble, no quotes, no bullet points and no markdown. \
Only use what is in the transcript and agenda; never invent facts. If nothing useful can be said, reply exactly: NO_HINT";

/// Everything the hint is based on.
#[derive(Debug, Clone, Default)]
pub struct HintContext {
    /// Optional goals or agenda the user typed before the meeting.
    pub agenda: String,
    /// Recent transcript, oldest first.
    pub recent: Vec<TranscriptSegment>,
}

/// Limits that keep the prompt inside the model's context window. Roughly 4 characters per token.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_transcript_chars: usize,
    pub max_agenda_chars: usize,
}

impl Default for Budget {
    fn default() -> Self {
        // About 1,500 tokens of transcript plus 400 of agenda: leaves room for the reply in a 4k context.
        Budget { max_transcript_chars: 6_000, max_agenda_chars: 1_600 }
    }
}

/// Used when the other participants were recorded too: lines say who spoke.
pub(crate) const SPEAKER_NOTE: &str = "Lines starting with \"You:\" are the user; lines starting with \"Others:\" are the other \
participants on the call (not told apart) and the transcript may contain recognition mistakes.";

/// True if the transcript has both sides of a call, so lines should be labelled.
pub(crate) fn has_others(segments: &[TranscriptSegment]) -> bool {
    segments.iter().any(|s| s.source == Source::System)
}

fn stamp(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Segments from the last `window_ms` of the meeting (measured back from the newest segment).
pub fn recent_window(segments: &[TranscriptSegment], window_ms: i64) -> Vec<TranscriptSegment> {
    let newest = segments.iter().map(|s| s.end_ms).max().unwrap_or(0);
    segments.iter().filter(|s| s.end_ms >= newest - window_ms).cloned().collect()
}

/// Every non-empty segment as a `[m:ss] text` line (labelled like `transcript_tail`), oldest first.
pub fn transcript_lines(segments: &[TranscriptSegment]) -> Vec<String> {
    let labelled = has_others(segments);
    segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| line_for(s, labelled))
        .collect()
}

fn line_for(seg: &TranscriptSegment, labelled: bool) -> String {
    let who = match (labelled, seg.source) {
        (false, _) => "",
        (true, Source::Mic) => "You: ",
        (true, Source::System) => "Others: ",
    };
    format!("[{}] {}{}", stamp(seg.start_ms), who, seg.text.trim())
}

/// The newest segments that fit in `max_chars`, formatted as `[m:ss] text` lines, oldest first
/// (`[m:ss] You: text` / `[m:ss] Others: text` when the other participants were recorded too).
/// Whole segments only, so a sentence is never cut in half.
pub fn transcript_tail(segments: &[TranscriptSegment], max_chars: usize) -> String {
    let labelled = has_others(segments);
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    for seg in segments.iter().rev() {
        if seg.text.trim().is_empty() {
            continue;
        }
        let line = line_for(seg, labelled);
        if used + line.len() + 1 > max_chars && !lines.is_empty() {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();
    lines.join("\n")
}

pub(crate) fn truncate_chars(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub fn build_hint_messages(ctx: &HintContext, budget: Budget) -> Vec<ChatMessage> {
    let mut user = String::new();
    let agenda = ctx.agenda.trim();
    if !agenda.is_empty() {
        user.push_str("Meeting goals / agenda:\n");
        user.push_str(truncate_chars(agenda, budget.max_agenda_chars));
        user.push_str("\n\n");
    }
    user.push_str("Transcript so far (most recent last):\n");
    user.push_str(&transcript_tail(&ctx.recent, budget.max_transcript_chars));
    user.push_str("\n\nWhat should I say or ask next?");
    let system = if has_others(&ctx.recent) {
        SYSTEM_PROMPT.replace("The transcript has no speaker names and may contain recognition mistakes.", SPEAKER_NOTE)
    } else {
        SYSTEM_PROMPT.to_string()
    };
    vec![ChatMessage::new(ChatRole::System, system), ChatMessage::new(ChatRole::User, user)]
}

/// Remove reasoning blocks some models emit (`<|channel>thought ... <channel|>`), keeping only the answer.
pub(crate) fn strip_thinking(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<|channel>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("<channel|>") {
            Some(end) => rest = &rest[start + end + "<channel|>".len()..],
            None => return out, // unfinished thought: nothing usable after it
        }
    }
    out.push_str(rest);
    out
}

/// Model output cleaned for display. `None` means "no hint" (the model declined, or said nothing).
pub fn clean_hint(raw: &str) -> Option<String> {
    let without_thoughts = strip_thinking(raw);
    let mut t = without_thoughts.trim();
    // Some models add a label or wrap the reply in quotes despite instructions.
    for prefix in ["Hint:", "hint:", "**Hint:**", "Suggestion:", "Suggested reply:"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim_start();
        }
    }
    let t = t.trim_matches(|c| c == '"' || c == '\u{201C}' || c == '\u{201D}').trim();
    if t.is_empty() || t.to_uppercase().contains(NO_HINT) {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_ms: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment::new(start_ms, start_ms + 1000, text)
    }

    #[test]
    fn tail_formats_timestamps_and_keeps_order() {
        let t = transcript_tail(&[seg(2_000, "Hello."), seg(65_000, "Second thing.")], 1_000);
        assert_eq!(t, "[0:02] Hello.\n[1:05] Second thing.");
    }

    #[test]
    fn tail_keeps_newest_whole_segments_within_budget() {
        let segs: Vec<_> = (0..10).map(|i| seg(i * 1000, &format!("sentence number {i}"))).collect();
        let t = transcript_tail(&segs, 80);
        let lines: Vec<&str> = t.lines().collect();
        assert!(t.len() <= 80, "{}", t.len());
        assert!(lines.last().unwrap().ends_with("sentence number 9"), "newest is kept: {t}");
        assert!(!t.contains("sentence number 0"), "oldest is dropped");
        assert!(lines.iter().all(|l| l.starts_with('[') && l.contains("sentence number")), "no half lines");
    }

    #[test]
    fn tail_always_includes_the_latest_segment_even_if_it_alone_exceeds_the_budget() {
        let t = transcript_tail(&[seg(0, &"word ".repeat(100))], 20);
        assert!(t.contains("word word"));
    }

    #[test]
    fn recent_window_keeps_only_the_last_minutes() {
        let segs = vec![seg(0, "old"), seg(100_000, "middle"), seg(200_000, "new")];
        let w = recent_window(&segs, 120_000);
        assert_eq!(w.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["middle", "new"]);
        assert_eq!(recent_window(&segs, 10_000_000).len(), 3);
        assert!(recent_window(&[], 1000).is_empty());
    }

    #[test]
    fn tail_skips_blank_segments() {
        assert_eq!(transcript_tail(&[seg(0, "  "), seg(1000, "Hi")], 100), "[0:01] Hi");
        assert_eq!(transcript_tail(&[], 100), "");
    }

    #[test]
    fn messages_have_system_then_user_and_include_agenda_only_when_present() {
        let ctx = HintContext { agenda: "  Decide the launch date  ".into(), recent: vec![seg(0, "When can we launch?")] };
        let m = build_hint_messages(&ctx, Budget::default());
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].role, ChatRole::System);
        assert_eq!(m[1].role, ChatRole::User);
        assert!(m[1].content.contains("Meeting goals / agenda:\nDecide the launch date\n"));
        assert!(m[1].content.contains("[0:00] When can we launch?"));
        assert!(m[1].content.ends_with("What should I say or ask next?"));

        let no_agenda = build_hint_messages(&HintContext { recent: vec![seg(0, "Hi")], ..Default::default() }, Budget::default());
        assert!(!no_agenda[1].content.contains("agenda"));
    }

    #[test]
    fn long_agenda_is_truncated_on_a_char_boundary() {
        let ctx = HintContext { agenda: "é".repeat(5_000), recent: vec![seg(0, "x")] };
        let m = build_hint_messages(&ctx, Budget { max_transcript_chars: 100, max_agenda_chars: 101 });
        assert!(m[1].content.len() < 400);
    }

    #[test]
    fn clean_hint_strips_labels_and_quotes_and_detects_no_hint() {
        assert_eq!(clean_hint("  Hint: \"Ask about the budget.\"  ").as_deref(), Some("Ask about the budget."));
        assert_eq!(clean_hint("Say you can send it Friday.").as_deref(), Some("Say you can send it Friday."));
        assert_eq!(clean_hint("NO_HINT"), None);
        assert_eq!(clean_hint("no_hint"), None);
        assert_eq!(clean_hint("   "), None);
    }

    #[test]
    fn clean_hint_removes_reasoning_blocks() {
        assert_eq!(clean_hint("<|channel>thought\nthe user asked...<channel|>Say yes.").as_deref(), Some("Say yes."));
        assert_eq!(clean_hint("<|channel>thought\nstill thinking"), None);
    }

    #[test]
    fn lines_are_labelled_only_when_both_sides_of_a_call_were_recorded() {
        let me = |t, s: &str| TranscriptSegment::new(t, t + 500, s);
        let them = |t, s: &str| TranscriptSegment::new(t, t + 500, s).from_source(Source::System);
        let both = [them(0, "Can you ship Friday?"), me(2_000, "Probably.")];
        assert_eq!(transcript_tail(&both, 500), "[0:00] Others: Can you ship Friday?\n[0:02] You: Probably.");
        // Mic only: exactly as before, so notes-only and microphone-only meetings are unchanged.
        assert_eq!(transcript_tail(&[me(0, "Hello")], 500), "[0:00] Hello");

        let ctx = HintContext { agenda: String::new(), recent: both.to_vec() };
        let m = build_hint_messages(&ctx, Budget::default());
        assert!(m[0].content.contains("\"Others:\""), "the model is told what the labels mean");
        assert!(!m[0].content.contains("no speaker names"));
        let mic_only = build_hint_messages(&HintContext { agenda: String::new(), recent: vec![me(0, "Hi")] }, Budget::default());
        assert!(mic_only[0].content.contains("no speaker names"));
    }
}
