// SPDX-License-Identifier: MPL-2.0
//! Personal listening stats, computed from the history, likes, sessions and identified tracks.
//!
//! Artist, album, genre, decade and country figures only count tracks identified on MusicBrainz
//! (parsed names are not reliable enough to group on); the number of liked tracks left out is
//! reported next to them. Station figures rely on station IDs and are always reliable.

use rusqlite::{params, Connection};
use serde::Serialize;

use super::Database;
use crate::musicbrainz::SETTING_IDENTIFY;

/// Top entries kept per ranking.
const TOP: u32 = 10;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Stats {
    pub totals: Totals,
    pub identification: Identification,
    pub stations: Vec<StationStats>,
    pub artists: Vec<Count>,
    pub albums: Vec<Count>,
    /// Stations that brought artists liked for the first time.
    pub discovery_stations: Vec<Count>,
    /// Artists liked for the first time, per month (`YYYY-MM`, local time), oldest first.
    pub new_artists_by_month: Vec<Count>,
    /// Tracks heard on several stations (count = number of stations).
    pub shared_tracks: Vec<Count>,
    /// Listening hours per day of the week, Monday first (local time).
    pub hours_by_weekday: [f64; 7],
    /// Listening hours per hour of the day (local time).
    pub hours_by_hour: [f64; 24],
    pub decades: Vec<Count>,
    pub genres: Vec<Count>,
    /// ISO 3166-1 codes.
    pub countries: Vec<Count>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Totals {
    pub tracks_heard: u32,
    pub likes: u32,
    pub listening_hours: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Identification {
    pub enabled: bool,
    /// Liked tracks (in the period) identified on MusicBrainz, not found, or not looked up yet.
    pub matched: u32,
    pub not_found: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StationStats {
    pub station_id: String,
    pub name: String,
    pub likes: u32,
    pub tracks_heard: u32,
    pub listening_hours: f64,
    /// Likes per hour of listening (`None` without listening time).
    pub likes_per_hour: Option<f64>,
    /// Likes ÷ tracks heard (`None` if no track was heard).
    pub like_rate: Option<f64>,
    /// Plays per distinct track: how often the same tracks come back.
    pub repetition: Option<f64>,
    /// Distinct artists per 100 tracks heard.
    pub diversity: Option<f64>,
    /// Share of titles that could not be split into artist and title (ads, jingles, no metadata).
    pub unrecognised_share: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Count {
    pub label: String,
    /// Second line (an album's artist…).
    pub detail: Option<String>,
    pub count: u32,
}

/// SQL condition: `column` within the last `?1` days (`?1` NULL = all time).
fn since(column: &str) -> String {
    format!("(?1 IS NULL OR {column} >= datetime('now', '-' || ?1 || ' days'))")
}

/// Liked tracks identified on MusicBrainz, with their artist.
const MATCHED_LIKES: &str = "likes l
     JOIN tracks t ON t.track_key = l.track_key AND t.status = 'matched'
     JOIN artists a ON a.mbid = t.artist_mbid";

/// First like of each identified artist, with the station it happened on.
const FIRST_LIKES: &str = "WITH firsts AS (
         SELECT a.mbid, l.station_id, l.station_name, l.liked_at,
                ROW_NUMBER() OVER (PARTITION BY a.mbid ORDER BY l.liked_at, l.rowid) AS rank
         FROM likes l
         JOIN tracks t ON t.track_key = l.track_key AND t.status = 'matched'
         JOIN artists a ON a.mbid = t.artist_mbid
     )";

impl Database {
    /// Stats over the last `days` days, or all time.
    pub fn stats(&self, days: Option<u32>) -> rusqlite::Result<Stats> {
        let enabled = self.get_bool(SETTING_IDENTIFY, false)?;
        let conn = self.conn();
        let (hours_by_weekday, hours_by_hour) = listening_profile(&conn, days)?;
        let stations = station_stats(&conn, days)?;
        Ok(Stats {
            totals: Totals {
                tracks_heard: stations.iter().map(|s| s.tracks_heard).sum(),
                likes: stations.iter().map(|s| s.likes).sum(),
                listening_hours: hours_by_hour.iter().sum(),
            },
            identification: identification(&conn, days, enabled)?,
            stations,
            artists: counts(
                &conn,
                &format!(
                    "SELECT a.name, NULL, COUNT(*) FROM {MATCHED_LIKES} WHERE {}
                     GROUP BY a.mbid ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("l.liked_at")
                ),
                days,
            )?,
            albums: counts(
                &conn,
                &format!(
                    "SELECT al.title, a.name, COUNT(*) FROM {MATCHED_LIKES}
                     JOIN albums al ON al.mbid = t.album_mbid
                     WHERE {} GROUP BY al.mbid ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("l.liked_at")
                ),
                days,
            )?,
            discovery_stations: counts(
                &conn,
                &format!(
                    "{FIRST_LIKES}
                     SELECT COALESCE(station_name, station_id), NULL, COUNT(*) FROM firsts
                     WHERE rank = 1 AND {} GROUP BY station_id ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("liked_at")
                ),
                days,
            )?,
            new_artists_by_month: counts(
                &conn,
                &format!(
                    "{FIRST_LIKES}
                     SELECT strftime('%Y-%m', liked_at, 'localtime') AS month, NULL, COUNT(*) FROM firsts
                     WHERE rank = 1 AND {} GROUP BY month ORDER BY month DESC LIMIT 12",
                    since("liked_at")
                ),
                days,
            )?
            .into_iter()
            .rev()
            .collect(),
            shared_tracks: counts(
                &conn,
                &format!(
                    "SELECT MAX(title), MAX(artist), COUNT(DISTINCT station_id) AS stations
                     FROM listening_history
                     WHERE track_key IS NOT NULL AND artist IS NOT NULL AND {}
                     GROUP BY track_key HAVING stations >= 2 ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("played_at")
                ),
                days,
            )?,
            hours_by_weekday,
            hours_by_hour,
            decades: counts(
                &conn,
                &format!(
                    "SELECT substr(t.first_release_date, 1, 3) || '0s' AS decade, NULL, COUNT(*)
                     FROM {MATCHED_LIKES}
                     WHERE length(t.first_release_date) >= 4 AND {} GROUP BY decade ORDER BY decade",
                    since("l.liked_at")
                ),
                days,
            )?,
            // The artist's genres when known, else the recording's tags.
            genres: counts(
                &conn,
                &format!(
                    "SELECT g.value, NULL, COUNT(*) FROM {MATCHED_LIKES},
                         json_each(CASE WHEN a.genres IS NOT NULL AND a.genres <> '[]' THEN a.genres
                                        ELSE COALESCE(t.tags, '[]') END) g
                     WHERE {} GROUP BY g.value ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("l.liked_at")
                ),
                days,
            )?,
            countries: counts(
                &conn,
                &format!(
                    "SELECT a.country, NULL, COUNT(*) FROM {MATCHED_LIKES}
                     WHERE a.country IS NOT NULL AND {} GROUP BY a.country ORDER BY 3 DESC, 1 LIMIT {TOP}",
                    since("l.liked_at")
                ),
                days,
            )?,
        })
    }
}

fn counts(conn: &Connection, sql: &str, days: Option<u32>) -> rusqlite::Result<Vec<Count>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![days], |r| Ok(Count { label: r.get(0)?, detail: r.get(1)?, count: r.get(2)? }))?;
    rows.collect()
}

fn identification(conn: &Connection, days: Option<u32>, enabled: bool) -> rusqlite::Result<Identification> {
    conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(t.status = 'matched'), 0), COALESCE(SUM(t.status = 'not_found'), 0),
                    COALESCE(SUM(t.status IS NULL OR t.status = 'pending'), 0)
             FROM likes l LEFT JOIN tracks t ON t.track_key = l.track_key
             WHERE {}",
            since("l.liked_at")
        ),
        params![days],
        |r| Ok(Identification { enabled, matched: r.get(0)?, not_found: r.get(1)?, pending: r.get(2)? }),
    )
}

