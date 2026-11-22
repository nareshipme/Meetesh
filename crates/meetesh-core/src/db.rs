//! SQLite storage with a tiny migration system.
//!
//! Schema version is tracked in `PRAGMA user_version`. Add new migrations to the
//! end of `MIGRATIONS`; never edit an existing entry.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::asr::{Source, TranscriptSegment};
use crate::error::{Error, Result};

/// SQL for the placeholder title of a meeting created at the unix time `$ts` (an SQL expression):
/// "Meeting on Sun 22 Nov, 3:45 PM" in local time (the year only when it is not this year), with
/// `$pre` before it and `$post` after it. A macro, because `concat!` needs literals and this text
/// is used both in a migration (column `created_at`) and when creating a meeting (parameter `?1`).
macro_rules! human_title_sql {
    ($pre:literal, $ts:literal, $post:literal) => {
        concat!(
            $pre,
            "'Meeting on ' || substr('SunMonTueWedThuFriSat', CAST(strftime('%w', ", $ts, ",'unixepoch','localtime') AS INTEGER) * 3 + 1, 3) || ' ' || ",
            "CAST(strftime('%d', ", $ts, ",'unixepoch','localtime') AS INTEGER) || ' ' || ",
            "substr('JanFebMarAprMayJunJulAugSepOctNovDec', (CAST(strftime('%m', ", $ts, ",'unixepoch','localtime') AS INTEGER) - 1) * 3 + 1, 3) || ",
            "CASE WHEN strftime('%Y', ", $ts, ",'unixepoch','localtime') = strftime('%Y', 'now', 'localtime') THEN '' ELSE ' ' || strftime('%Y', ", $ts, ",'unixepoch','localtime') END || ', ' || ",
            "((CAST(strftime('%H', ", $ts, ",'unixepoch','localtime') AS INTEGER) + 11) % 12 + 1) || ':' || strftime('%M', ", $ts, ",'unixepoch','localtime') || ' ' || ",
            "CASE WHEN CAST(strftime('%H', ", $ts, ",'unixepoch','localtime') AS INTEGER) >= 12 THEN 'PM' ELSE 'AM' END",
            $post
        )
    };
}

