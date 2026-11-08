// SPDX-License-Identifier: MPL-2.0
//! Local SQLite persistence (offline-first): favorites, listening history, likes, listening
//! sessions.
//!
//! The connection is single and protected by a `Mutex`: accesses are short and WAL mode
//! prevents concurrent reads (future sync worker) from blocking writes.

use std::collections::HashMap;
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

/// Schema version 1, applied by [`baseline`]. Frozen: a later change goes in a new entry of
/// [`MIGRATIONS`], never here.
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

-- Dropped by version 4 (`drop_station_reports`): nothing ever wrote to it.
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

-- Loudness of each station heard, learned by Constant volume (docs/11-loudness.md): the
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Favorite {
    pub station_id: String,
    pub name: String,
    pub stream_url: String,
    pub genre: Option<String>,
    #[serde(default)]
    pub added_at: Option<String>,
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
        migrate(&conn)?;
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

    /// Adds a station to favorites, at the top (updates its info, not its place, if it is already
    /// there). History rows of that station logged without a name take its
    /// name; rows that have one keep it (the name at the time of listening).
    pub fn add_favorite(&self, fav: &Favorite) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO favorites (station_id, name, stream_url, genre, position)
             VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MIN(position), 0) - 1 FROM favorites))
             ON CONFLICT(station_id) DO UPDATE SET
                 name = excluded.name,
                 stream_url = excluded.stream_url,
                 genre = excluded.genre",
            params![fav.station_id, fav.name, fav.stream_url, fav.genre],
        )?;
        tx.execute(
            "UPDATE listening_history SET station_name = ?2
             WHERE station_id = ?1 AND station_name IS NULL",
            params![fav.station_id, fav.name],
        )?;
        tx.commit()
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
             FROM favorites ORDER BY position, added_at DESC, rowid DESC",
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

    /// One favorite, by its station id.
    pub fn favorite(&self, station_id: &str) -> rusqlite::Result<Option<Favorite>> {
        self.conn()
            .query_row(
                "SELECT station_id, name, stream_url, genre, added_at
                 FROM favorites WHERE station_id = ?1",
                params![station_id],
                |row| {
                    Ok(Favorite {
                        station_id: row.get(0)?,
                        name: row.get(1)?,
                        stream_url: row.get(2)?,
                        genre: row.get(3)?,
                        added_at: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    /// Puts the favorites in the order of `station_ids` (the listener dragged them). A favorite
    /// missing from it keeps its position; an unknown ID is ignored.
    pub fn reorder_favorites(&self, station_ids: &[String]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE favorites SET position = ?2 WHERE station_id = ?1")?;
            for (position, id) in station_ids.iter().enumerate() {
                stmt.execute(params![id, position as i64])?;
            }
        }
        tx.commit()
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
        self.set_strs(&[(key, value)])
    }

    /// Several settings that go together, written in one transaction.
    pub fn set_strs(&self, settings: &[(&str, &str)]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (key, value) in settings {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()
    }

    // ── personal_stations ────────────────────────────────────────────────────

    /// Adds a personal station. If the URL already exists, returns the existing station, which
    /// keeps its origin.
    pub fn add_personal_station(
        &self,
        name: &str,
        stream_url: &str,
        genre: Option<&str>,
    ) -> rusqlite::Result<PersonalStation> {
        self.insert_personal_station(name, stream_url, genre, None)
    }

    /// Adds a personal station received in a share link: its stream stays off the local network.
    pub fn add_personal_station_from_link(
        &self,
        name: &str,
        stream_url: &str,
    ) -> rusqlite::Result<PersonalStation> {
        self.insert_personal_station(name, stream_url, None, Some("link"))
    }

    fn insert_personal_station(
        &self,
        name: &str,
        stream_url: &str,
        genre: Option<&str>,
        origin: Option<&str>,
    ) -> rusqlite::Result<PersonalStation> {
        let conn = self.conn();
        conn.execute(
            // Typing an address by hand is the listener's choice: it lifts the rule of a station
            // first received in a link (docs/16-sharing.md §6.2).
            "INSERT INTO personal_stations (name, stream_url, genre, origin) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(stream_url) DO UPDATE SET origin = CASE WHEN ?4 IS NULL THEN NULL ELSE origin END",
            params![name, stream_url, genre, origin],
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

    /// Whether the personal station was received in a share link.
    pub fn personal_station_from_link(&self, station_id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT origin = 'link' FROM personal_stations WHERE station_id = ?1",
                params![station_id],
                |r| r.get::<_, Option<bool>>(0),
            )
            .optional()?
            .flatten()
            .unwrap_or(false))
    }

    pub fn remove_personal_station(&self, station_id: &str) -> rusqlite::Result<bool> {
        let deleted = self.conn().execute(
            "DELETE FROM personal_stations WHERE station_id = ?1",
            params![station_id],
        )?;
        Ok(deleted > 0)
    }

    // ── station_names ────────────────────────────────────────────────────────

    /// Names the user gave stations, by station ID.
    pub fn station_names(&self) -> rusqlite::Result<HashMap<String, String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT station_id, name FROM station_names")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// The name the user gave a station, if any.
    pub fn station_name(&self, station_id: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT name FROM station_names WHERE station_id = ?1",
                params![station_id],
                |r| r.get(0),
            )
            .optional()
    }

    /// Names a station; a blank name gives it back its own. Returns the name kept, if any.
    pub fn set_station_name(
        &self,
        station_id: &str,
        name: Option<&str>,
    ) -> rusqlite::Result<Option<String>> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        let conn = self.conn();
        match name {
            Some(name) => conn.execute(
                "INSERT INTO station_names (station_id, name) VALUES (?1, ?2)
                 ON CONFLICT(station_id) DO UPDATE SET name = excluded.name",
                params![station_id, name],
            )?,
            None => conn.execute(
                "DELETE FROM station_names WHERE station_id = ?1",
                params![station_id],
            )?,
        };
        Ok(name.map(str::to_owned))
    }
}

type Migration = fn(&Connection) -> rusqlite::Result<()>;

/// Schema migrations, in order: the database is at version `n` (`PRAGMA user_version`) once the
/// first `n` have run. Append only; never edit or reorder one that has shipped.
const MIGRATIONS: &[Migration] = &[
    baseline,
    history_and_likes_indexes,
    history_station_names,
    drop_station_reports,
    station_names,
    favorite_positions,
    personal_station_origin,
];

/// Schema version of this build (`user_version` is a 32-bit integer in SQLite).
const LATEST: u32 = MIGRATIONS.len() as u32;

/// Runs the migrations the database has not had yet, each in its own transaction with the
/// version bump, so a failure leaves the database at the last version that fully applied.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > LATEST {
        // Written by a newer soundfolk. Refusing it would lock the user out after a downgrade;
        // what the newer migrations added is simply not used here.
        tracing::warn!(
            current,
            latest = LATEST,
            "database schema is newer than this build"
        );
        return Ok(());
    }
    for version in current..LATEST {
        let tx = conn.unchecked_transaction()?;
        MIGRATIONS[version as usize](&tx)?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
        tracing::info!(version = version + 1, "database schema migrated");
    }
    Ok(())
}