fn station_stats(conn: &Connection, days: Option<u32>) -> rusqlite::Result<Vec<StationStats>> {
    let sql = format!(
        "WITH heard AS (
             SELECT station_id, COUNT(*) AS plays, COUNT(DISTINCT track_key) AS tracks,
                    COUNT(DISTINCT lower(artist)) AS artists, SUM(artist IS NULL) AS unparsed
             FROM listening_history WHERE {history} GROUP BY station_id
         ),
         liked AS (SELECT station_id, COUNT(*) AS n FROM likes WHERE {likes} GROUP BY station_id),
         listened AS (
             SELECT station_id,
                    SUM(strftime('%s', COALESCE(ended_at, CURRENT_TIMESTAMP)) - strftime('%s', started_at)) AS secs
             FROM listening_sessions WHERE {sessions} GROUP BY station_id
         ),
         ids AS (SELECT station_id FROM heard UNION SELECT station_id FROM liked UNION SELECT station_id FROM listened)
         SELECT ids.station_id,
                COALESCE(
                    (SELECT station_name FROM listening_history h
                     WHERE h.station_id = ids.station_id AND station_name IS NOT NULL ORDER BY id DESC LIMIT 1),
                    (SELECT station_name FROM listening_sessions s
                     WHERE s.station_id = ids.station_id AND station_name IS NOT NULL ORDER BY id DESC LIMIT 1),
                    (SELECT station_name FROM likes k
                     WHERE k.station_id = ids.station_id AND station_name IS NOT NULL LIMIT 1),
                    (SELECT name FROM favorites f WHERE f.station_id = ids.station_id),
                    (SELECT name FROM personal_stations p WHERE p.station_id = ids.station_id),
                    ids.station_id),
                COALESCE(liked.n, 0), COALESCE(heard.plays, 0), COALESCE(heard.tracks, 0),
                COALESCE(heard.artists, 0), COALESCE(heard.unparsed, 0), COALESCE(listened.secs, 0)
         FROM ids
         LEFT JOIN heard ON heard.station_id = ids.station_id
         LEFT JOIN liked ON liked.station_id = ids.station_id
         LEFT JOIN listened ON listened.station_id = ids.station_id",
        history = since("played_at"),
        likes = since("liked_at"),
        sessions = since("started_at"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![days], |r| {
        let likes: u32 = r.get(2)?;
        let plays: u32 = r.get(3)?;
        let tracks: u32 = r.get(4)?;
        let artists: u32 = r.get(5)?;
        let unparsed: u32 = r.get(6)?;
        let hours = r.get::<_, i64>(7)?.max(0) as f64 / 3600.0;
        let ratio = |num: f64, den: f64| (den > 0.0).then(|| num / den);
        Ok(StationStats {
            station_id: r.get(0)?,
            name: r.get(1)?,
            likes,
            tracks_heard: plays,
            listening_hours: hours,
            // Under a minute of listening, a rate per hour means nothing.
            likes_per_hour: if hours >= 1.0 / 60.0 { Some(likes as f64 / hours) } else { None },
            like_rate: ratio(likes as f64, plays as f64),
            repetition: ratio(plays as f64, tracks as f64),
            diversity: ratio(artists as f64 * 100.0, (plays - unparsed) as f64),
            unrecognised_share: ratio(unparsed as f64, plays as f64),
        })
    })?;
    let mut stations = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    stations.sort_by(|a, b| {
        b.likes.cmp(&a.likes).then(b.listening_hours.total_cmp(&a.listening_hours)).then(a.name.cmp(&b.name))
    });
    Ok(stations)
}

