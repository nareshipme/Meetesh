//! When to offer a hint without being asked. Pure logic, driven by the live transcript and a
//! clock the caller supplies, so every rule is unit-tested without a model or a microphone.
//!
//! Two triggers, both opt-in:
//! - **Question**: the other participants asked something, and the user has not started to
//!   answer. Needs the system-audio track (it is what tells the user from the others).
//! - **Pause**: the conversation went quiet for a while after a stretch of real talk.
//!
//! A cooldown keeps automatic hints from piling up.

use serde::{Deserialize, Serialize};

use crate::{Source, TranscriptSegment};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoHintMode {
    #[default]
    Off,
    /// A hint when the other participants ask a question.
    Questions,
    /// Questions, and when the conversation pauses after a stretch of talk.
    QuestionsAndPauses,
}

impl AutoHintMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoHintMode::Off => "off",
            AutoHintMode::Questions => "questions",
            AutoHintMode::QuestionsAndPauses => "questions_and_pauses",
        }
    }

    pub fn parse(s: &str) -> AutoHintMode {
        match s {
            "questions" => AutoHintMode::Questions,
            "questions_and_pauses" => AutoHintMode::QuestionsAndPauses,
            _ => AutoHintMode::Off,
        }
    }
}

/// Why an automatic hint is being offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Question,
    Pause,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Question => "question",
            Trigger::Pause => "pause",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TriggerConfig {
    /// Minimum time between two automatic hints.
    pub cooldown_ms: u64,
    /// How long to wait after a question for the speaker to finish (and to see if the user answers).
    pub settle_ms: u64,
    /// Silence that counts as a pause.
    pub pause_ms: u64,
    /// A pause only counts after at least this many words since the last hint.
    pub min_words_for_pause: usize,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        TriggerConfig { cooldown_ms: 45_000, settle_ms: 1_500, pause_ms: 12_000, min_words_for_pause: 30 }
    }
}

/// Does this look like a question? Whisper usually punctuates questions, so a trailing "?" is the
/// main signal; a sentence that opens with a question phrase and has no full stop also counts.
pub fn is_question(text: &str) -> bool {
    let t = text.trim().trim_end_matches(|c: char| matches!(c, '"' | '\'' | ')' | '\u{201D}')).trim();
    if t.is_empty() {
        return false;
    }
    if t.ends_with('?') {
        return t.split_whitespace().count() >= 2;
    }
    if t.ends_with('.') || t.ends_with('!') {
        return false;
    }
    let lower = t.to_lowercase();
    const OPENERS: [&str; 17] = [
        "can you", "could you", "would you", "will you", "do you", "did you", "are you", "is there",
        "what", "when", "where", "who", "why", "how", "which", "have you", "should we",
    ];
    t.split_whitespace().count() >= 4 && OPENERS.iter().any(|o| lower.starts_with(o) && lower[o.len()..].starts_with(|c: char| c.is_whitespace() || c == ','))
}

pub struct TriggerDetector {
    cfg: TriggerConfig,
    mode: AutoHintMode,
    /// The other participants are on the system-audio track. Without it, "who asked" is unknown.
    has_others: bool,
    last_fire_ms: Option<u64>,
    last_speech_ms: Option<u64>,
    words_since_hint: usize,
    /// A question was asked; fire at this time unless the user starts answering.
    pending_question_at: Option<u64>,
    pause_handled: bool,
}

impl TriggerDetector {
    pub fn new(cfg: TriggerConfig, mode: AutoHintMode, has_others: bool) -> Self {
        TriggerDetector {
            cfg,
            mode,
            has_others,
            last_fire_ms: None,
            last_speech_ms: None,
            words_since_hint: 0,
            pending_question_at: None,
            pause_handled: false,
        }
    }

    pub fn set_mode(&mut self, mode: AutoHintMode) {
        self.mode = mode;
        if mode == AutoHintMode::Off {
            self.pending_question_at = None;
        }
    }

