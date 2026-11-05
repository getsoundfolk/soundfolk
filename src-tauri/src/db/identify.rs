// SPDX-License-Identifier: MPL-2.0
//! Identification of liked tracks (MusicBrainz): the work queue and the cache of results.

use rusqlite::{params, OptionalExtension};

use super::Database;
use crate::musicbrainz::{ArtistInfo, RecordingMatch};

/// A liked track waiting to be identified.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackToIdentify {
    pub track_key: String,
    pub artist: Option<String>,
    pub title: String,
}

impl Database {
    /// Oldest liked track never looked up, or whose retry delay has passed.
    pub fn next_track_to_identify(&self) -> rusqlite::Result<Option<TrackToIdentify>> {
        self.conn()
            .query_row(
                "SELECT l.track_key, l.artist, l.title
                 FROM likes l
                 LEFT JOIN tracks t ON t.track_key = l.track_key
                 WHERE t.track_key IS NULL
                    OR (t.status = 'pending' AND t.next_attempt_at <= CURRENT_TIMESTAMP)
                 ORDER BY l.liked_at, l.rowid
                 LIMIT 1",
                [],
                |r| Ok(TrackToIdentify { track_key: r.get(0)?, artist: r.get(1)?, title: r.get(2)? }),
            )
            .optional()
    }

    pub fn track_not_found(&self, track_key: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO tracks (track_key, status, looked_up_at) VALUES (?1, 'not_found', CURRENT_TIMESTAMP)
             ON CONFLICT(track_key) DO UPDATE SET status = 'not_found', looked_up_at = CURRENT_TIMESTAMP",
            params![track_key],
        )?;
        Ok(())
    }

    /// The lookup failed (network, rate limit): retry later, waiting twice as long each time
    /// (2 minutes, 4, 8… up to a day).
    pub fn postpone_track(&self, track_key: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO tracks (track_key, status, attempts, next_attempt_at)
             VALUES (?1, 'pending', 1, datetime('now', '+2 minutes'))
             ON CONFLICT(track_key) DO UPDATE SET
                 attempts = attempts + 1,
                 next_attempt_at = datetime('now', '+' || min(1440, 1 << min(attempts + 1, 11)) || ' minutes')",
            params![track_key],
        )?;
        Ok(())
    }

    pub fn save_track_match(&self, track_key: &str, found: &RecordingMatch) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO artists (mbid, name) VALUES (?1, ?2) ON CONFLICT(mbid) DO NOTHING",
            params![found.artist_mbid, found.artist_name],
        )?;
        if let Some(album) = &found.album {
            tx.execute(
                "INSERT INTO albums (mbid, title, primary_type) VALUES (?1, ?2, ?3)
                 ON CONFLICT(mbid) DO UPDATE SET title = excluded.title, primary_type = excluded.primary_type",
                params![album.mbid, album.title, album.primary_type],
            )?;
        }
        tx.execute(
            "INSERT INTO tracks (track_key, status, recording_mbid, title, artist_mbid, album_mbid,
                                 first_release_date, tags, looked_up_at)
             VALUES (?1, 'matched', ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP)
             ON CONFLICT(track_key) DO UPDATE SET
                 status = 'matched', recording_mbid = excluded.recording_mbid, title = excluded.title,
                 artist_mbid = excluded.artist_mbid, album_mbid = excluded.album_mbid,
                 first_release_date = excluded.first_release_date, tags = excluded.tags,
                 looked_up_at = excluded.looked_up_at",
            params![
                track_key,
                found.recording_mbid,
                found.title,
                found.artist_mbid,
                found.album.as_ref().map(|a| &a.mbid),
                found.first_release_date,
                serde_json::to_string(&found.tags).unwrap_or_default(),
            ],
        )?;
        tx.commit()
    }

    /// The artist's country and genres were never fetched.
    pub fn artist_needs_lookup(&self, mbid: &str) -> rusqlite::Result<bool> {
        let looked_up: Option<bool> = self
            .conn()
            .query_row("SELECT looked_up_at IS NOT NULL FROM artists WHERE mbid = ?1", params![mbid], |r| r.get(0))
            .optional()?;
        Ok(!looked_up.unwrap_or(false))
    }

    pub fn save_artist(&self, artist: &ArtistInfo) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO artists (mbid, name, country, genres, looked_up_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(mbid) DO UPDATE SET
                 name = excluded.name, country = excluded.country, genres = excluded.genres,
                 looked_up_at = excluded.looked_up_at",
            params![
                artist.mbid,
                artist.name,
                artist.country,
                serde_json::to_string(&artist.genres).unwrap_or_default()
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::Album;

    fn liked(db: &Database, raw: &str) -> String {
        db.log_track("s", Some("Station"), raw, true).unwrap();
        let entry = db.recent_history(1, None).unwrap().remove(0);
        db.like_track(entry.id).unwrap();
        entry.track_key
    }

    fn found() -> RecordingMatch {
        RecordingMatch {
            recording_mbid: "rec".into(),
            title: "Sexy Boy".into(),
            artist_mbid: "air".into(),
            artist_name: "Air".into(),
            album: Some(Album { mbid: "mw".into(), title: "Moon Safari".into(), primary_type: Some("Album".into()) }),
            first_release_date: Some("1998-01-16".into()),
            tags: vec!["downtempo".into()],
        }
    }

    #[test]
    fn queue_takes_liked_tracks_once() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.next_track_to_identify().unwrap(), None);
        let air = liked(&db, "Air - Sexy Boy");
        let moby = liked(&db, "Moby - Porcelain");
        assert_eq!(db.next_track_to_identify().unwrap().unwrap().track_key, air);

        db.save_track_match(&air, &found()).unwrap();
        assert_eq!(db.next_track_to_identify().unwrap().unwrap().track_key, moby);
        // A failed lookup waits before being retried.
        db.postpone_track(&moby).unwrap();
        assert_eq!(db.next_track_to_identify().unwrap(), None);
        db.postpone_track(&moby).unwrap();
        let attempts: i64 =
            db.conn().query_row("SELECT attempts FROM tracks WHERE track_key = ?1", [&moby], |r| r.get(0)).unwrap();
        assert_eq!(attempts, 2);
    }

    #[test]
    fn artists_are_looked_up_once() {
        let db = Database::open_in_memory().unwrap();
        let air = liked(&db, "Air - Sexy Boy");
        db.save_track_match(&air, &found()).unwrap();
        assert!(db.artist_needs_lookup("air").unwrap());
        db.save_artist(&ArtistInfo {
            mbid: "air".into(),
            name: "Air".into(),
            country: Some("FR".into()),
            genres: vec!["electronic".into()],
        })
        .unwrap();
        assert!(!db.artist_needs_lookup("air").unwrap());
        // A second match of the same artist keeps what was looked up.
        db.save_track_match(&air, &found()).unwrap();
        assert!(!db.artist_needs_lookup("air").unwrap());
    }
}
