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

const MIGRATIONS: &[&str] = &[
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
];

/// `recording`: audio is being captured. `stopped`: finished normally.
/// `recovered`: the app died mid-recording; audio on disk was kept.
pub mod status {
    pub const RECORDING: &str = "recording";
    pub const STOPPED: &str = "stopped";
    pub const RECOVERED: &str = "recovered";
}

const COLS: &str = "id, title, created_at, summary, status, duration_ms, audio_dir, transcript_model, agenda, has_system_audio, summary_model";

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
}

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
        let title = match title.map(str::trim) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => self.conn.query_row(
                "SELECT 'Meeting ' || strftime('%Y-%m-%d %H:%M', ?1, 'unixepoch', 'localtime')",
                params![ts],
                |r| r.get::<_, String>(0),
            )?,
        };
        self.conn.execute(
            "INSERT INTO meetings (title, created_at) VALUES (?1, ?2)",
            params![title, ts],
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
            .execute("UPDATE meetings SET title = ?1 WHERE id = ?2", params![title, id])?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        self.get_meeting(id)
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

    fn looks_like_default_title(t: &str) -> bool {
        // "Meeting 2026-11-21 21:59"
        let b = t.as_bytes();
        t.len() == 24
            && t.starts_with("Meeting ")
            && b[12] == b'-' && b[15] == b'-' && b[18] == b' ' && b[21] == b':'
            && t[8..12].chars().all(|c| c.is_ascii_digit())
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
}