    pub fn mode(&self) -> AutoHintMode {
        self.mode
    }

    /// New transcript segments arrived at `now_ms`.
    pub fn on_segments(&mut self, segments: &[TranscriptSegment], now_ms: u64) {
        for s in segments {
            let words = s.text.split_whitespace().count();
            if words == 0 {
                continue;
            }
            self.last_speech_ms = Some(now_ms);
            self.words_since_hint += words;
            self.pause_handled = false;
            match s.source {
                // The user started talking: a question to them is being answered.
                Source::Mic => self.pending_question_at = None,
                Source::System => {
                    if is_question(&s.text) {
                        self.pending_question_at = Some(now_ms + self.cfg.settle_ms);
                    }
                }
            }
        }
    }

    /// Call regularly. Returns a trigger when a hint should be offered now.
    pub fn poll(&mut self, now_ms: u64) -> Option<Trigger> {
        if self.mode == AutoHintMode::Off {
            return None;
        }
        if self.last_fire_ms.is_some_and(|t| now_ms < t + self.cfg.cooldown_ms) {
            return None;
        }
        if self.has_others {
            if let Some(at) = self.pending_question_at {
                if now_ms >= at {
                    return Some(self.fire(now_ms, Trigger::Question));
                }
            }
        }
        if self.mode == AutoHintMode::QuestionsAndPauses
            && !self.pause_handled
            && self.words_since_hint >= self.cfg.min_words_for_pause
            && self.last_speech_ms.is_some_and(|t| now_ms >= t + self.cfg.pause_ms)
        {
            return Some(self.fire(now_ms, Trigger::Pause));
        }
        None
    }

    /// A hint could not be started (the model is busy or not ready): try again later, and do not
    /// count it against the cooldown.
    pub fn not_fired(&mut self) {
        self.last_fire_ms = None;
    }