pub(crate) const MIGRATIONS: &[&str] = &[
    // 1: meetings
    "CREATE TABLE meetings (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        title      TEXT    NOT NULL,
        created_at INTEGER NOT NULL,
        transcript TEXT    NOT NULL DEFAULT '',
        summary    TEXT    NOT NULL DEFAULT ''
    );",
    // 2: recording state and audio location (Slice 1)
    "ALTER TABLE meetings ADD COLUMN status      TEXT    NOT NULL DEFAULT 'stopped';
     ALTER TABLE meetings ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE meetings ADD COLUMN audio_dir   TEXT    NOT NULL DEFAULT '';",
    // 3: timestamped transcript segments; remember which model produced them (Slice 2)
    "ALTER TABLE meetings DROP COLUMN transcript;
     ALTER TABLE meetings ADD COLUMN transcript_model TEXT NOT NULL DEFAULT '';
     CREATE TABLE transcript_segments (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        start_ms   INTEGER NOT NULL,
        end_ms     INTEGER NOT NULL,
        text       TEXT    NOT NULL
     );
     CREATE INDEX idx_segments_meeting ON transcript_segments(meeting_id, start_ms);",
    // 4: auto-generated titles were "Meeting <unix seconds>"; make them readable, in local time
    "UPDATE meetings
        SET title = 'Meeting ' || strftime('%Y-%m-%d %H:%M', created_at, 'unixepoch', 'localtime')
      WHERE length(title) = 18
        AND title GLOB 'Meeting [0-9]*'
        AND CAST(substr(title, 9) AS INTEGER) = created_at;",
    // 5: optional goals/agenda the user types before a meeting, used as context for live hints
    "ALTER TABLE meetings ADD COLUMN agenda TEXT NOT NULL DEFAULT '';",
    // 6: a second audio track (the other participants, from system audio) and which track a segment came from
    "ALTER TABLE meetings ADD COLUMN has_system_audio INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE transcript_segments ADD COLUMN source TEXT NOT NULL DEFAULT 'mic';",
    // 7: which model wrote the summary (the summary text itself has been a column since migration 1)
    "ALTER TABLE meetings ADD COLUMN summary_model TEXT NOT NULL DEFAULT '';",
    // 8: who chose the title: 'default' (the date placeholder), 'model' (written by the assistant)
    //    or 'user'. Only the first two may be replaced automatically.
    "ALTER TABLE meetings ADD COLUMN title_source TEXT NOT NULL DEFAULT 'default';
     UPDATE meetings SET title_source = 'user'
      WHERE NOT (title GLOB 'Meeting [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]');",
    // 9: small app settings that must survive restarts (key/value)
    "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // 10: full-text search over titles, notes and transcript lines (SQLite FTS5, external content so
    //     the text is stored once; triggers keep the indexes in step, including cascaded deletes)
    "CREATE VIRTUAL TABLE fts_meetings USING fts5(title, summary, content='meetings', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
     CREATE TRIGGER fts_meetings_ai AFTER INSERT ON meetings BEGIN
        INSERT INTO fts_meetings(rowid, title, summary) VALUES (new.id, new.title, new.summary);
     END;
     CREATE TRIGGER fts_meetings_ad AFTER DELETE ON meetings BEGIN
        INSERT INTO fts_meetings(fts_meetings, rowid, title, summary) VALUES ('delete', old.id, old.title, old.summary);
     END;
     CREATE TRIGGER fts_meetings_au AFTER UPDATE OF title, summary ON meetings BEGIN
        INSERT INTO fts_meetings(fts_meetings, rowid, title, summary) VALUES ('delete', old.id, old.title, old.summary);
        INSERT INTO fts_meetings(rowid, title, summary) VALUES (new.id, new.title, new.summary);
     END;
     CREATE VIRTUAL TABLE fts_segments USING fts5(text, content='transcript_segments', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
     CREATE TRIGGER fts_segments_ai AFTER INSERT ON transcript_segments BEGIN
        INSERT INTO fts_segments(rowid, text) VALUES (new.id, new.text);
     END;
     CREATE TRIGGER fts_segments_ad AFTER DELETE ON transcript_segments BEGIN
        INSERT INTO fts_segments(fts_segments, rowid, text) VALUES ('delete', old.id, old.text);
     END;
     INSERT INTO fts_meetings(fts_meetings) VALUES ('rebuild');
     INSERT INTO fts_segments(fts_segments) VALUES ('rebuild');",
    // 11: a readable placeholder title, "Meeting on Sun 22 Nov, 3:45 PM", instead of "Meeting 2026-11-22 15:45".
    //     Only titles nobody has changed ('default') and that still have the old date format are rewritten.
    human_title_sql!(
        "UPDATE meetings SET title = ",
        "created_at",
        " WHERE title_source = 'default' AND title GLOB 'Meeting [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]';"
    ),
];

/// Values of `Meeting::title_source`.
pub mod title_source {
    pub const DEFAULT: &str = "default";
    pub const MODEL: &str = "model";
    pub const USER: &str = "user";
}

/// `recording`: audio is being captured. `stopped`: finished normally.
/// `recovered`: the app died mid-recording; audio on disk was kept.
pub mod status {
    pub const RECORDING: &str = "recording";
    pub const STOPPED: &str = "stopped";
    pub const RECOVERED: &str = "recovered";
}

const COLS: &str = "id, title, created_at, summary, status, duration_ms, audio_dir, transcript_model, agenda, has_system_audio, summary_model, title_source";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub summary: String,
    pub status: String,
    pub duration_ms: i64,
    pub audio_dir: String,
    /// Manifest id of the model that produced the transcript ("" if none yet).
    pub transcript_model: String,
    /// Goals or agenda typed by the user; context for live hints.
    pub agenda: String,
    /// True when the other participants' audio (system audio) was recorded as a second track,
    /// in `audio_dir/system`.
    pub has_system_audio: bool,
    /// Manifest id of the model that wrote `summary` ("" if none yet).
    pub summary_model: String,
    /// Who chose the title: "default" (date placeholder), "model" or "user". See `title_source`.
    pub title_source: String,
}