/// Version 1: the schema as it stood before versioning. Databases from that time are all at
/// version 0 but with more or fewer `listening_history` columns depending on their age, hence the
/// columns probed one by one.
fn baseline(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
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
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_history_track_key ON listening_history (track_key);",
    )
}

/// Version 2: indexes on the two unbounded tables for the lookups that scanned them, the history by
/// station (dedup on insert, re-parsing, per-station stats) and the likes by history row (kept in
/// step when a history row is re-parsed).
fn history_and_likes_indexes(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_history_station ON listening_history (station_id);
         CREATE INDEX IF NOT EXISTS idx_likes_history ON likes (history_id);",
    )
}

/// Version 3: history rows logged without a station name take the name of the favorite or
/// personal station of the same ID, once, instead of every read joining both tables for them.
/// New rows get that name when they are logged (see `history::known_station_name`).
fn history_station_names(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE listening_history SET station_name = COALESCE(
             (SELECT name FROM favorites f WHERE f.station_id = listening_history.station_id),
             (SELECT name FROM personal_stations p WHERE p.station_id = listening_history.station_id))
         WHERE station_name IS NULL;",
    )
}

/// Version 4: `station_reports` goes. The app never offered a way to report a station, so the table
/// was always empty, and a future report feature would need a table of its own.
fn drop_station_reports(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS station_reports;")
}

