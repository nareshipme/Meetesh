//! SQLite storage with a tiny migration system.
//!
//! Schema version is tracked in `PRAGMA user_version`. Add new migrations to the
//! end of `MIGRATIONS`; never edit an existing entry.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

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
];

/// `recording`: audio is being captured. `stopped`: finished normally.
/// `recovered`: the app died mid-recording; audio on disk was kept.
pub mod status {
    pub const RECORDING: &str = "recording";
    pub const STOPPED: &str = "stopped";
    pub const RECOVERED: &str = "recovered";
}

const COLS: &str = "id, title, created_at, transcript, summary, status, duration_ms, audio_dir";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub transcript: String,
    pub summary: String,
    pub status: String,
    pub duration_ms: i64,
    pub audio_dir: String,
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

    /// Create a meeting. An empty title gets a placeholder name.
    pub fn create_meeting(&self, title: Option<&str>) -> Result<Meeting> {
        let ts = now();
        let title = match title.map(str::trim) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => format!("Meeting {ts}"),
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
        transcript: r.get(3)?,
        summary: r.get(4)?,
        status: r.get(5)?,
        duration_ms: r.get(6)?,
        audio_dir: r.get(7)?,
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

    #[test]
    fn empty_title_gets_placeholder() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.create_meeting(Some("   ")).unwrap().title.starts_with("Meeting "));
        assert!(db.create_meeting(None).unwrap().title.starts_with("Meeting "));
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
}
