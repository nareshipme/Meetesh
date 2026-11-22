//! A short title for a meeting, written from its finished notes by the on-device model.

use crate::assist::prompt::{strip_thinking, truncate_chars};
use crate::assist::{Assistant, ChatMessage, ChatRole, GenOptions};
use crate::Result;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const MAX_TITLE_CHARS: usize = 60;
const MAX_WORDS: usize = 9;
const NOTES_CHARS: usize = 2_400;

const INSTRUCTIONS: &str = "You name meetings. Given the notes of a meeting, reply with a title of three to seven words \
that says what it was about, like \"Onboarding launch date\" or \"Q3 budget review\". Reply with the title only: \
no quotes, no full stop, no \"Meeting\" prefix, no markdown.";

pub fn title_messages(notes_markdown: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::new(ChatRole::System, INSTRUCTIONS),
        ChatMessage::new(
            ChatRole::User,
            format!("Meeting notes:\n{}\n\nTitle:", truncate_chars(notes_markdown.trim(), NOTES_CHARS)),
        ),
    ]
}

/// The model's reply made into a title, or `None` if it is unusable (empty, a sentence, too long).
pub fn clean_title(raw: &str) -> Option<String> {
    let text = strip_thinking(raw);
    let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut t = first.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '#' | '_' | '\u{201C}' | '\u{201D}')).trim();
    for prefix in ["Title:", "title:", "TITLE:"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '\u{201C}' | '\u{201D}') || c.is_whitespace());
        }
    }
    let t = t.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '-')).trim();
    if t.is_empty() || t.split_whitespace().count() > MAX_WORDS || t.chars().count() > MAX_TITLE_CHARS {
        return None;
    }
    // Capitalize an all-lowercase first word ("onboarding plan"); leave "iOS", "eBay" and the rest alone.
    let first_word = t.split_whitespace().next()?;
    if first_word.chars().any(|c| c.is_uppercase()) {
        return Some(t.to_string());
    }
    let mut chars = t.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

/// Ask the model for a title. `Ok(None)` if it answered with something unusable.
pub fn generate_title(a: &dyn Assistant, notes_markdown: &str, cancel: Arc<AtomicBool>) -> Result<Option<String>> {
    let opts = GenOptions { max_tokens: 28, temperature: 0.2, ..GenOptions::hint(cancel) };
    let c = a.complete(&title_messages(notes_markdown), &opts, &mut |_| true)?;
    if c.stats.cancelled {
        return Err(crate::Error::Cancelled);
    }
    Ok(clean_title(&c.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::{Completion, GenStats};

    struct Say(&'static str);
    impl Assistant for Say {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn complete(&self, _m: &[ChatMessage], _o: &GenOptions, _p: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
            Ok(Completion { text: self.0.to_string(), stats: GenStats::default() })
        }
    }

    #[test]
    fn cleans_the_usual_decorations() {
        assert_eq!(clean_title("Onboarding launch date").as_deref(), Some("Onboarding launch date"));
        assert_eq!(clean_title("  \"Q3 budget review.\"  ").as_deref(), Some("Q3 budget review"));
        assert_eq!(clean_title("**Title:** onboarding launch plan").as_deref(), Some("Onboarding launch plan"));
        assert_eq!(clean_title("# Hiring sync\nSome explanation that should be ignored").as_deref(), Some("Hiring sync"));
        assert_eq!(clean_title("<|channel>thought\nhmm<channel|>API rate limits").as_deref(), Some("API rate limits"));
        assert_eq!(clean_title("iOS release checklist").as_deref(), Some("iOS release checklist"), "mixed case is the model's choice");
    }

    #[test]
    fn rejects_what_is_not_a_title() {
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("  \n "), None);
        assert_eq!(clean_title("\"\""), None);
        assert_eq!(clean_title("The team met on Friday to talk about the launch of the new onboarding flow and more"), None, "a sentence, not a title");
        assert_eq!(clean_title(&"x".repeat(80)), None);
    }

    #[test]
    fn the_prompt_carries_the_notes_and_is_bounded() {
        let m = title_messages(&format!("## Summary\n{}", "word ".repeat(2_000)));
        assert!(m[1].content.starts_with("Meeting notes:\n## Summary"));
        assert!(m[1].content.len() < NOTES_CHARS + 60);
    }

    #[test]
    fn generate_returns_the_cleaned_title_or_none() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(generate_title(&Say("\"Launch planning.\""), "notes", cancel.clone()).unwrap().as_deref(), Some("Launch planning"));
        assert_eq!(generate_title(&Say(""), "notes", cancel).unwrap(), None);
    }
}