/// Version 5: the names the user gives stations. Read over the name stored with each history row,
/// like and session, which keeps the name the station had then (the title parser relies on it).
fn station_names(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS station_names (
             station_id TEXT PRIMARY KEY,
             name       TEXT NOT NULL
         );",
    )
}

/// Version 6: the favorites get a place of their own, so the listener can reorder them. They keep
/// the order they were shown in until then, the newest first.
fn favorite_positions(conn: &Connection) -> rusqlite::Result<()> {
    add_column(conn, "favorites", "position INTEGER NOT NULL DEFAULT 0")?;
    conn.execute_batch(
        "UPDATE favorites SET position = (
             SELECT COUNT(*) FROM favorites f
             WHERE COALESCE(f.added_at, '') > COALESCE(favorites.added_at, '')
                OR (COALESCE(f.added_at, '') = COALESCE(favorites.added_at, '')
                    AND f.rowid > favorites.rowid));",
    )
}

/// Version 7: where a personal station came from. 'link' for one received in a share link, whose
/// stream stays off the local network (docs/16-sharing.md §6.2); NULL for one typed by hand.
fn personal_station_origin(conn: &Connection) -> rusqlite::Result<()> {
    add_column(conn, "personal_stations", "origin TEXT")
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

    fn favorite_ids(db: &Database) -> Vec<String> {
        db.list_favorites()
            .unwrap()
            .into_iter()
            .map(|f| f.station_id)
            .collect()
    }

    #[test]
    fn new_favorites_go_on_top_and_keep_the_order_given() {
        let db = Database::open_in_memory().unwrap();
        for id in ["a", "b", "c"] {
            db.add_favorite(&fav(id)).unwrap();
        }
        assert_eq!(favorite_ids(&db), ["c", "b", "a"]);

        db.reorder_favorites(&["a".into(), "c".into(), "b".into(), "gone".into()])
            .unwrap();
        assert_eq!(favorite_ids(&db), ["a", "c", "b"]);

        // Updating a favorite leaves it where it is; a new one goes on top.
        db.add_favorite(&Favorite {
            name: "Renamed".into(),
            ..fav("b")
        })
        .unwrap();
        db.add_favorite(&fav("d")).unwrap();
        assert_eq!(favorite_ids(&db), ["d", "a", "c", "b"]);
    }

    #[test]
    fn favorites_keep_their_order_through_the_positions_migration() {
        let conn = Connection::open_in_memory().unwrap();
        for migration in &MIGRATIONS[..5] {
            migration(&conn).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            "INSERT INTO favorites (station_id, name, stream_url, added_at) VALUES
                 ('old', 'Old', 'u1', '2025-01-01 10:00:00'),
                 ('new', 'New', 'u2', '2025-03-01 10:00:00'),
                 ('tie1', 'Tie 1', 'u3', '2025-02-01 10:00:00'),
                 ('tie2', 'Tie 2', 'u4', '2025-02-01 10:00:00');",
        )
        .unwrap();
        let db = Database::init(conn).unwrap();
        assert_eq!(favorite_ids(&db), ["new", "tie2", "tie1", "old"]);
    }

    fn user_version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn new_database_is_at_the_latest_version() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        assert_eq!(user_version(&conn), LATEST);
        migrate(&conn).unwrap(); // nothing left to run
        assert_eq!(user_version(&conn), LATEST);
    }

    #[test]
    fn unversioned_database_is_brought_up_to_date() {
        // A database from before the history gained its parsed columns.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE listening_history (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 station_id  TEXT NOT NULL,
                 track_title TEXT NOT NULL,
                 played_at   DATETIME DEFAULT CURRENT_TIMESTAMP
             );
             INSERT INTO listening_history (station_id, track_title)
                 VALUES ('a', 'Artist - Title');",
        )
        .unwrap();
        let db = Database::init(conn).unwrap();
        let conn = db.conn();
        assert_eq!(user_version(&conn), LATEST);
        let (title, key): (String, Option<String>) = conn
            .query_row(
                "SELECT track_title, track_key FROM listening_history",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "Artist - Title");
        assert!(key.is_some(), "the old row is re-parsed");
        let indexed: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'idx_history_track_key')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(indexed);
    }

    #[test]
    fn version_1_database_gains_the_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        baseline(&conn).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        let db = Database::init(conn).unwrap();
        let conn = db.conn();
        assert_eq!(user_version(&conn), LATEST);
        for index in ["idx_history_station", "idx_likes_history"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                    [index],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(exists, "{index}");
        }
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN DELETE FROM likes WHERE history_id = 1 AND track_key != 'k'",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("idx_likes_history"), "{plan}");
    }

    #[test]
    fn version_2_history_takes_the_known_station_names() {
        let conn = Connection::open_in_memory().unwrap();
        baseline(&conn).unwrap();
        history_and_likes_indexes(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO favorites (station_id, name, stream_url) VALUES ('fav', 'Fav', 'u1');
             INSERT INTO personal_stations (station_id, name, stream_url) VALUES ('own', 'Own', 'u2');
             INSERT INTO listening_history (station_id, station_name, track_title) VALUES
                 ('fav', NULL, 't1'), ('own', NULL, 't2'), ('fav', 'Kept', 't3'), ('none', NULL, 't4');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        let db = Database::init(conn).unwrap();
        let conn = db.conn();
        assert_eq!(user_version(&conn), LATEST);
        let names: Vec<Option<String>> = conn
            .prepare("SELECT station_name FROM listening_history ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            names,
            [
                Some("Fav".into()),
                Some("Own".into()),
                Some("Kept".into()),
                None
            ]
        );
    }

    #[test]
    fn version_3_database_loses_the_reports_table() {
        let conn = Connection::open_in_memory().unwrap();
        baseline(&conn).unwrap();
        history_and_likes_indexes(&conn).unwrap();
        history_station_names(&conn).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        let db = Database::init(conn).unwrap();
        let conn = db.conn();
        assert_eq!(user_version(&conn), LATEST);
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('station_reports', 'idx_reports_station')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn newer_database_still_opens() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", LATEST + 1)
            .unwrap();
        let db = Database::init(conn).unwrap();
        assert_eq!(user_version(&db.conn()), LATEST + 1);
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
    fn station_names_are_set_trimmed_and_cleared() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.station_name("a").unwrap(), None);
        assert_eq!(
            db.set_station_name("a", Some("  Groove Salad ")).unwrap(),
            Some("Groove Salad".to_owned())
        );
        db.set_station_name("b", Some("Drone Zone")).unwrap();
        db.set_station_name("b", Some("Drones")).unwrap();
        assert_eq!(
            db.station_name("a").unwrap().as_deref(),
            Some("Groove Salad")
        );
        assert_eq!(db.station_names().unwrap().len(), 2);
        assert_eq!(db.set_station_name("a", Some(" ")).unwrap(), None);
        db.set_station_name("b", None).unwrap();
        assert!(db.station_names().unwrap().is_empty());
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
