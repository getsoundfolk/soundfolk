// SPDX-License-Identifier: MPL-2.0
//! Local SQLite persistence (offline-first): favorites, listening history, likes, listening
//! sessions, reports.
//!
//! The connection is single and protected by a `Mutex`: accesses are short and WAL mode
//! prevents concurrent reads (future sync worker) from blocking writes.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

mod covers;
mod history;
mod identify;
mod loudness;
mod sessions;
mod stats;

pub use history::{HistoryEntry, HistorySummary, LikedTrack};
pub use identify::TrackToIdentify;
pub use stats::Stats;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS favorites (
    station_id TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- `artist`, `title` and `track_key` are parsed from `track_title` (see `tracks.rs`) and recomputed
-- when the parsing rules change, unless `title_source` is set (artist and title given apart by the
-- source); `track_title` is never altered.
CREATE TABLE IF NOT EXISTS listening_history (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    track_title  TEXT NOT NULL,
    artist       TEXT,
    title        TEXT,
    track_key    TEXT,
    title_source TEXT,
    played_at    DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_history_played_at ON listening_history (played_at DESC);

-- One row per liked track (same track_key = same track). Self-contained: clearing the history
-- keeps the likes, and the station is the one the track was liked on.
CREATE TABLE IF NOT EXISTS likes (
    track_key    TEXT PRIMARY KEY,
    artist       TEXT,
    title        TEXT NOT NULL,
    raw_title    TEXT NOT NULL,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    history_id   INTEGER,
    liked_at     DATETIME DEFAULT CURRENT_TIMESTAMP
);

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

-- When each station was actually playing. `last_seen_at` is a heartbeat: a session cut by a crash
-- ends there.
CREATE TABLE IF NOT EXISTS listening_sessions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    started_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_seen_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    ended_at     DATETIME
);
CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON listening_sessions (started_at);

-- MusicBrainz identification of liked tracks (opt-in), keyed like the likes. `tags` and `genres`
-- are JSON arrays of lowercase names.
CREATE TABLE IF NOT EXISTS tracks (
    track_key          TEXT PRIMARY KEY,
    status             TEXT NOT NULL CHECK (status IN ('pending', 'matched', 'not_found')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    next_attempt_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
    recording_mbid     TEXT,
    title              TEXT,
    artist_mbid        TEXT,
    album_mbid         TEXT,
    first_release_date TEXT,
    tags               TEXT,
    looked_up_at       DATETIME
);

CREATE TABLE IF NOT EXISTS artists (
    mbid         TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    country      TEXT,
    genres       TEXT,
    looked_up_at DATETIME
);

-- Release groups: the album, whatever the edition.
CREATE TABLE IF NOT EXISTS albums (
    mbid         TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    primary_type TEXT
);

-- Cover art found online for a track (`tracks.rs` key), or NULL when none was found: each track is
-- looked up once (again after a while when nothing was found).
CREATE TABLE IF NOT EXISTS covers (
    track_key    TEXT PRIMARY KEY,
    url          TEXT,
    looked_up_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Loudness of each station heard, learned by Constant volume (docs/10-loudness.md): the
-- Radio-Browser UUID, else the stream URL. Cleared with the history.
CREATE TABLE IF NOT EXISTS station_loudness (
    station_key   TEXT PRIMARY KEY,
    loudness_lufs REAL NOT NULL,
    peak_dbfs     REAL NOT NULL,
    measured_s    REAL NOT NULL,
    updated_at    INTEGER NOT NULL
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

/// Indexes on columns that older databases only gain in [`migrate`].
const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS idx_history_track_key ON listening_history (track_key);
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

/// Cheap to clone: the clones share the connection (for work moved to a blocking thread).
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
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
    pub(crate) fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        conn.execute_batch(INDEXES)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.reparse_history_if_outdated()?;
        db.close_abandoned_sessions()?;
        Ok(db)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic during a query does not corrupt SQLite: we take the connection back.
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        let deleted = self.conn().execute(
            "DELETE FROM favorites WHERE station_id = ?1",
            params![station_id],
        )?;
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
        Ok(self.get_str(key)?.map_or(default, |v| v == "true"))
    }

    pub fn set_bool(&self, key: &str, value: bool) -> rusqlite::Result<()> {
        self.set_str(key, if value { "true" } else { "false" })
    }

    /// A list of short ids (no commas), or `None` until it is first set. An empty list is a choice.
    pub fn get_list(&self, key: &str) -> rusqlite::Result<Option<Vec<String>>> {
        Ok(self.get_str(key)?.map(|v| {
            v.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        }))
    }

    pub fn set_list(&self, key: &str, values: &[String]) -> rusqlite::Result<()> {
        self.set_str(key, &values.join(","))
    }

    pub fn get_str(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn set_str(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
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

    pub fn personal_station(&self, station_id: &str) -> rusqlite::Result<Option<PersonalStation>> {
        self.conn()
            .query_row(
                "SELECT station_id, name, stream_url, genre FROM personal_stations WHERE station_id = ?1",
                params![station_id],
                personal_from_row,
            )
            .optional()
    }

    pub fn remove_personal_station(&self, station_id: &str) -> rusqlite::Result<bool> {
        let deleted = self.conn().execute(
            "DELETE FROM personal_stations WHERE station_id = ?1",
            params![station_id],
        )?;
        Ok(deleted > 0)
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

/// Brings a database created by an older version up to the current schema.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // `title_source`: NULL = parsed from `track_title`; 'source' = artist and title as the
    // source gave them (never re-parsed).
    for column in [
        "station_name TEXT",
        "artist TEXT",
        "title TEXT",
        "track_key TEXT",
        "title_source TEXT",
    ] {
        add_column(conn, "listening_history", column)?;
    }
    Ok(())
}

/// `ALTER TABLE … ADD COLUMN` unless the column already exists (`column` = name and type).
fn add_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<()> {
    let name = column.split_whitespace().next().unwrap_or(column);
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, name],
        |r| r.get(0),
    )?;
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column}"))?;
    }
    Ok(())
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
pub(super) mod tests {
    use super::*;

    pub(super) fn fav(id: &str) -> Favorite {
        Favorite {
            station_id: id.into(),
            name: format!("Radio {id}"),
            stream_url: "https://live.aurafm.org/R-Dwa".into(),
            genre: Some("ambient".into()),
            added_at: None,
        }
    }

    #[test]
    fn clones_share_the_connection() {
        let db = Database::open_in_memory().unwrap();
        let clone = db.clone();
        clone.add_favorite(&fav("a")).unwrap();
        assert_eq!(db.list_favorites().unwrap().len(), 1);
    }

    #[test]
    fn favorites_roundtrip_and_upsert() {
        let db = Database::open_in_memory().unwrap();
        db.add_favorite(&fav("a")).unwrap();
        db.add_favorite(&Favorite {
            name: "Renamed".into(),
            ..fav("a")
        })
        .unwrap();
        let list = db.list_favorites().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Renamed");
        assert!(list[0].added_at.is_some());
        assert!(db.remove_favorite("a").unwrap());
        assert!(!db.remove_favorite("a").unwrap());
    }

    #[test]
    fn reports_validate_type() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            db.add_report("a", ReportType::DeadLink, Some("404"))
                .unwrap(),
            1
        );
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
        let mode: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
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
    fn lists_tell_unset_from_empty() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_list("track_services").unwrap(), None);
        db.set_list("track_services", &[]).unwrap();
        assert_eq!(db.get_list("track_services").unwrap(), Some(vec![]));
        db.set_list("track_services", &["bandcamp".into(), "genius".into()])
            .unwrap();
        assert_eq!(
            db.get_list("track_services").unwrap(),
            Some(vec!["bandcamp".into(), "genius".into()])
        );
    }

    #[test]
    fn personal_stations_get_private_ids_and_dedupe_by_url() {
        let db = Database::open_in_memory().unwrap();
        let a = db
            .add_personal_station("My radio", "http://192.168.1.10:8000/live", None)
            .unwrap();
        assert!(a.station_id.starts_with("user:"));
        let again = db
            .add_personal_station("Other name", "http://192.168.1.10:8000/live", None)
            .unwrap();
        assert_eq!(again.station_id, a.station_id);
        assert_eq!(db.list_personal_stations().unwrap().len(), 1);
        assert!(db.remove_personal_station(&a.station_id).unwrap());
        assert!(db.list_personal_stations().unwrap().is_empty());
    }
}