    fn fire(&mut self, now_ms: u64, t: Trigger) -> Trigger {
        self.last_fire_ms = Some(now_ms);
        self.pending_question_at = None;
        self.words_since_hint = 0;
        self.pause_handled = true;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn others(text: &str) -> TranscriptSegment {
        TranscriptSegment::new(0, 1000, text).from_source(Source::System)
    }
    fn me(text: &str) -> TranscriptSegment {
        TranscriptSegment::new(0, 1000, text)
    }
    fn det(mode: AutoHintMode, has_others: bool) -> TriggerDetector {
        TriggerDetector::new(TriggerConfig::default(), mode, has_others)
    }
    const TALK: &str = "we went through the whole roadmap and talked about the budget, the hiring plan, and the launch dates for next quarter, and then we spent a while on the risks for the first release and who would own each of them";

    #[test]
    fn recognizes_questions() {
        for q in ["Can you ship it on Friday?", "What do you think", "how are we doing on the budget", "Is that right?", "\"Did that work?\"", "Could you share the numbers, please"] {
            assert!(is_question(q), "{q}");
        }
        for s in ["I can ship it on Friday.", "What I mean is that it works.", "Thanks everyone!", "", "Yes", "so", "Right?", "How are you doing today."] {
            assert!(!is_question(s), "{s}");
        }
        assert!(!is_question("?"), "a lone mark is not a question");
        assert!(TALK.split_whitespace().count() >= 30, "test text must be enough talk for a pause");
        assert!(!is_question("Whatever works for you"), "a word that merely starts with 'what' is not an opener");
    }

    #[test]
    fn a_question_from_the_others_fires_after_the_settle_time() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[others("Can you ship it on Friday?")], 10_000);
        assert_eq!(d.poll(10_500), None, "waits for the speaker to finish");
        assert_eq!(d.poll(11_600), Some(Trigger::Question));
        assert_eq!(d.poll(11_700), None, "fires once");
    }

    #[test]
    fn answering_cancels_the_question() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[others("Can you ship it on Friday?")], 10_000);
        d.on_segments(&[me("Yes, I think so.")], 10_800);
        assert_eq!(d.poll(12_000), None, "the user is already answering");
    }

    #[test]
    fn questions_from_the_microphone_never_trigger_and_neither_does_a_mic_only_meeting() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[me("Can you ship it on Friday?")], 10_000);
        assert_eq!(d.poll(20_000), None, "the user's own question is not a reason to hint");
        let mut mic_only = det(AutoHintMode::Questions, false);
        mic_only.on_segments(&[others("Can you ship it on Friday?")], 10_000);
        assert_eq!(mic_only.poll(20_000), None, "without the system track nobody can tell who asked");
    }

    #[test]
    fn off_never_fires() {
        let mut d = det(AutoHintMode::Off, true);
        d.on_segments(&[others("Can you ship it?"), others(TALK)], 1_000);
        assert_eq!(d.poll(100_000), None);
    }

    #[test]
    fn cooldown_spaces_hints_and_a_held_back_question_fires_afterwards() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[others("Can you ship it on Friday?")], 0);
        assert_eq!(d.poll(2_000), Some(Trigger::Question));
        d.on_segments(&[others("And who owns the release?")], 10_000);
        assert_eq!(d.poll(12_000), None, "inside the 45 s cooldown");
        assert_eq!(d.poll(47_100), Some(Trigger::Question), "the question was kept and is offered when the cooldown ends");
    }

    #[test]
    fn a_hint_that_could_not_start_does_not_use_up_the_cooldown() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[others("Can you ship it on Friday?")], 0);
        assert_eq!(d.poll(2_000), Some(Trigger::Question));
        d.not_fired();
        d.on_segments(&[others("Sorry, can you ship it on Friday?")], 3_000);
        assert_eq!(d.poll(4_600), Some(Trigger::Question));
    }

    #[test]
    fn a_pause_after_real_talk_fires_once_and_only_in_the_pauses_mode() {
        for mode in [AutoHintMode::Questions, AutoHintMode::QuestionsAndPauses] {
            let mut d = det(mode, false);
            d.on_segments(&[me(TALK)], 5_000);
            assert_eq!(d.poll(10_000), None, "not quiet for long enough yet");
            let expect = if mode == AutoHintMode::QuestionsAndPauses { Some(Trigger::Pause) } else { None };
            assert_eq!(d.poll(17_100), expect);
            assert_eq!(d.poll(30_000), None, "one hint per pause");
        }
    }

    #[test]
    fn a_pause_needs_enough_talk_and_resumed_talk_resets_it() {
        let mut d = det(AutoHintMode::QuestionsAndPauses, false);
        d.on_segments(&[me("Okay, sounds good.")], 5_000);
        assert_eq!(d.poll(40_000), None, "a few words are not a conversation");
        d.on_segments(&[me(TALK)], 41_000);
        d.on_segments(&[me("one more thing")], 50_000);
        assert_eq!(d.poll(55_000), None, "talk resumed, the pause clock restarted");
        assert_eq!(d.poll(62_100), Some(Trigger::Pause));
        // New talk after the hint can earn another pause hint, after the cooldown.
        d.on_segments(&[me(TALK)], 70_000);
        assert_eq!(d.poll(83_000), None, "cooldown (45 s) not over");
        assert_eq!(d.poll(108_000), Some(Trigger::Pause));
    }

    #[test]
    fn switching_off_drops_a_pending_question() {
        let mut d = det(AutoHintMode::Questions, true);
        d.on_segments(&[others("Can you ship it on Friday?")], 0);
        d.set_mode(AutoHintMode::Off);
        d.set_mode(AutoHintMode::Questions);
        assert_eq!(d.poll(5_000), None);
    }

    #[test]
    fn mode_names_roundtrip_and_unknown_means_off() {
        for m in [AutoHintMode::Off, AutoHintMode::Questions, AutoHintMode::QuestionsAndPauses] {
            assert_eq!(AutoHintMode::parse(m.as_str()), m);
        }
        assert_eq!(AutoHintMode::parse("garbage"), AutoHintMode::Off);
    }
}
