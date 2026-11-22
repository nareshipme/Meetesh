//! Full-text search across meetings: titles, notes and transcript lines.
//!
//! SQLite FTS5 does the matching (see migration 10). This module turns what the user typed into a
//! safe query, runs it against both indexes and groups the hits per meeting.

use std::collections::HashMap;

use rusqlite::params;
use serde::Serialize;

use super::Db;
use crate::Result;

/// Marks the start and end of a matched word in a snippet (control characters never occur in
/// real text, so the UI can split on them safely).
pub const HIT_START: char = '\u{1}';
pub const HIT_END: char = '\u{2}';

const MAX_MEETINGS: usize = 40;
const MAX_MATCHES_PER_MEETING: usize = 3;
const SNIPPET_WORDS: i64 = 14;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MatchKind {
    Title,
    Notes,
    Transcript,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Match {
    pub kind: MatchKind,
    /// Text around the match, with `HIT_START` / `HIT_END` around the matched words.
    pub snippet: String,
    /// For transcript lines: when it was said.
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResult {
    pub meeting_id: i64,
    pub title: String,
    pub created_at: i64,
    pub matches: Vec<Match>,
}

/// What the user typed as an FTS5 query: every word must appear (in any order), and each word
/// also matches as a prefix ("quarter road" finds "quarterly roadmap"), so results follow the
/// typing. `None` if there is nothing to search for.
pub fn fts_query(input: &str) -> Option<String> {
    let words: Vec<&str> = input.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    // Quoted, so FTS5 operators typed by the user (AND, NEAR, -, *) are plain words.
    Some(words.iter().map(|w| format!("\"{w}\"*")).collect::<Vec<_>>().join(" "))
}

impl Db {
    /// Meetings that match `query`, best first, each with up to three matching places.
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let Some(q) = fts_query(query) else { return Ok(vec![]) };
        // meeting id -> (best rank, matches). bm25 ranks are negative; lower is better.
        let mut by_meeting: HashMap<i64, (f64, Vec<(f64, Match)>)> = HashMap::new();
        let mut add = |id: i64, rank: f64, m: Match| {
            let e = by_meeting.entry(id).or_insert((f64::MAX, vec![]));
            e.0 = e.0.min(rank);
            e.1.push((rank, m));
        };

        // Titles and notes. A title match is worth showing even when the notes also match.
        let mut stmt = self.conn.prepare(
            "SELECT rowid, bm25(fts_meetings), snippet(fts_meetings, 0, char(1), char(2), '…', ?2),
                    snippet(fts_meetings, 1, char(1), char(2), '…', ?2),
                    instr(snippet(fts_meetings, 0, char(1), char(2), '…', ?2), char(1)) > 0,
                    instr(snippet(fts_meetings, 1, char(1), char(2), '…', ?2), char(1)) > 0
               FROM fts_meetings WHERE fts_meetings MATCH ?1 ORDER BY bm25(fts_meetings) LIMIT 200",
        )?;
        let rows = stmt.query_map(params![q, SNIPPET_WORDS], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?, r.get::<_, bool>(5)?))
        })?;
        for row in rows {
            let (id, rank, title_snip, notes_snip, in_title, in_notes) = row?;
            // Titles weigh more than a line of the transcript: nudge their rank down.
            if in_title {
                add(id, rank - 5.0, Match { kind: MatchKind::Title, snippet: title_snip, start_ms: None });
            }
            if in_notes {
                add(id, rank - 1.0, Match { kind: MatchKind::Notes, snippet: notes_snip, start_ms: None });
            }
        }

        // Transcript lines.
        let mut stmt = self.conn.prepare(
            "SELECT s.meeting_id, s.start_ms, bm25(fts_segments), snippet(fts_segments, 0, char(1), char(2), '…', ?2)
               FROM fts_segments JOIN transcript_segments s ON s.id = fts_segments.rowid
              WHERE fts_segments MATCH ?1 ORDER BY bm25(fts_segments) LIMIT 400",
        )?;
        let rows = stmt.query_map(params![q, SNIPPET_WORDS], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, String>(3)?))
        })?;
        for row in rows {
            let (id, start_ms, rank, snip) = row?;
            add(id, rank, Match { kind: MatchKind::Transcript, snippet: snip, start_ms: Some(start_ms) });
        }

        let mut out = Vec::new();
        for (id, (best, mut matches)) in by_meeting {
            let Ok(m) = self.get_meeting(id) else { continue };
            matches.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut chosen: Vec<Match> = matches.into_iter().map(|(_, m)| m).collect();
            // Show where it was found in a stable, readable order: title, notes, then the transcript in time order.
            chosen.truncate(MAX_MATCHES_PER_MEETING);
            chosen.sort_by_key(|m| (m.kind as u8, m.start_ms.unwrap_or(0)));
            out.push((best, SearchResult { meeting_id: id, title: m.title, created_at: m.created_at, matches: chosen }));
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.created_at.cmp(&a.1.created_at)));
        Ok(out.into_iter().map(|(_, r)| r).take(MAX_MEETINGS).collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::asr::{Source, TranscriptSegment};
    use crate::db::search::{fts_query, MatchKind, HIT_END, HIT_START};
    use crate::Db;

    fn seg(ms: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment::new(ms, ms + 1000, text)
    }

    fn marked(s: &str) -> Vec<String> {
        // the words between the hit markers
        let mut out = vec![];
        let mut cur = None::<String>;
        for c in s.chars() {
            match c {
                HIT_START => cur = Some(String::new()),
                HIT_END => out.extend(cur.take()),
                c => {
                    if let Some(w) = cur.as_mut() {
                        w.push(c)
                    }
                }
            }
        }
        out
    }

    #[test]
    fn user_text_becomes_a_safe_query() {
        assert_eq!(fts_query("budget plan").as_deref(), Some("\"budget\"* \"plan\"*"));
        assert_eq!(fts_query("  Q3,  review!  ").as_deref(), Some("\"Q3\"* \"review\"*"));
        assert_eq!(fts_query("AND OR NOT NEAR(").as_deref(), Some("\"AND\"* \"OR\"* \"NOT\"* \"NEAR\"*"), "operators are only words");
        assert_eq!(fts_query("naïve").as_deref(), Some("\"naïve\"*"));
        assert_eq!(fts_query("  ...  "), None);
        assert_eq!(fts_query(""), None);
        // quotes and wildcards typed by the user cannot break out of the quoting
        assert_eq!(fts_query("say \"hi\" *").as_deref(), Some("\"say\"* \"hi\"*"));
    }

    #[test]
    fn finds_titles_notes_and_transcript_lines_with_highlights() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_meeting(Some("Budget review")).unwrap();
        let b = db.create_meeting(Some("Standup")).unwrap();
        let c = db.create_meeting(Some("Hiring sync")).unwrap();
        db.replace_transcript(b.id, "m", &[seg(0, "We should check the budget before Friday."), seg(5_000, "Lunch was great.")]).unwrap();
        db.set_summary(c.id, "## Summary\nThe team discussed the hiring budget.", "m").unwrap();

        let r = db.search("budget").unwrap();
        let ids: Vec<i64> = r.iter().map(|x| x.meeting_id).collect();
        assert_eq!(ids.len(), 3, "{r:?}");
        assert_eq!(ids[0], a.id, "a title match ranks first");
        let by = |id: i64| r.iter().find(|x| x.meeting_id == id).unwrap();
        assert_eq!(by(a.id).matches[0].kind, MatchKind::Title);
        assert_eq!(marked(&by(a.id).matches[0].snippet), ["Budget"]);
        let line = &by(b.id).matches[0];
        assert_eq!((line.kind, line.start_ms), (MatchKind::Transcript, Some(0)));
        assert!(line.snippet.contains("check the"), "{}", line.snippet);
        assert_eq!(by(c.id).matches[0].kind, MatchKind::Notes);
    }

    #[test]
    fn matches_every_word_as_a_prefix_ignoring_case_and_accents() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("Café planning")).unwrap();
        db.replace_transcript(m.id, "m", &[seg(0, "The quarterly roadmap is ready")]).unwrap();
        assert_eq!(db.search("QUARTER road").unwrap().len(), 1, "every word as a prefix, any case");
        assert_eq!(db.search("cafe").unwrap().len(), 1, "accents are ignored");
        assert!(db.search("quarterly budget").unwrap().is_empty(), "every word must match");
        assert!(db.search("   ").unwrap().is_empty());
        assert!(db.search("zzzz").unwrap().is_empty());
    }

    #[test]
    fn the_index_follows_edits_replacements_and_deletes() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("Alpha")).unwrap();
        db.append_transcript_segment(m.id, "m", &seg(0, "pineapple pizza")).unwrap();
        assert_eq!(db.search("pineapple").unwrap().len(), 1, "live segments are searchable as they arrive");
        // re-transcription replaces the lines: the old words are gone, the new ones found
        db.replace_transcript(m.id, "m", &[seg(0, "mushroom risotto")]).unwrap();
        assert!(db.search("pineapple").unwrap().is_empty());
        assert_eq!(db.search("mushroom").unwrap().len(), 1);
        // renaming and notes
        db.rename_meeting(m.id, "Omega").unwrap();
        assert!(db.search("alpha").unwrap().is_empty());
        assert_eq!(db.search("omega").unwrap().len(), 1);
        db.set_summary(m.id, "## Summary\nZeppelin trip", "m").unwrap();
        assert_eq!(db.search("zeppelin").unwrap().len(), 1);
        db.set_summary(m.id, "", "").unwrap();
        assert!(db.search("zeppelin").unwrap().is_empty());
        // deleting the meeting (its lines go with it by cascade) removes it from the index
        db.delete_meeting(m.id).unwrap();
        assert!(db.search("mushroom").unwrap().is_empty());
        assert!(db.search("omega").unwrap().is_empty());
    }

    #[test]
    fn keeps_three_places_per_meeting_and_labels_both_sides() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("Long call")).unwrap();
        let lines: Vec<_> = (0..8).map(|i| seg(i * 4_000, &format!("release number {i} is on track"))).collect();
        db.replace_transcript(m.id, "m", &lines).unwrap();
        let r = db.search("release").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].matches.len(), 3);
        let times: Vec<_> = r[0].matches.iter().filter_map(|x| x.start_ms).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "transcript matches are shown in time order: {times:?}");
        let _ = Source::Mic;
    }

    #[test]
    fn upgrading_indexes_what_was_already_saved() {
        use crate::db::MIGRATIONS;
        use rusqlite::Connection;
        let path = std::env::temp_dir().join(format!("meetesh-v9-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..9] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 9u32).unwrap();
            conn.execute("INSERT INTO meetings (title, created_at, summary) VALUES ('Old meeting', 1, 'Talked about tulips')", []).unwrap();
            conn.execute("INSERT INTO transcript_segments (meeting_id, start_ms, end_ms, text) VALUES (1, 0, 10, 'daffodils everywhere')", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.search("tulips").unwrap().len(), 1, "notes written before the index existed");
        assert_eq!(db.search("daffodils").unwrap().len(), 1, "so are old transcript lines");
        assert_eq!(db.search("old").unwrap().len(), 1);
        drop(db);
        std::fs::remove_file(path).ok();
    }

    /// Rough size check, run by hand: `cargo test -p meetesh-core --lib search_scale -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_scale() {
        use crate::db::MIGRATIONS;
        use rusqlite::Connection;
        use std::time::Instant;
        let path = std::env::temp_dir().join(format!("meetesh-scale-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let words = ["budget", "release", "customer", "roadmap", "hiring", "latency", "quarter", "invoice", "design", "support", "pricing", "launch", "contract", "review", "metrics", "partner"];
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..9] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 9u32).unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            let mut n = 7u64;
            for m in 0..500 {
                tx.execute("INSERT INTO meetings (title, created_at, summary) VALUES (?1, ?2, 'notes')", rusqlite::params![format!("Meeting {m}"), m]).unwrap();
                for l in 0..400 {
                    let text: Vec<&str> = (0..14).map(|_| { n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); words[(n >> 33) as usize % words.len()] }).collect();
                    tx.execute("INSERT INTO transcript_segments (meeting_id, start_ms, end_ms, text) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![m + 1, l * 3000, l * 3000 + 2000, text.join(" ")]).unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let t = Instant::now();
        let db = Db::open(&path).unwrap(); // runs migration 10: builds the index
        eprintln!("index built for 200,000 lines (500 meetings) in {:.2}s", t.elapsed().as_secs_f32());
        let t = Instant::now();
        let r = db.search("budget release").unwrap();
        eprintln!("search 'budget release': {} meetings in {:.0} ms", r.len(), t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        db.append_transcript_segment(1, "m", &seg(0, "one more live line")).unwrap();
        eprintln!("one live insert with the index: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
        drop(db);
        std::fs::remove_file(path).ok();
    }
}
