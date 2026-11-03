// SPDX-License-Identifier: MPL-2.0
//! Local SQLite persistence (offline-first): favorites, listening history, reports.
//!
//! The connection is single and protected by a `Mutex`: accesses are short and WAL mode
//! prevents concurrent reads (future sync worker) from blocking writes.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS favorites (
    station_id TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS listening_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id  TEXT NOT NULL,
    track_title TEXT NOT NULL,
    played_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_history_played_at ON listening_history (played_at DESC);

CREATE TABLE IF NOT EXISTS station_reports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id  TEXT NOT NULL,
    report_type TEXT NOT NULL CHECK (report_type IN ('dead_link', 'duplicate', 'outdated_info')),
    comment     TEXT,
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_reports_station ON station_reports (station_id);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Manually added stations: strictly local, never sent anywhere.
CREATE TABLE IF NOT EXISTS personal_stations (
    station_id TEXT PRIMARY KEY DEFAULT ('user:' || lower(hex(randomblob(8)))),
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL UNIQUE,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Favorite {
    pub station_id: String,
    pub name: String,
    pub stream_url: String,
    pub genre: Option<String>,
    #[serde(default)]
    pub added_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub station_id: String,
    pub track_title: String,
    pub played_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportType {
    DeadLink,
    Duplicate,
    OutdatedInfo,
}

impl ReportType {
    fn as_str(self) -> &'static str {
        match self {
            Self::DeadLink => "dead_link",
            Self::Duplicate => "duplicate",
            Self::OutdatedInfo => "outdated_info",
        }
    }
}

/// Station added by the user (`user:…` ID, private).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PersonalStation {
    pub station_id: String,
    pub name: String,
    pub stream_url: String,
    pub genre: Option<String>,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens (or creates) the on-disk database in WAL mode and applies the schema.
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        tracing::info!(journal_mode = %mode, "SQLite database opened");
        // NORMAL is safe in WAL mode (at worst the last transaction is lost on power loss).
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    #[cfg(test)]
    fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic during a query does not corrupt SQLite: we take the connection back.
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ── favorites ────────────────────────────────────────────────────────────

    /// Adds a station to favorites (updates its info if it is already there).
    pub fn add_favorite(&self, fav: &Favorite) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO favorites (station_id, name, stream_url, genre)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(station_id) DO UPDATE SET
                 name = excluded.name,
                 stream_url = excluded.stream_url,
                 genre = excluded.genre",
            params![fav.station_id, fav.name, fav.stream_url, fav.genre],
        )?;
        Ok(())
    }

    pub fn remove_favorite(&self, station_id: &str) -> rusqlite::Result<bool> {
        let deleted = self
            .conn()
            .execute("DELETE FROM favorites WHERE station_id = ?1", params![station_id])?;
        Ok(deleted > 0)
    }

    pub fn list_favorites(&self) -> rusqlite::Result<Vec<Favorite>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT station_id, name, stream_url, genre, added_at
             FROM favorites ORDER BY added_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Favorite {
                station_id: row.get(0)?,
                name: row.get(1)?,
                stream_url: row.get(2)?,
                genre: row.get(3)?,
                added_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    // ── settings ─────────────────────────────────────────────────────────────

    pub fn get_bool(&self, key: &str, default: bool) -> rusqlite::Result<bool> {
        let value: Option<String> = self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?;
        Ok(value.map_or(default, |v| v == "true"))
    }

    pub fn set_bool(&self, key: &str, value: bool) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, if value { "true" } else { "false" }],
        )?;
        Ok(())
    }

    // ── personal_stations ────────────────────────────────────────────────────

    /// Adds a personal station. If the URL already exists, returns the existing station.
    pub fn add_personal_station(
        &self,
        name: &str,
        stream_url: &str,
        genre: Option<&str>,
    ) -> rusqlite::Result<PersonalStation> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO personal_stations (name, stream_url, genre) VALUES (?1, ?2, ?3)
             ON CONFLICT(stream_url) DO NOTHING",
            params![name, stream_url, genre],
        )?;
        conn.query_row(
            "SELECT station_id, name, stream_url, genre FROM personal_stations WHERE stream_url = ?1",
            params![stream_url],
            personal_from_row,
        )
    }

    pub fn list_personal_stations(&self) -> rusqlite::Result<Vec<PersonalStation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT station_id, name, stream_url, genre FROM personal_stations
             ORDER BY added_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], personal_from_row)?;
        rows.collect()
    }

    pub fn remove_personal_station(&self, station_id: &str) -> rusqlite::Result<bool> {
        let deleted = self
            .conn()
            .execute("DELETE FROM personal_stations WHERE station_id = ?1", params![station_id])?;
        Ok(deleted > 0)
    }

    // ── listening_history ────────────────────────────────────────────────────

    pub fn log_track(&self, station_id: &str, track_title: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO listening_history (station_id, track_title) VALUES (?1, ?2)",
            params![station_id, track_title],
        )?;
        Ok(())
    }

    pub fn recent_history(&self, limit: u32) -> rusqlite::Result<Vec<HistoryEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, station_id, track_title, played_at
             FROM listening_history ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                station_id: row.get(1)?,
                track_title: row.get(2)?,
                played_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    // ── station_reports ──────────────────────────────────────────────────────

    pub fn add_report(
        &self,
        station_id: &str,
        report_type: ReportType,
        comment: Option<&str>,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO station_reports (station_id, report_type, comment) VALUES (?1, ?2, ?3)",
            params![station_id, report_type.as_str(), comment],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

fn personal_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonalStation> {
    Ok(PersonalStation {
        station_id: row.get(0)?,
        name: row.get(1)?,
        stream_url: row.get(2)?,
        genre: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(id: &str) -> Favorite {
        Favorite {
            station_id: id.into(),
            name: format!("Radio {id}"),
            stream_url: "https://live.aurafm.org/R-Dwa".into(),
            genre: Some("ambient".into()),
            added_at: None,
        }
    }

    #[test]
    fn favorites_roundtrip_and_upsert() {
        let db = Database::open_in_memory().unwrap();
        db.add_favorite(&fav("a")).unwrap();
        db.add_favorite(&Favorite { name: "Renamed".into(), ..fav("a") }).unwrap();
        let list = db.list_favorites().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Renamed");
        assert!(list[0].added_at.is_some());
        assert!(db.remove_favorite("a").unwrap());
        assert!(!db.remove_favorite("a").unwrap());
    }

    #[test]
    fn history_is_newest_first() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("a", "first").unwrap();
        db.log_track("a", "second").unwrap();
        let history = db.recent_history(10).unwrap();
        assert_eq!(history[0].track_title, "second");
        assert_eq!(db.recent_history(1).unwrap().len(), 1);
    }

    #[test]
    fn reports_validate_type() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.add_report("a", ReportType::DeadLink, Some("404")).unwrap(), 1);
        let bad = db.conn().execute(
            "INSERT INTO station_reports (station_id, report_type) VALUES ('a', 'nope')",
            [],
        );
        assert!(bad.is_err());
    }

    #[test]
    fn file_database_uses_wal() {
        let path = std::env::temp_dir().join(format!("soundfolk-test-{}.db", std::process::id()));
        let db = Database::open(&path).unwrap();
        let mode: String =
            db.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        drop(db);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn settings_use_the_default_until_set() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_bool("click_counting", true).unwrap());
        assert!(!db.get_bool("click_notice_seen", false).unwrap());
        db.set_bool("click_counting", false).unwrap();
        db.set_bool("click_counting", false).unwrap(); // idempotent
        assert!(!db.get_bool("click_counting", true).unwrap());
    }

    #[test]
    fn personal_stations_get_private_ids_and_dedupe_by_url() {
        let db = Database::open_in_memory().unwrap();
        let a = db.add_personal_station("My radio", "http://192.168.1.10:8000/live", None).unwrap();
        assert!(a.station_id.starts_with("user:"));
        let again = db.add_personal_station("Other name", "http://192.168.1.10:8000/live", None).unwrap();
        assert_eq!(again.station_id, a.station_id);
        assert_eq!(db.list_personal_stations().unwrap().len(), 1);
        assert!(db.remove_personal_station(&a.station_id).unwrap());
        assert!(db.list_personal_stations().unwrap().is_empty());
    }
}
