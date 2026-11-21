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
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub transcript: String,
    pub summary: String,
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
                "SELECT id, title, created_at, transcript, summary FROM meetings WHERE id = ?1",
                params![id],
                row_to_meeting,
            )
            .optional()?
            .ok_or(Error::NotFound(id))
    }

    /// Newest first.
    pub fn list_meetings(&self) -> Result<Vec<Meeting>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, transcript, summary FROM meetings ORDER BY id DESC",
        )?;
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

    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }
}

fn row_to_meeting(r: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: r.get(0)?,
        title: r.get(1)?,
        created_at: r.get(2)?,
        transcript: r.get(3)?,
        summary: r.get(4)?,
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
}
