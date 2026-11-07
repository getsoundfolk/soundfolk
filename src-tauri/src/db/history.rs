// SPDX-License-Identifier: MPL-2.0
//! Listening history (titles announced on air) and liked tracks.

use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use super::Database;
use crate::tracks::{self, ParsedTitle};

/// Version of the title parser the stored `artist` / `title` / `track_key` columns come from.
const SETTING_PARSER_VERSION: &str = "title_parser_version";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub station_id: String,
    /// The name the user gave the station, else the name at the time of listening (for old rows,
    /// the favorite or personal station name if known).
    pub station_name: Option<String>,
    /// Raw title as announced on air.
    pub track_title: String,
    pub artist: Option<String>,
    pub title: String,
    pub track_key: String,
    pub played_at: String,
    pub liked: bool,
    /// Artist and title cleaned for a music service's search box.
    pub search_query: String,
}

/// Size of the history, for the settings.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HistorySummary {
    pub count: i64,
    /// Oldest entry (`YYYY-MM-DD HH:MM:SS`, UTC), if any.
    pub since: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LikedTrack {
    pub track_key: String,
    pub artist: Option<String>,
    pub title: String,
    pub raw_title: String,
    /// Station the track was liked on.
    pub station_id: String,
    /// The name the user gave the station, else its name when the track was liked.
    pub station_name: Option<String>,
    pub liked_at: String,
    pub search_query: String,
}

/// The name to log a station under: the one given, else that of the favorite or personal station
/// of the same ID. Rows logged before the station became known are named when it does (see
/// `add_favorite`; a personal station gets a new ID, so no row predates it).
fn known_station_name(
    conn: &rusqlite::Connection,
    station_id: &str,
    given: Option<&str>,
) -> rusqlite::Result<Option<String>> {
    if let Some(name) = given {
        return Ok(Some(name.to_owned()));
    }
    conn.query_row(
        "SELECT COALESCE(
             (SELECT name FROM favorites WHERE station_id = ?1),
             (SELECT name FROM personal_stations WHERE station_id = ?1))",
        params![station_id],
        |r| r.get(0),
    )
}