mod search;
pub use search::{Match, MatchKind, SearchResult, HIT_END, HIT_START};

pub struct Db {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    /// Open (or create) a database file and bring it to the latest schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        // WAL: crash-safe, readers don't block the writer (NFR-09).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&self) -> Result<()> {
        let current = self.schema_version()? as usize;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as u32)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Create a meeting. An empty title becomes "Meeting YYYY-MM-DD HH:MM" in local time.
    pub fn create_meeting(&self, title: Option<&str>) -> Result<Meeting> {
        let ts = now();
        let given = title.map_or(false, |t| !t.trim().is_empty());
        let title = match title.map(str::trim) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => self.conn.query_row(
                human_title_sql!("SELECT ", "?1", ""),
                params![ts],
                |r| r.get::<_, String>(0),
            )?,
        };
        let source = if given { title_source::USER } else { title_source::DEFAULT };
        self.conn.execute(
            "INSERT INTO meetings (title, created_at, title_source) VALUES (?1, ?2, ?3)",
            params![title, ts, source],
        )?;
        self.get_meeting(self.conn.last_insert_rowid())
    }

    pub fn get_meeting(&self, id: i64) -> Result<Meeting> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM meetings WHERE id = ?1"),
                params![id],
                row_to_meeting,
            )
            .optional()?
            .ok_or(Error::NotFound(id))
    }

    /// Newest first.
    pub fn list_meetings(&self) -> Result<Vec<Meeting>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM meetings ORDER BY id DESC"))?;
        let rows = stmt.query_map([], row_to_meeting)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn rename_meeting(&self, id: i64, title: &str) -> Result<Meeting> {
        let n = self
            .conn
            .execute("UPDATE meetings SET title = ?1, title_source = ?2 WHERE id = ?3", params![title, title_source::USER, id])?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        self.get_meeting(id)
    }

    /// Set a title written by the assistant. A title the user chose is never replaced: returns
    /// `Ok(None)` and changes nothing in that case (decided in one statement, so a rename that
    /// happens at the same moment wins).
    pub fn set_model_title(&self, id: i64, title: &str) -> Result<Option<Meeting>> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(None);
        }
        let n = self.conn.execute(
            "UPDATE meetings SET title = ?1, title_source = ?2 WHERE id = ?3 AND title_source != ?4",
            params![title, title_source::MODEL, id, title_source::USER],
        )?;
        if n == 0 {
            self.get_meeting(id)?; // NotFound if there is no such meeting
            return Ok(None);
        }
        Ok(Some(self.get_meeting(id)?))
    }

    /// Replace the whole transcript (used after transcribing a finished recording).
    pub fn replace_transcript(&self, id: i64, model_id: &str, segments: &[TranscriptSegment]) -> Result<()> {
        self.get_meeting(id)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM transcript_segments WHERE meeting_id = ?1", params![id])?;
        for s in segments {
            tx.execute(
                "INSERT INTO transcript_segments (meeting_id, start_ms, end_ms, text, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, s.start_ms, s.end_ms, s.text, s.source.as_str()],
            )?;
        }
        tx.execute("UPDATE meetings SET transcript_model = ?1 WHERE id = ?2", params![model_id, id])?;
        tx.commit()?;
        Ok(())
    }

    /// Append one segment (live transcription).
    pub fn append_transcript_segment(&self, id: i64, model_id: &str, s: &TranscriptSegment) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transcript_segments (meeting_id, start_ms, end_ms, text, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, s.start_ms, s.end_ms, s.text, s.source.as_str()],
        )?;
        self.conn.execute("UPDATE meetings SET transcript_model = ?1 WHERE id = ?2", params![model_id, id])?;
        Ok(())
    }

    pub fn get_transcript(&self, id: i64) -> Result<Vec<TranscriptSegment>> {
        let mut stmt = self.conn.prepare(
            "SELECT start_ms, end_ms, text, source FROM transcript_segments WHERE meeting_id = ?1 ORDER BY start_ms, id",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok(TranscriptSegment::new(r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)
                .from_source(Source::parse(&r.get::<_, String>(3)?)))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Record that the other participants' audio was captured as a second track (on the same
    /// clock as the microphone).
    pub fn set_system_audio(&self, id: i64) -> Result<Meeting> {
        let n = self.conn.execute("UPDATE meetings SET has_system_audio = 1 WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        self.get_meeting(id)
    }

    /// Save the meeting notes (Markdown) and which model wrote them. An empty `summary` clears them.
    pub fn set_summary(&self, id: i64, summary: &str, model_id: &str) -> Result<Meeting> {
        let n = self.conn.execute(
            "UPDATE meetings SET summary = ?1, summary_model = ?2 WHERE id = ?3",
            params![summary, if summary.is_empty() { "" } else { model_id }, id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        self.get_meeting(id)
    }

    /// A unix timestamp as "YYYY-MM-DD HH:MM" in the computer's local time.
    pub fn local_time(&self, ts: i64) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M', ?1, 'unixepoch', 'localtime')",
            params![ts],
            |r| r.get(0),
        )?)
    }

    /// The day of a unix timestamp for people: "Mon 16 Nov" (with the year when it is not this year).
    pub fn day_label(&self, ts: i64) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT substr('SunMonTueWedThuFriSat', CAST(strftime('%w', ?1, 'unixepoch', 'localtime') AS INTEGER) * 3 + 1, 3) || ' ' ||
                    CAST(strftime('%d', ?1, 'unixepoch', 'localtime') AS INTEGER) || ' ' ||
                    substr('JanFebMarAprMayJunJulAugSepOctNovDec', (CAST(strftime('%m', ?1, 'unixepoch', 'localtime') AS INTEGER) - 1) * 3 + 1, 3) ||
                    CASE WHEN strftime('%Y', ?1, 'unixepoch', 'localtime') = strftime('%Y', 'now', 'localtime') THEN ''
                         ELSE ' ' || strftime('%Y', ?1, 'unixepoch', 'localtime') END",
            params![ts],
            |r| r.get(0),
        )?)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn set_agenda(&self, id: i64, agenda: &str) -> Result<Meeting> {
        let n = self.conn.execute("UPDATE meetings SET agenda = ?1 WHERE id = ?2", params![agenda, id])?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        self.get_meeting(id)
    }

    /// Deletes the meeting and its audio files (NFR-02).
    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        let m = self.get_meeting(id)?;
        self.conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        if !m.audio_dir.is_empty() {
            let _ = std::fs::remove_dir_all(&m.audio_dir);
        }
        Ok(())
    }

    /// Create a new meeting already in `recording` state.
    pub fn start_recording_meeting(&self, title: Option<&str>, recordings_root: &Path) -> Result<Meeting> {
        let m = self.create_meeting(title)?;
        self.begin_recording(m.id, recordings_root)
    }

    /// Put an existing meeting into `recording` state with audio directory `<root>/<id>`.
    /// One recording per meeting for now: refuses if audio already exists (it would be overwritten).
    pub fn begin_recording(&self, id: i64, recordings_root: &Path) -> Result<Meeting> {
        let m = self.get_meeting(id)?;
        if m.status == status::RECORDING {
            return Err(Error::Other("This meeting is already being recorded".into()));
        }
        let dir = recordings_root.join(id.to_string());
        if !crate::audio::wav::list_segments(&dir)?.is_empty() {
            return Err(Error::Other("This meeting already has a recording. Start a new meeting instead.".into()));
        }
        std::fs::create_dir_all(&dir)?;
        self.conn.execute(
            "UPDATE meetings SET status = ?1, audio_dir = ?2 WHERE id = ?3",
            params![status::RECORDING, dir.to_string_lossy(), id],
        )?;
        self.get_meeting(id)
    }

    /// Abort a recording that failed to start: remove the meeting and anything written.
    pub fn abort_recording(&self, id: i64) -> Result<()> {
        self.delete_meeting(id)
    }

    /// Mark a recording finished and store its duration.
    pub fn finish_recording(&self, id: i64, duration_ms: i64) -> Result<Meeting> {
        self.conn.execute(
            "UPDATE meetings SET status = ?1, duration_ms = ?2 WHERE id = ?3",
            params![status::STOPPED, duration_ms, id],
        )?;
        self.get_meeting(id)
    }

    /// Call on launch: any meeting still `recording` was interrupted. Keep its audio, compute the
    /// duration from what reached disk, and mark it `recovered`.
    pub fn recover_interrupted(&self) -> Result<Vec<Meeting>> {
        let stuck: Vec<Meeting> = self
            .list_meetings()?
            .into_iter()
            .filter(|m| m.status == status::RECORDING)
            .collect();
        let mut out = vec![];
        for m in stuck {
            let ms = crate::audio::wav::duration_ms(Path::new(&m.audio_dir)).unwrap_or(0) as i64;
            self.conn.execute(
                "UPDATE meetings SET status = ?1, duration_ms = ?2 WHERE id = ?3",
                params![status::RECOVERED, ms, m.id],
            )?;
            out.push(self.get_meeting(m.id)?);
        }
        Ok(out)
    }
}

