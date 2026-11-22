//! A memory for hints: what was said in earlier meetings about the same things.
//!
//! The words that matter in the last few minutes of the conversation (`keywords`) are looked up in
//! the search index (`Db::related`), and the best places found in other meetings are shown to the
//! model next to the transcript (`prompt`). Keyword matching, not meaning: it finds "the vendor
//! contract" when it is called the same thing, not when it is paraphrased. Everything stays on the device.

use std::collections::HashMap;

use crate::TranscriptSegment;

/// A place in an earlier meeting that may matter now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub meeting_id: i64,
    pub title: String,
    /// "Mon 16 Nov"
    pub day: String,
    /// What was said or written there, as plain text.
    pub excerpt: String,
}

const STOPWORDS: &[&str] = &[
    "about", "above", "after", "again", "against", "also", "always", "among", "another", "anyone", "anything", "around", "because",
    "been", "before", "being", "below", "between", "both", "bring", "call", "came", "cannot", "come", "could", "didn", "does",
    "doing", "done", "down", "during", "each", "either", "else", "enough", "even", "ever", "every", "everyone", "everything", "from",
    "gave", "going", "gone", "good", "great", "have", "having", "here", "hers", "himself", "however", "into", "just", "keep",
    "kind", "know", "last", "later", "least", "less", "like", "little", "look", "made", "make", "many", "maybe", "might", "more",
    "most", "much", "must", "myself", "need", "never", "next", "nobody", "nothing", "okay", "once", "only", "other", "ours",
    "ourselves", "over", "part", "people", "please", "probably", "quite", "really", "right", "said", "same", "say", "says", "see",
    "seem", "should", "since", "some", "someone", "something", "sometimes", "still", "such", "sure", "take", "talk", "tell",
    "than", "thank", "thanks", "that", "their", "theirs", "them", "themselves", "then", "there", "these", "they", "thing",
    "things", "think", "this", "those", "though", "through", "thus", "today", "together", "told", "took", "toward", "under",
    "until", "upon", "very", "want", "wants", "was", "well", "were", "what", "whatever", "when", "where", "whether", "which",
    "while", "whom", "whose", "will", "with", "within", "without", "would", "yeah", "yes", "yourself", "your", "yours",
    "meeting", "moment", "minute", "minutes", "time", "work", "working", "way", "actually", "basically", "kinda",
];