/// Listening hours per weekday and per hour of the day, in local time.
fn listening_profile(conn: &Connection, days: Option<u32>) -> rusqlite::Result<([f64; 7], [f64; 24])> {
    // Offset of local time from UTC now (changes of daylight-saving time are ignored).
    let offset: i64 = conn.query_row(
        "SELECT strftime('%s', 'now', 'localtime') - strftime('%s', 'now')",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT strftime('%s', started_at), strftime('%s', COALESCE(ended_at, CURRENT_TIMESTAMP))
         FROM listening_sessions WHERE {}",
        since("started_at")
    ))?;
    let mut by_weekday = [0.0; 7];
    let mut by_hour = [0.0; 24];
    let rows = stmt.query_map(params![days], |r| {
        Ok((r.get::<_, String>(0)?.parse().unwrap_or(0), r.get::<_, String>(1)?.parse().unwrap_or(0)))
    })?;
    for row in rows {
        let (start, end): (i64, i64) = row?;
        spread(start + offset, end + offset, &mut by_weekday, &mut by_hour);
    }
    Ok((by_weekday, by_hour))
}

/// Adds the hours between two local Unix times to the weekday (Monday first) and hour buckets.
fn spread(start: i64, end: i64, by_weekday: &mut [f64; 7], by_hour: &mut [f64; 24]) {
    let mut t = start;
    while t < end {
        let next = (t.div_euclid(3600) + 1) * 3600;
        let chunk = (next.min(end) - t) as f64 / 3600.0;
        let day = t.div_euclid(86_400);
        // 1970-01-01 was a Thursday (index 3 when Monday is 0).
        by_weekday[(day + 3).rem_euclid(7) as usize] += chunk;
        by_hour[(t.rem_euclid(86_400) / 3600) as usize] += chunk;
        t = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::{Album, ArtistInfo, RecordingMatch};

    #[test]
    fn spreads_sessions_over_hours_and_days() {
        let (mut days, mut hours) = ([0.0; 7], [0.0; 24]);
        // 2026-11-02 (a Monday) 23:30 → Tuesday 01:00.
        let start = 1_793_662_200; // 2026-11-02T23:30:00Z
        spread(start, start + 5400, &mut days, &mut hours);
        assert_eq!(days[0], 0.5);
        assert_eq!(days[1], 1.0);
        assert_eq!(hours[23], 0.5);
        assert_eq!(hours[0], 1.0);
        assert_eq!(hours.iter().sum::<f64>(), 1.5);
    }

    fn like(db: &Database, station: &str, raw: &str) -> String {
        db.log_track(station, Some(&station.to_uppercase()), raw).unwrap();
        let entry = db.recent_history(1, None).unwrap().remove(0);
        db.like_track(entry.id).unwrap();
        entry.track_key
    }

    fn matched(db: &Database, key: &str, artist: &str, date: &str) {
        db.save_track_match(
            key,
            &RecordingMatch {
                recording_mbid: format!("rec-{key}"),
                title: key.into(),
                artist_mbid: artist.into(),
                artist_name: artist.to_uppercase(),
                album: Some(Album { mbid: format!("al-{artist}"), title: format!("{artist} album"), primary_type: None }),
                first_release_date: Some(date.into()),
                tags: vec!["rock".into()],
            },
        )
        .unwrap();
    }

    #[test]
    fn stats_rank_stations_and_identified_artists() {
        let db = Database::open_in_memory().unwrap();
        let air1 = like(&db, "fip", "Air - Sexy Boy");
        let air2 = like(&db, "nova", "Air - Kelly Watch the Stars");
        let moby = like(&db, "fip", "Moby - Porcelain");
        let _unidentified = like(&db, "fip", "Unknown - Song");
        db.log_track("nova", Some("NOVA"), "Air - Sexy Boy").unwrap();
        db.log_track("nova", Some("NOVA"), "Station jingle").unwrap();
        matched(&db, &air1, "air", "1998-01-16");
        matched(&db, &air2, "air", "1998");
        matched(&db, &moby, "moby", "1999");
        db.save_artist(&ArtistInfo { mbid: "air".into(), name: "AIR".into(), country: Some("FR".into()), genres: vec!["electronic".into()] })
            .unwrap();

        let stats = db.stats(None).unwrap();
        assert_eq!(stats.totals.likes, 4);
        assert_eq!(stats.totals.tracks_heard, 6);
        assert_eq!(stats.stations[0].name, "FIP");
        assert_eq!(stats.stations[0].likes, 3);
        let nova = &stats.stations[1];
        assert_eq!(nova.tracks_heard, 3);
        assert_eq!(nova.unrecognised_share, Some(1.0 / 3.0));
        assert_eq!(nova.likes_per_hour, None);

        assert_eq!(stats.identification.matched, 3);
        assert_eq!(stats.identification.pending, 1);
        assert_eq!(stats.artists[0].label, "AIR");
        assert_eq!(stats.artists[0].count, 2);
        assert_eq!(stats.albums[0].detail.as_deref(), Some("AIR"));
        assert_eq!(stats.decades, [Count { label: "1990s".into(), detail: None, count: 3 }]);
        assert_eq!(stats.countries[0].label, "FR");
        // Air's genres replace the recording tags; Moby has none, so its tags count.
        let genres: Vec<_> = stats.genres.iter().map(|g| (g.label.as_str(), g.count)).collect();
        assert_eq!(genres, [("electronic", 2), ("rock", 1)]);
        // Air was first liked on FIP, Moby too.
        assert_eq!(stats.discovery_stations[0].label, "FIP");
        assert_eq!(stats.discovery_stations[0].count, 2);
        assert_eq!(stats.new_artists_by_month[0].count, 2);
        // "Sexy Boy" was heard on both stations.
        assert_eq!(stats.shared_tracks[0].label, "Sexy Boy");
        assert_eq!(stats.shared_tracks[0].count, 2);

        assert_eq!(db.stats(Some(30)).unwrap().totals.likes, 4);
    }

    #[test]
    fn stats_of_an_empty_database() {
        let stats = Database::open_in_memory().unwrap().stats(Some(7)).unwrap();
        assert!(stats.stations.is_empty());
        assert_eq!(stats.totals.listening_hours, 0.0);
        assert_eq!(stats.identification.pending, 0);
    }
}