fn row_to_meeting(r: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: r.get(0)?,
        title: r.get(1)?,
        created_at: r.get(2)?,
        summary: r.get(3)?,
        status: r.get(4)?,
        duration_ms: r.get(5)?,
        audio_dir: r.get(6)?,
        transcript_model: r.get(7)?,
        agenda: r.get(8)?,
        has_system_audio: r.get::<_, i64>(9)? != 0,
        summary_model: r.get(10)?,
        title_source: r.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_to_latest() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap() as usize, MIGRATIONS.len());
    }

    #[test]
    fn create_and_list_newest_first() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_meeting(Some("Standup")).unwrap();
        let b = db.create_meeting(Some("  1:1  ")).unwrap();
        assert_eq!(b.title, "1:1");
        let list = db.list_meetings().unwrap();
        assert_eq!(list.iter().map(|m| m.id).collect::<Vec<_>>(), vec![b.id, a.id]);
    }

    /// "Meeting on Sun 22 Nov, 3:45 PM", or with the year when it is not this year.
    fn looks_like_default_title(t: &str) -> bool {
        let Some(rest) = t.strip_prefix("Meeting on ") else { return false };
        let (date, time) = match rest.split_once(", ") {
            Some(x) => x,
            None => return false,
        };
        let w: Vec<&str> = date.split(' ').collect();
        let day_ok = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].contains(&w[0]);
        let month_ok = w.len() >= 3 && ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].contains(&w[2]) && w[1].parse::<u32>().is_ok_and(|d| (1..=31).contains(&d));
        let year_ok = w.len() == 3 || (w.len() == 4 && w[3].len() == 4 && w[3].parse::<u32>().is_ok());
        let (hm, ampm) = match time.split_once(' ') {
            Some(x) => x,
            None => return false,
        };
        let time_ok = hm.split_once(':').is_some_and(|(h, m)| h.parse::<u32>().is_ok_and(|h| (1..=12).contains(&h)) && m.len() == 2 && m.parse::<u32>().is_ok_and(|m| m < 60)) && (ampm == "AM" || ampm == "PM");
        day_ok && month_ok && year_ok && time_ok
    }

    #[test]
    fn empty_title_gets_a_readable_local_time_placeholder() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_meeting(Some("   ")).unwrap().title;
        let b = db.create_meeting(None).unwrap().title;
        assert!(looks_like_default_title(&a), "{a}");
        assert!(looks_like_default_title(&b), "{b}");
    }

    #[test]
    fn migration_4_rewrites_only_auto_generated_titles() {
        // A database at schema v3 containing old-style titles.
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3u32).unwrap();
        let ins = "INSERT INTO meetings (title, created_at) VALUES (?1, ?2)";
        conn.execute(ins, params!["Meeting 1795278543", 1795278543i64]).unwrap(); // auto-generated
        conn.execute(ins, params!["Meeting 1795278543", 1700000000i64]).unwrap(); // number does not match created_at
        conn.execute(ins, params!["Meeting 5", 1795278543i64]).unwrap(); // user title
        conn.execute(ins, params!["Weekly sync", 1795278543i64]).unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.schema_version().unwrap() as usize, MIGRATIONS.len());
        let titles: Vec<String> = db.list_meetings().unwrap().into_iter().rev().map(|m| m.title).collect();
        assert!(looks_like_default_title(&titles[0]), "{}", titles[0]);
        assert_eq!(&titles[1..], ["Meeting 1795278543", "Meeting 5", "Weekly sync"]);
    }

    #[test]
    fn rename_and_delete() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("Old")).unwrap();
        assert_eq!(db.rename_meeting(m.id, "New").unwrap().title, "New");
        db.delete_meeting(m.id).unwrap();
        assert!(matches!(db.get_meeting(m.id), Err(Error::NotFound(_))));
        assert!(matches!(db.delete_meeting(m.id), Err(Error::NotFound(_))));
    }

    #[test]
    fn reopening_file_keeps_data_and_does_not_remigrate() {
        let dir = std::env::temp_dir().join(format!("meetesh-test-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        {
            let db = Db::open(&path).unwrap();
            db.create_meeting(Some("Persisted")).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.list_meetings().unwrap().len(), 1);
        assert_eq!(db.schema_version().unwrap() as usize, MIGRATIONS.len());
        std::fs::remove_dir_all(dir).ok();
    }

    fn tmp_root(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("meetesh-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn recording_lifecycle() {
        let root = tmp_root("life");
        let db = Db::open_in_memory().unwrap();
        let m = db.start_recording_meeting(Some("Call"), &root).unwrap();
        assert_eq!(m.status, status::RECORDING);
        assert!(std::path::Path::new(&m.audio_dir).is_dir());
        let m = db.finish_recording(m.id, 12_000).unwrap();
        assert_eq!((m.status.as_str(), m.duration_ms), (status::STOPPED, 12_000));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn interrupted_recording_is_recovered_with_audio_intact() {
        use crate::audio::wav::SegmentWriter;
        let root = tmp_root("recover");
        let db = Db::open_in_memory().unwrap();
        let m = db.start_recording_meeting(Some("Crashed"), &root).unwrap();
        let mut w = SegmentWriter::with_segment_seconds(&m.audio_dir, 5).unwrap();
        w.push(&vec![0.1; 16_000 * 2]).unwrap();
        std::mem::forget(w); // app killed: no finish()
        let rec = db.recover_interrupted().unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].status, status::RECOVERED);
        assert!(rec[0].duration_ms >= 2_000 - 1_000, "got {}", rec[0].duration_ms);
        assert!(db.recover_interrupted().unwrap().is_empty()); // idempotent
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn delete_removes_audio_files() {
        let root = tmp_root("del");
        let db = Db::open_in_memory().unwrap();
        let m = db.start_recording_meeting(None, &root).unwrap();
        std::fs::write(std::path::Path::new(&m.audio_dir).join("seg-000001.wav"), b"x").unwrap();
        db.delete_meeting(m.id).unwrap();
        assert!(!std::path::Path::new(&m.audio_dir).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cannot_record_over_existing_audio() {
        let root = tmp_root("noover");
        let db = Db::open_in_memory().unwrap();
        let m = db.start_recording_meeting(None, &root).unwrap();
        std::fs::write(std::path::Path::new(&m.audio_dir).join("seg-000001.wav"), b"x").unwrap();
        db.finish_recording(m.id, 1000).unwrap();
        assert!(db.begin_recording(m.id, &root).is_err());
        // and a meeting mid-recording cannot be started twice
        let m2 = db.start_recording_meeting(None, &root).unwrap();
        assert!(db.begin_recording(m2.id, &root).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn transcript_roundtrip_and_cascade_delete() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("T")).unwrap();
        let seg = |a, b, t: &str| TranscriptSegment::new(a, b, t);
        db.replace_transcript(m.id, "whisper-base.en", &[seg(2000, 3000, "second"), seg(0, 1000, "first")]).unwrap();
        let t = db.get_transcript(m.id).unwrap();
        assert_eq!(t.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(db.get_meeting(m.id).unwrap().transcript_model, "whisper-base.en");
        // replacing overwrites
        db.replace_transcript(m.id, "whisper-tiny.en", &[seg(0, 1, "only")]).unwrap();
        assert_eq!(db.get_transcript(m.id).unwrap().len(), 1);
        db.append_transcript_segment(m.id, "whisper-tiny.en", &seg(5, 6, "more")).unwrap();
        assert_eq!(db.get_transcript(m.id).unwrap().len(), 2);
        db.delete_meeting(m.id).unwrap();
        assert!(db.get_transcript(m.id).unwrap().is_empty());
    }

    #[test]
    fn agenda_roundtrip_and_old_rows_default_to_empty() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("A")).unwrap();
        assert_eq!(m.agenda, "");
        assert_eq!(db.set_agenda(m.id, "Decide the launch date").unwrap().agenda, "Decide the launch date");
        assert!(matches!(db.set_agenda(999, "x"), Err(Error::NotFound(999))));
    }

    #[test]
    fn segments_remember_their_track_and_interleave_by_time() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("Call")).unwrap();
        assert!(!m.has_system_audio);
        let segs = [
            TranscriptSegment::new(0, 900, "hello").from_source(Source::Mic),
            TranscriptSegment::new(1000, 2000, "hi there").from_source(Source::System),
            TranscriptSegment::new(2500, 3000, "great").from_source(Source::Mic),
        ];
        db.replace_transcript(m.id, "m", &segs).unwrap();
        db.append_transcript_segment(m.id, "m", &TranscriptSegment::new(1500, 1800, "sure").from_source(Source::System)).unwrap();
        let t = db.get_transcript(m.id).unwrap();
        assert_eq!(
            t.iter().map(|s| (s.text.as_str(), s.source)).collect::<Vec<_>>(),
            [("hello", Source::Mic), ("hi there", Source::System), ("sure", Source::System), ("great", Source::Mic)]
        );
        assert!(db.set_system_audio(m.id).unwrap().has_system_audio);
        assert!(matches!(db.set_system_audio(999), Err(Error::NotFound(999))));
    }

    #[test]
    fn upgrading_from_schema_5_keeps_old_transcripts_as_microphone() {
        let path = std::env::temp_dir().join(format!("meetesh-v5-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            // Build a v5 database by hand, with one saved transcript.
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..5] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 5u32).unwrap();
            conn.execute("INSERT INTO meetings (title, created_at) VALUES ('Old', 1)", []).unwrap();
            conn.execute("INSERT INTO transcript_segments (meeting_id, start_ms, end_ms, text) VALUES (1, 0, 10, 'legacy')", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap() as usize, MIGRATIONS.len());
        let t = db.get_transcript(1).unwrap();
        assert_eq!(t[0].source, Source::Mic);
        assert!(!db.get_meeting(1).unwrap().has_system_audio);
        drop(db);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn summary_roundtrip_and_clear() {
        let db = Db::open_in_memory().unwrap();
        let m = db.create_meeting(Some("S")).unwrap();
        assert_eq!((m.summary.as_str(), m.summary_model.as_str()), ("", ""));
        let m = db.set_summary(m.id, "## Summary\nOk.", "gemma").unwrap();
        assert_eq!((m.summary.as_str(), m.summary_model.as_str()), ("## Summary\nOk.", "gemma"));
        let m = db.set_summary(m.id, "", "gemma").unwrap();
        assert_eq!((m.summary.as_str(), m.summary_model.as_str()), ("", ""), "clearing forgets the model too");
        assert!(matches!(db.set_summary(999, "x", "m"), Err(Error::NotFound(999))));
    }

    #[test]
    fn a_title_the_user_chose_is_never_replaced_automatically() {
        let db = Db::open_in_memory().unwrap();
        // Placeholder title: the model may set it, and replace its own title later.
        let m = db.create_meeting(None).unwrap();
        assert_eq!(m.title_source, "default");
        let m = db.set_model_title(m.id, "  Launch date for onboarding  ").unwrap().unwrap();
        assert_eq!((m.title.as_str(), m.title_source.as_str()), ("Launch date for onboarding", "model"));
        let m = db.set_model_title(m.id, "Onboarding launch plan").unwrap().unwrap();
        assert_eq!(m.title, "Onboarding launch plan");
        // The user renames it: from now on the model stays out.
        let m = db.rename_meeting(m.id, "My title").unwrap();
        assert_eq!(m.title_source, "user");
        assert!(db.set_model_title(m.id, "Something else").unwrap().is_none());
        assert_eq!(db.get_meeting(m.id).unwrap().title, "My title");
        // A title given at creation is the user's too.
        let named = db.create_meeting(Some("Standup")).unwrap();
        assert_eq!(named.title_source, "user");
        assert!(db.set_model_title(named.id, "x").unwrap().is_none());
        // Blank suggestions and unknown meetings.
        let d = db.create_meeting(None).unwrap();
        assert!(db.set_model_title(d.id, "   ").unwrap().is_none());
        assert!(matches!(db.set_model_title(999, "x"), Err(Error::NotFound(999))));
    }

    #[test]
    fn upgrading_from_schema_7_marks_existing_custom_titles_as_the_users() {
        let path = std::env::temp_dir().join(format!("meetesh-v7-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..7] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 7u32).unwrap();
            for t in ["Meeting 2026-11-21 21:59", "Quarterly planning", "Meeting with Sam"] {
                conn.execute("INSERT INTO meetings (title, created_at) VALUES (?1, 1)", [t]).unwrap();
            }
        }
        let db = Db::open(&path).unwrap();
        let first = db.get_meeting(1).unwrap();
        assert_eq!(first.title_source, "default", "an untouched placeholder may be replaced");
        assert!(looks_like_default_title(&first.title), "and it now has the readable form: {}", first.title);
        let by_title = |t: &str| db.list_meetings().unwrap().into_iter().find(|m| m.title == t).unwrap().title_source;
        assert_eq!(by_title("Quarterly planning"), "user");
        assert_eq!(by_title("Meeting with Sam"), "user", "only the exact date placeholder counts as untouched");
        drop(db);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn settings_are_stored_and_overwritten() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_setting("auto_hints").unwrap(), None);
        db.set_setting("auto_hints", "questions").unwrap();
        db.set_setting("other", "x").unwrap();
        db.set_setting("auto_hints", "off").unwrap();
        assert_eq!(db.get_setting("auto_hints").unwrap().as_deref(), Some("off"));
        assert_eq!(db.get_setting("other").unwrap().as_deref(), Some("x"));
    }

    #[test]
    fn upgrading_rewrites_untouched_date_titles_to_the_readable_form_and_nothing_else() {
        let path = std::env::temp_dir().join(format!("meetesh-v10-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let ts = 1_795_278_543i64;
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..10] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 10u32).unwrap();
            let ins = "INSERT INTO meetings (title, created_at, title_source) VALUES (?1, ?2, ?3)";
            conn.execute(ins, params!["Meeting 2026-11-22 15:45", ts, "default"]).unwrap(); // untouched placeholder
            conn.execute(ins, params!["Meeting 2026-11-22 15:45", ts, "user"]).unwrap(); // the user kept that exact text
            conn.execute(ins, params!["Launch date for onboarding", ts, "model"]).unwrap();
            conn.execute(ins, params!["Standup", ts, "user"]).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let by_id = |id: i64| db.get_meeting(id).unwrap().title;
        // What a new meeting created at the same moment would be called.
        let expected: String = db
            .conn
            .query_row(human_title_sql!("SELECT ", "?1", ""), params![ts], |r| r.get(0))
            .unwrap();
        assert!(looks_like_default_title(&expected), "{expected}");
        assert_eq!(by_id(1), expected, "an untouched placeholder is rewritten, to the same text a new meeting gets");
        assert_eq!(by_id(2), "Meeting 2026-11-22 15:45", "a title the user chose is never rewritten");
        assert_eq!(by_id(3), "Launch date for onboarding");
        assert_eq!(by_id(4), "Standup");
        drop(db);
        std::fs::remove_file(path).ok();
    }
}