/// `LIKE` pattern matching `text` anywhere, with the wildcards in `text` escaped (`ESCAPE '\'`).
fn contains_pattern(text: &str) -> String {
    let escaped = text
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The last row of the history, if it is the same track on the same station (same track key or
/// same raw title): a title announced several times in a row, while listening goes on, is one
/// entry. Returns its ID, its
/// track key and whether the source gave its artist and title.
fn same_as_last(
    conn: &rusqlite::Connection,
    station_id: &str,
    track_title: &str,
    track_key: &str,
) -> rusqlite::Result<Option<(i64, Option<String>, bool)>> {
    conn.query_row(
        "SELECT id, track_key, title_source IS NOT NULL FROM
             (SELECT * FROM listening_history ORDER BY id DESC LIMIT 1)
         WHERE station_id = ?1 AND (track_key = ?3 OR lower(trim(track_title)) = lower(trim(?2)))",
        params![station_id, track_title, track_key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
}

/// Gives a row the artist and title its source gave apart; a like given to it follows the new key.
fn take_parts(
    tx: &rusqlite::Transaction<'_>,
    id: i64,
    old_key: Option<&str>,
    parsed: &ParsedTitle,
) -> rusqlite::Result<()> {
    let new_key = tracks::track_key(parsed);
    tx.execute(
        "UPDATE listening_history SET artist = ?2, title = ?3, track_key = ?4, title_source = 'source'
         WHERE id = ?1",
        params![id, parsed.artist, parsed.title, new_key],
    )?;
    if old_key != Some(new_key.as_str()) {
        let moved = tx.execute(
            "UPDATE OR IGNORE likes SET track_key = ?2, artist = ?3, title = ?4 WHERE history_id = ?1",
            params![id, new_key, parsed.artist, parsed.title],
        )?;
        if moved == 0 {
            // The new track was already liked: one like is enough.
            tx.execute(
                "DELETE FROM likes WHERE history_id = ?1 AND track_key != ?2",
                params![id, new_key],
            )?;
        }
    }
    Ok(())
}

impl Database {
    /// Logs a title announced on air, unless listening went on since the last entry (`continuing`)
    /// and it is already that track on that station (see `same_as_last`). After a stop, the same
    /// track is heard again: it gets its own entry. Returns whether a row was added.
    pub fn log_track(
        &self,
        station_id: &str,
        station_name: Option<&str>,
        track_title: &str,
        continuing: bool,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn();
        let station_name = known_station_name(&conn, station_id, station_name)?;
        let parsed = tracks::parse(track_title, station_name.as_deref());
        let key = tracks::track_key(&parsed);
        if continuing && same_as_last(&conn, station_id, track_title, &key)?.is_some() {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO listening_history (station_id, station_name, track_title, artist, title, track_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                station_id,
                station_name,
                track_title,
                parsed.artist,
                parsed.title,
                key
            ],
        )?;
        Ok(true)
    }

    /// A track whose source gives artist and title apart (station page, stream tags): stored as
    /// given, never re-parsed. `played_at`: when it started, if the source says
    /// (`YYYY-MM-DD HH:MM:SS`, UTC). If listening went on since the last entry (`continuing`) and
    /// it is that track on that station, logged from its raw title, it takes the parts instead of
    /// a second row. Returns whether a row was added.
    pub fn log_structured_track(
        &self,
        station_id: &str,
        station_name: Option<&str>,
        track_title: &str,
        parsed: &ParsedTitle,
        played_at: Option<&str>,
        continuing: bool,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let key = tracks::track_key(parsed);
        let last = if continuing {
            same_as_last(&tx, station_id, track_title, &key)?
        } else {
            None
        };
        if let Some((id, old_key, structured)) = last {
            if !structured {
                take_parts(&tx, id, old_key.as_deref(), parsed)?;
            }
            tx.commit()?;
            return Ok(false);
        }
        let station_name = known_station_name(&tx, station_id, station_name)?;
        tx.execute(
            "INSERT INTO listening_history
                 (station_id, station_name, track_title, artist, title, track_key, title_source, played_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'source', COALESCE(?7, CURRENT_TIMESTAMP))",
            params![
                station_id,
                station_name,
                track_title,
                parsed.artist,
                parsed.title,
                key,
                played_at
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The source gave artist and title apart after the row was logged from the raw title (a
    /// server API confirming an ICY title): the latest row of that title on that station takes
    /// them and is no longer re-parsed. A like given in the meantime follows the new track key.
    pub fn set_track_parts(
        &self,
        station_id: &str,
        track_title: &str,
        parsed: &ParsedTitle,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT id, track_key FROM listening_history
                 WHERE station_id = ?1 AND track_title = ?2 AND title_source IS NULL
                 ORDER BY id DESC LIMIT 1",
                params![station_id, track_title],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((id, old_key)) = row else {
            return Ok(());
        };
        take_parts(&tx, id, old_key.as_deref(), parsed)?;
        tx.commit()
    }

    /// Newest first; `query` filters on the title, artist or station name.
    pub fn recent_history(
        &self,
        limit: u32,
        query: Option<&str>,
    ) -> rusqlite::Result<Vec<HistoryEntry>> {
        let pattern = query.filter(|q| !q.trim().is_empty()).map(contains_pattern);
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT h.id, h.station_id, COALESCE(n.name, h.station_name), h.track_title, h.artist,
                    h.title, h.track_key, h.played_at, l.track_key IS NOT NULL
             FROM listening_history h
             LEFT JOIN likes l ON l.track_key = h.track_key
             LEFT JOIN station_names n ON n.station_id = h.station_id
             WHERE ?2 IS NULL
                OR h.track_title LIKE ?2 ESCAPE '\\'
                OR h.artist LIKE ?2 ESCAPE '\\'
                OR h.station_name LIKE ?2 ESCAPE '\\'
                OR n.name LIKE ?2 ESCAPE '\\'
             ORDER BY h.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit, pattern], |row| {
            let parsed = parsed_from(row, 4, 5, 3)?;
            Ok(HistoryEntry {
                id: row.get(0)?,
                station_id: row.get(1)?,
                station_name: row.get(2)?,
                track_title: row.get(3)?,
                search_query: tracks::search_query(&parsed),
                artist: parsed.artist,
                title: parsed.title,
                track_key: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                played_at: row.get(7)?,
                liked: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn history_summary(&self) -> rusqlite::Result<HistorySummary> {
        self.conn().query_row(
            "SELECT COUNT(*), MIN(played_at) FROM listening_history",
            [],
            |row| {
                Ok(HistorySummary {
                    count: row.get(0)?,
                    since: row.get(1)?,
                })
            },
        )
    }

    /// Deletes the whole listening history (likes are kept), along with the station levels learned
    /// for Constant volume, which say which stations were heard too. Both go in one transaction.
    /// Returns the number of history rows removed.
    pub fn clear_history(&self) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM station_loudness", [])?;
        let removed = tx.execute("DELETE FROM listening_history", [])?;
        tx.commit()?;
        Ok(removed)
    }

    /// Likes the track of a history entry, recording the station it was heard on. Liking a track
    /// that is already liked keeps the first like.
    pub fn like_track(&self, history_id: i64) -> rusqlite::Result<bool> {
        let inserted = self.conn().execute(
            "INSERT INTO likes (track_key, artist, title, raw_title, station_id, station_name, history_id)
             SELECT track_key, artist, title, track_title, station_id, station_name, id
             FROM listening_history
             WHERE id = ?1 AND track_key IS NOT NULL
             ON CONFLICT(track_key) DO NOTHING",
            params![history_id],
        )?;
        Ok(inserted > 0)
    }

    pub fn unlike_track(&self, track_key: &str) -> rusqlite::Result<bool> {
        let deleted = self
            .conn()
            .execute("DELETE FROM likes WHERE track_key = ?1", params![track_key])?;
        Ok(deleted > 0)
    }

    /// Newest first; `query` filters on the title, artist or station name.
    pub fn list_likes(&self, limit: u32, query: Option<&str>) -> rusqlite::Result<Vec<LikedTrack>> {
        let pattern = query.filter(|q| !q.trim().is_empty()).map(contains_pattern);
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT k.track_key, k.artist, k.title, k.raw_title, k.station_id,
                    COALESCE(n.name, k.station_name), k.liked_at
             FROM likes k
             LEFT JOIN station_names n ON n.station_id = k.station_id
             WHERE ?2 IS NULL
                OR k.raw_title LIKE ?2 ESCAPE '\\'
                OR k.artist LIKE ?2 ESCAPE '\\'
                OR k.station_name LIKE ?2 ESCAPE '\\'
                OR n.name LIKE ?2 ESCAPE '\\'
             ORDER BY k.liked_at DESC, k.rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit, pattern], |row| {
            let parsed = ParsedTitle {
                artist: row.get(1)?,
                title: row.get(2)?,
            };
            Ok(LikedTrack {
                track_key: row.get(0)?,
                search_query: tracks::search_query(&parsed),
                artist: parsed.artist,
                title: parsed.title,
                raw_title: row.get(3)?,
                station_id: row.get(4)?,
                station_name: row.get(5)?,
                liked_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Re-parses the stored titles when the parsing rules changed (or were never applied).
    pub(super) fn reparse_history_if_outdated(&self) -> rusqlite::Result<()> {
        let current = tracks::PARSER_VERSION.to_string();
        if self.get_str(SETTING_PARSER_VERSION)?.as_deref() == Some(current.as_str()) {
            return Ok(());
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut select = tx.prepare(
                "SELECT id, track_title, station_name FROM listening_history
                 WHERE title_source IS NULL",
            )?;
            let rows: Vec<(i64, String, Option<String>)> = select
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut update = tx.prepare(
                "UPDATE listening_history SET artist = ?2, title = ?3, track_key = ?4 WHERE id = ?1",
            )?;
            for (id, raw, station) in rows {
                let parsed = tracks::parse(&raw, station.as_deref());
                update.execute(params![
                    id,
                    parsed.artist,
                    parsed.title,
                    tracks::track_key(&parsed)
                ])?;
            }

            // Likes follow the new keys; two likes that now mean the same track become one.
            let mut select = tx.prepare("SELECT track_key, raw_title, station_name FROM likes")?;
            let likes: Vec<(String, String, Option<String>)> = select
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (old_key, raw, station) in likes {
                let parsed = tracks::parse(&raw, station.as_deref());
                let new_key = tracks::track_key(&parsed);
                if new_key == old_key {
                    continue;
                }
                let moved = tx.execute(
                    "UPDATE OR IGNORE likes SET track_key = ?2, artist = ?3, title = ?4 WHERE track_key = ?1",
                    params![old_key, new_key, parsed.artist, parsed.title],
                )?;
                if moved == 0 {
                    tx.execute("DELETE FROM likes WHERE track_key = ?1", params![old_key])?;
                }
            }
        }
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SETTING_PARSER_VERSION, current],
        )?;
        tx.commit()
    }
}

/// Parsed title from stored columns; rows not parsed yet fall back to the raw title.
fn parsed_from(
    row: &Row<'_>,
    artist: usize,
    title: usize,
    raw: usize,
) -> rusqlite::Result<ParsedTitle> {
    Ok(match row.get::<_, Option<String>>(title)? {
        Some(title) => ParsedTitle {
            artist: row.get(artist)?,
            title,
        },
        None => ParsedTitle {
            artist: None,
            title: row.get(raw)?,
        },
    })
}

#[cfg(test)]
mod structured_tests {
    use super::*;

    #[test]
    fn structured_tracks_keep_their_fields_and_start() {
        let db = Database::open_in_memory().unwrap();
        let parsed = ParsedTitle {
            artist: Some("MELLOW MOOD / ANTHONY B".into()),
            title: "Home - Abroad".into(),
        };
        db.log_structured_track(
            "f7",
            Some("Fréquence 7"),
            "MELLOW MOOD / ANTHONY B - Home - Abroad",
            &parsed,
            Some("2026-11-04 21:36:51"),
            true,
        )
        .unwrap();
        db.log_track("f7", Some("Fréquence 7"), "Air - Sexy Boy", true)
            .unwrap();
        // A parser change re-parses ICY titles only.
        db.set_str(SETTING_PARSER_VERSION, "0").unwrap();
        db.reparse_history_if_outdated().unwrap();

        let history = db.recent_history(10, None).unwrap();
        let structured = history
            .iter()
            .find(|e| e.station_id == "f7" && e.title == "Home - Abroad")
            .unwrap();
        assert_eq!(
            structured.artist.as_deref(),
            Some("MELLOW MOOD / ANTHONY B")
        );
        assert_eq!(structured.played_at, "2026-11-04 21:36:51");
        assert!(history.iter().any(|e| e.title == "Sexy Boy"));
    }

    #[test]
    fn icy_rows_take_the_parts_a_server_api_gives() {
        let db = Database::open_in_memory().unwrap();
        // Parsed from the raw title: the track number is taken for the artist.
        db.log_track(
            "wc",
            Some("West Coast"),
            "082 - Dru Down - Pimp of the Year",
            true,
        )
        .unwrap();
        let id = db.recent_history(1, None).unwrap()[0].id;
        assert!(db.like_track(id).unwrap());

        let parsed = ParsedTitle {
            artist: Some("Dru Down".into()),
            title: "Pimp of the Year".into(),
        };
        db.set_track_parts("wc", "082 - Dru Down - Pimp of the Year", &parsed)
            .unwrap();
        let entry = &db.recent_history(1, None).unwrap()[0];
        assert_eq!(
            (entry.artist.as_deref(), entry.title.as_str()),
            (Some("Dru Down"), "Pimp of the Year")
        );
        assert_eq!(entry.track_title, "082 - Dru Down - Pimp of the Year"); // the raw title stays
        assert!(entry.liked, "the like follows the new track key");

        // Kept through a parser change, and a second call changes nothing.
        db.set_str(SETTING_PARSER_VERSION, "0").unwrap();
        db.reparse_history_if_outdated().unwrap();
        db.set_track_parts("wc", "082 - Dru Down - Pimp of the Year", &parsed)
            .unwrap();
        assert_eq!(
            db.recent_history(1, None).unwrap()[0].artist.as_deref(),
            Some("Dru Down")
        );
        // Another station's row is not touched.
        db.set_track_parts("other", "082 - Dru Down - Pimp of the Year", &parsed)
            .unwrap();
        assert_eq!(db.history_summary().unwrap().count, 1);
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::super::tests::fav;
    use super::*;

    #[test]
    fn history_is_newest_first_and_parsed() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("a", Some("Radio A"), "Air - Sexy Boy", true)
            .unwrap();
        db.log_track("a", Some("Radio A"), "Moby - Porcelain - Radio A", true)
            .unwrap();
        let history = db.recent_history(10, None).unwrap();
        assert_eq!(history[0].artist.as_deref(), Some("Moby"));
        assert_eq!(history[0].title, "Porcelain");
        assert_eq!(history[0].search_query, "Moby Porcelain");
        assert_eq!(history[0].station_name.as_deref(), Some("Radio A"));
        assert_eq!(db.recent_history(1, None).unwrap().len(), 1);
        assert_eq!(db.history_summary().unwrap().count, 2);
        assert_eq!(db.clear_history().unwrap(), 2);
        assert_eq!(
            db.history_summary().unwrap(),
            HistorySummary {
                count: 0,
                since: None
            }
        );
        assert!(db.recent_history(10, None).unwrap().is_empty());
    }

    #[test]
    fn the_name_given_to_a_station_shows_in_the_history_and_likes() {
        let db = Database::open_in_memory().unwrap();
        db.log_track(
            "a",
            Some("SomaFM Groove Salad (128k MP3)"),
            "Air - Sexy Boy",
            true,
        )
        .unwrap();
        let id = db.recent_history(1, None).unwrap()[0].id;
        assert!(db.like_track(id).unwrap());
        db.set_station_name("a", Some("Groove Salad")).unwrap();

        let shown = |db: &Database| {
            (
                db.recent_history(1, None).unwrap()[0].station_name.clone(),
                db.list_likes(1, None).unwrap()[0].station_name.clone(),
            )
        };
        let renamed = Some("Groove Salad".to_owned());
        assert_eq!(shown(&db), (renamed.clone(), renamed));
        // Both names find the station.
        for query in ["groove salad", "128k"] {
            assert_eq!(
                db.recent_history(10, Some(query)).unwrap().len(),
                1,
                "{query}"
            );
            assert_eq!(db.list_likes(10, Some(query)).unwrap().len(), 1, "{query}");
        }
        // The stored name is the original one, which the title parser relies on.
        let stored: String = db
            .conn()
            .query_row("SELECT station_name FROM listening_history", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, "SomaFM Groove Salad (128k MP3)");

        db.set_station_name("a", Some("  ")).unwrap();
        let original = Some("SomaFM Groove Salad (128k MP3)".to_owned());
        assert_eq!(shown(&db), (original.clone(), original));
    }

    #[test]
    fn a_track_repeated_in_a_row_is_logged_once() {
        let db = Database::open_in_memory().unwrap();
        assert!(db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy", true)
            .unwrap());
        // Repeated in a row, spelled differently or with the station suffix.
        assert!(!db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy", true)
            .unwrap());
        assert!(!db
            .log_track("a", Some("Radio A"), "AIR - Sexy Boy", true)
            .unwrap());
        assert!(!db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy - Radio A", true)
            .unwrap());
        // Not in a row: something else came in between, on this station or another one.
        assert!(db
            .log_track(
                "a",
                Some("Radio A"),
                "Écoutez Radio A tous les matins",
                true
            )
            .unwrap());
        assert!(db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy", true)
            .unwrap());
        assert!(db
            .log_track("b", Some("Radio B"), "Air - Sexy Boy", true)
            .unwrap());
        assert!(db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy", true)
            .unwrap());
        // Listening stopped, then came back to the same track: heard again.
        assert!(db
            .log_track("a", Some("Radio A"), "Air - Sexy Boy", false)
            .unwrap());
        assert_eq!(db.history_summary().unwrap().count, 6);
    }

    #[test]
    fn a_structured_title_completes_the_entry_logged_from_icy() {
        let db = Database::open_in_memory().unwrap();
        db.log_track(
            "wc",
            Some("West Coast"),
            "082 - Dru Down - Pimp of the Year",
            true,
        )
        .unwrap();
        let parsed = ParsedTitle {
            artist: Some("Dru Down".into()),
            title: "Pimp of the Year".into(),
        };
        // The API confirms it under another spelling of the raw title.
        let raw = "082 - Dru Down - Pimp of the Year";
        assert!(!db
            .log_structured_track("wc", Some("West Coast"), raw, &parsed, None, true)
            .unwrap());
        let history = db.recent_history(10, None).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].artist.as_deref(), Some("Dru Down"));
        // Then the raw ICY title again, or the source's own spelling: still the same entry.
        assert!(!db
            .log_track(
                "wc",
                Some("West Coast"),
                "082 - Dru Down - Pimp of the Year",
                true
            )
            .unwrap());
        assert!(!db
            .log_structured_track(
                "wc",
                None,
                "Dru Down - Pimp of the Year",
                &parsed,
                None,
                true
            )
            .unwrap());
        assert_eq!(db.history_summary().unwrap().count, 1);
    }

    #[test]
    fn history_can_be_searched() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("a", Some("FIP"), "Air - Sexy Boy", true)
            .unwrap();
        db.log_track("b", Some("Nova"), "Moby - Porcelain", true)
            .unwrap();
        db.log_track("b", Some("Nova"), "100% Pure_Love", true)
            .unwrap();
        let titles = |q: &str| {
            db.recent_history(10, Some(q))
                .unwrap()
                .into_iter()
                .map(|e| e.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("air"), ["Sexy Boy"]);
        assert_eq!(titles("nova").len(), 2);
        // Wildcards are literal.
        assert_eq!(titles("100%"), ["100% Pure_Love"]);
        assert!(titles("_x").is_empty());
        assert_eq!(titles("  ").len(), 3);
    }

    #[test]
    fn history_without_a_name_takes_the_favorite_name() {
        let db = Database::open_in_memory().unwrap();
        db.add_favorite(&fav("a")).unwrap();
        db.log_track("a", None, "title", true).unwrap();
        db.log_track("b", None, "title", true).unwrap();
        let history = db.recent_history(10, None).unwrap();
        assert_eq!(history[0].station_name, None);
        assert_eq!(history[1].station_name.as_deref(), Some("Radio a"));
        // Stored with the row: renaming or removing the favorite later leaves it.
        db.remove_favorite("a").unwrap();
        let history = db.recent_history(10, None).unwrap();
        assert_eq!(history[1].station_name.as_deref(), Some("Radio a"));
    }

    #[test]
    fn unnamed_history_takes_the_name_of_a_new_favorite() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("a", None, "title", true).unwrap();
        db.log_track("a", Some("On air"), "other", true).unwrap();
        db.log_track("b", None, "title", true).unwrap();
        db.add_favorite(&fav("a")).unwrap();
        let names: Vec<_> = db
            .recent_history(10, None)
            .unwrap()
            .into_iter()
            .map(|e| e.station_name)
            .collect();
        // Newest first: another station, a name given on air, the unnamed row.
        assert_eq!(names, [None, Some("On air".into()), Some("Radio a".into())]);
    }

    #[test]
    fn likes_are_per_track_and_survive_clearing_the_history() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("fip", Some("FIP"), "Air - Sexy Boy", true)
            .unwrap();
        db.log_track("nova", Some("Nova"), "AIR - Sexy Boy (Remastered)", true)
            .unwrap();
        let history = db.recent_history(10, None).unwrap();
        assert!(db.like_track(history[1].id).unwrap());
        // Same track heard elsewhere: still the first like, on the first station.
        assert!(!db.like_track(history[0].id).unwrap());
        assert!(db.recent_history(10, None).unwrap().iter().all(|e| e.liked));

        db.clear_history().unwrap();
        let likes = db.list_likes(10, None).unwrap();
        assert_eq!(likes.len(), 1);
        assert_eq!(likes[0].station_name.as_deref(), Some("FIP"));
        assert_eq!(likes[0].artist.as_deref(), Some("Air"));
        assert_eq!(db.list_likes(10, Some("fip")).unwrap().len(), 1);
        assert!(db.list_likes(10, Some("nova")).unwrap().is_empty());

        assert!(db.unlike_track(&likes[0].track_key).unwrap());
        assert!(db.list_likes(10, None).unwrap().is_empty());
    }

    #[test]
    fn old_history_is_migrated_and_reparsed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE listening_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 station_id TEXT NOT NULL,
                 track_title TEXT NOT NULL,
                 played_at DATETIME DEFAULT CURRENT_TIMESTAMP
             );
             INSERT INTO listening_history (station_id, track_title) VALUES ('a', 'Air - Sexy Boy');",
        )
        .unwrap();
        let db = Database::init(conn).unwrap();
        db.log_track("a", Some("Radio A"), "new", true).unwrap();
        let history = db.recent_history(10, None).unwrap();
        assert_eq!(history[0].station_name.as_deref(), Some("Radio A"));
        assert_eq!(history[1].station_name, None);
        assert_eq!(history[1].artist.as_deref(), Some("Air"));
        assert!(db.like_track(history[1].id).unwrap());
    }

    #[test]
    fn reparsing_rekeys_and_merges_likes() {
        let db = Database::open_in_memory().unwrap();
        db.log_track("a", None, "Air - Sexy Boy", true).unwrap();
        db.log_track("a", None, "AIR - Sexy Boy", true).unwrap();
        let conn = db.conn();
        // Likes stored under outdated keys, as if made with older parsing rules.
        conn.execute_batch(
            "INSERT INTO likes (track_key, title, raw_title, station_id) VALUES
                 ('old-1', 'x', 'Air - Sexy Boy', 'a'),
                 ('old-2', 'x', 'AIR - Sexy Boy', 'a');
             DELETE FROM settings WHERE key = 'title_parser_version';",
        )
        .unwrap();
        drop(conn);
        db.reparse_history_if_outdated().unwrap();
        let likes = db.list_likes(10, None).unwrap();
        assert_eq!(likes.len(), 1);
        assert_eq!(likes[0].track_key, "air|sexy boy");
        assert_eq!(likes[0].title, "Sexy Boy");
    }
}