/// The `max` words that best say what the recent conversation is about: not common words, repeated,
/// longer, or capitalised in the middle of a sentence (names, products), most recent first on a tie.
pub fn keywords(recent: &[TranscriptSegment], max: usize) -> Vec<String> {
    #[derive(Default)]
    struct Stat {
        count: usize,
        proper: bool,
        last: usize,
    }
    let mut stats: HashMap<String, Stat> = HashMap::new();
    let mut order = 0usize;
    for seg in recent {
        let mut sentence_start = true;
        for raw in seg.text.split(|c: char| !c.is_alphanumeric() && c != '\'') {
            let word = raw.trim_matches('\'');
            if word.is_empty() {
                continue;
            }
            let lower = word.to_lowercase();
            let first_upper = word.chars().next().is_some_and(char::is_uppercase);
            let proper = first_upper && !sentence_start && !word.chars().all(char::is_uppercase);
            sentence_start = false;
            if lower.chars().count() < 4 || lower.chars().all(|c| c.is_ascii_digit()) || STOPWORDS.contains(&lower.as_str()) {
                continue;
            }
            order += 1;
            let s = stats.entry(lower).or_default();
            s.count += 1;
            s.proper |= proper;
            s.last = order;
        }
        // A sentence boundary inside a segment is not tracked; the segment start is a good enough signal.
    }
    let mut ranked: Vec<(String, usize, usize)> = stats
        .into_iter()
        .map(|(w, s)| {
            let score = s.count * 10 + w.chars().count().min(12) + if s.proper { 8 } else { 0 };
            (w, score, s.last)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    ranked.into_iter().take(max).map(|(w, _, _)| w).collect()
}

/// Search markers and markdown noise removed, whitespace collapsed, cut to `max_chars` at a word.
pub fn plain_excerpt(snippet: &str, max_chars: usize) -> String {
    let no_marks: String = snippet.chars().filter(|c| *c != '\u{1}' && *c != '\u{2}').collect();
    let no_md = no_marks.replace("## ", " ").replace("\n- ", " ").replace('\n', " ");
    let collapsed = no_md.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(max_chars).collect();
    match cut.rfind(' ') {
        Some(i) if i > max_chars / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Db, Source};

    fn seg(t: &str) -> TranscriptSegment {
        TranscriptSegment::new(0, 1000, t)
    }

    #[test]
    fn keywords_prefer_repeated_and_distinctive_words_over_chatter() {
        let k = keywords(
            &[
                seg("Okay, so about the vendor contract, can we really sign it this week?"),
                seg("The vendor wants sixty thousand for the campaign and Sam says that is too much."),
                seg("I think the contract needs legal review before anything."),
            ],
            4,
        );
        assert!(k[..2].contains(&"vendor".to_string()) && k[..2].contains(&"contract".to_string()), "the two repeated topic words lead: {k:?}");
        assert!(!k.iter().any(|w| ["okay", "about", "really", "think", "that", "this", "week"].contains(&w.as_str())), "no chatter: {k:?}");
        assert!(k.len() <= 4);
    }

    #[test]
    fn names_in_the_middle_of_a_sentence_count_as_distinctive() {
        let k = keywords(&[seg("We should ask Priya whether Kubernetes migration is worth it")], 3);
        assert!(k.contains(&"priya".to_string()) && k.contains(&"kubernetes".to_string()), "{k:?}");
    }

    #[test]
    fn small_talk_has_no_keywords() {
        assert!(keywords(&[seg("Yeah okay, thanks, see you, bye!")], 4).is_empty());
        assert!(keywords(&[], 4).is_empty());
    }

    #[test]
    fn excerpts_are_plain_and_short() {
        assert_eq!(plain_excerpt("\u{1}vendor\u{2} contract: cap at 40k", 100), "vendor contract: cap at 40k");
        assert_eq!(plain_excerpt("## Summary\nThe team set the cap.\n- Sam owns it", 100), "Summary The team set the cap. Sam owns it");
        let long = "word ".repeat(100);
        let e = plain_excerpt(&long, 30);
        assert!(e.chars().count() <= 31 && e.ends_with('…'), "{e}");
    }

    #[test]
    fn related_finds_other_meetings_about_the_same_things_and_never_the_current_one() {
        let db = Db::open_in_memory().unwrap();
        let old = db.create_meeting(Some("Budget review")).unwrap();
        db.set_summary(old.id, "## Decisions\n- The vendor contract is capped at 40k\n- Sam owns the vendor contract", "m").unwrap();
        let unrelated = db.create_meeting(Some("Lunch plans")).unwrap();
        db.replace_transcript(unrelated.id, "m", &[seg("We talked about pizza and the vendor next door")]).unwrap();
        let now = db.create_meeting(Some("Today")).unwrap();
        db.replace_transcript(now.id, "m", &[seg("Should we sign the vendor contract today?").from_source(Source::System)]).unwrap();

        let hits = db.related(now.id, &["vendor".into(), "contract".into(), "campaign".into()], 3).unwrap();
        // all three words match nowhere; the best two ("vendor contract") match the old meeting only
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].meeting_id, old.id);
        assert!(!hits.iter().any(|h| h.meeting_id == now.id), "the meeting in progress is never its own memory");
        // a single keyword is not enough to claim a relation
        assert!(db.related(now.id, &["vendor".into()], 3).unwrap().is_empty());
        assert!(db.related(now.id, &[], 3).unwrap().is_empty());
        assert!(db.day_label(old.created_at).unwrap().len() >= 8);
    }
}
