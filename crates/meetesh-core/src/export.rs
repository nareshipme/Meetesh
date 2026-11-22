//! Markdown export of a meeting: title, when, goals, notes and the transcript. Pure, so the format
//! is tested without a database or files.

use crate::{Meeting, Source, TranscriptSegment};

/// `m:ss`, or `h:mm:ss` from one hour on.
pub fn clock(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The meeting as a Markdown document. `when` is the start time already formatted in local time.
pub fn markdown(meeting: &Meeting, segments: &[TranscriptSegment], when: &str) -> String {
    let mut out = format!("# {}\n\n", meeting.title.trim());

    let mut facts = vec![when.to_string()];
    if meeting.duration_ms > 0 {
        facts.push(clock(meeting.duration_ms));
    }
    if meeting.has_system_audio {
        facts.push("microphone and the other participants".to_string());
    }
    out.push_str(&format!("*{}*\n\n", facts.join(" · ")));

    let agenda = meeting.agenda.trim();
    if !agenda.is_empty() {
        out.push_str(&format!("## Goals\n{agenda}\n\n"));
    }
    let notes = meeting.summary.trim();
    if !notes.is_empty() {
        out.push_str(notes);
        out.push_str("\n\n");
    }

    let lines: Vec<&TranscriptSegment> = segments.iter().filter(|s| !s.text.trim().is_empty()).collect();
    if !lines.is_empty() {
        out.push_str("## Transcript\n\n");
        let labelled = meeting.has_system_audio || lines.iter().any(|s| s.source == Source::System);
        for s in lines {
            let who = match (labelled, s.source) {
                (false, _) => "",
                (true, Source::Mic) => " You:",
                (true, Source::System) => " Others:",
            };
            out.push_str(&format!("**[{}]{}** {}\n\n", clock(s.start_ms), who, s.text.trim()));
        }
    }
    out.trim_end().to_string() + "\n"
}

/// A file name for the export: `<id>-<title words>.md`, safe on every OS.
pub fn file_name(meeting: &Meeting) -> String {
    let mut slug = String::new();
    for c in meeting.title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("meeting-{}.md", meeting.id)
    } else {
        format!("{}-{}.md", meeting.id, slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    fn meeting(f: impl FnOnce(&mut Meeting)) -> Meeting {
        let db = Db::open_in_memory().unwrap();
        let mut m = db.create_meeting(Some("Weekly sync")).unwrap();
        f(&mut m);
        m
    }

    #[test]
    fn clock_switches_to_hours() {
        assert_eq!((clock(0), clock(65_000), clock(3_599_000), clock(3_600_000), clock(3_725_000)), ("0:00".into(), "1:05".into(), "59:59".into(), "1:00:00".into(), "1:02:05".into()));
        assert_eq!(clock(-5), "0:00");
    }

    #[test]
    fn a_full_meeting_reads_well() {
        let m = meeting(|m| {
            m.duration_ms = 125_000;
            m.agenda = "Decide the launch date".into();
            m.summary = "## Summary\nShip Friday.\n\n## Decisions\n- Ship Friday".into();
            m.has_system_audio = true;
        });
        let segs = [
            TranscriptSegment::new(2_000, 4_000, "Can you ship Friday?").from_source(Source::System),
            TranscriptSegment::new(5_000, 6_000, "  Yes.  "),
            TranscriptSegment::new(7_000, 8_000, "   "),
        ];
        let md = markdown(&m, &segs, "2026-11-22 09:30");
        assert_eq!(
            md,
            "# Weekly sync\n\n*2026-11-22 09:30 · 2:05 · microphone and the other participants*\n\n\
             ## Goals\nDecide the launch date\n\n\
             ## Summary\nShip Friday.\n\n## Decisions\n- Ship Friday\n\n\
             ## Transcript\n\n**[0:02] Others:** Can you ship Friday?\n\n**[0:05] You:** Yes.\n"
        );
    }

    #[test]
    fn a_bare_meeting_has_only_what_it_has() {
        let md = markdown(&meeting(|_| {}), &[], "2026-11-22 09:30");
        assert_eq!(md, "# Weekly sync\n\n*2026-11-22 09:30*\n");
        // Microphone only: no labels.
        let md = markdown(&meeting(|_| {}), &[TranscriptSegment::new(1_000, 2_000, "Hello")], "now");
        assert!(md.ends_with("## Transcript\n\n**[0:01]** Hello\n"), "{md}");
    }

    #[test]
    fn file_names_are_safe_and_stable() {
        let m = |t: &str, id: i64| Meeting { title: t.into(), id, ..meeting(|_| {}) };
        assert_eq!(file_name(&m("Weekly sync", 7)), "7-weekly-sync.md");
        assert_eq!(file_name(&m("  Q3 / budget: review?!  ", 12)), "12-q3-budget-review.md");
        assert_eq!(file_name(&m("日本語の会議", 3)), "meeting-3.md", "nothing ASCII left: fall back to the id");
        assert!(file_name(&m(&"a".repeat(200), 1)).len() < 50);
        assert!(!file_name(&m("../../etc/passwd", 1)).contains(".."));
    }
}
