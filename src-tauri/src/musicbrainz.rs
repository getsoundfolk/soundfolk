// SPDX-License-Identifier: MPL-2.0
//! Identification of liked tracks on MusicBrainz (https://musicbrainz.org), the open music
//! database: canonical artist, album (release group), release date and genres, for reliable stats.
//!
//! Opt-in (setting off by default): only the artist and title of liked tracks are sent. Good-neighbor
//! rules of the API: an identifying `User-Agent`, at most one request per second, and backing off on
//! errors. Offline-tolerant: a failed lookup is retried later, with a growing delay.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use stream_download::http::reqwest::{Client, StatusCode, Url};
use tauri::Manager;
use tokio::sync::Notify;

use crate::db::{Database, TrackToIdentify};
use crate::tracks;

/// Setting: identify liked tracks on MusicBrainz (off by default: titles leave the device).
pub const SETTING_IDENTIFY: &str = "identify_tracks";

const API: &str = "https://musicbrainz.org/ws/2/";
const USER_AGENT: &str = concat!("Soundfolk/", env!("CARGO_PKG_VERSION"), " ( https://github.com/raphaelh/soundfolk )");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The API allows one request per second on average.
const REQUEST_INTERVAL: Duration = Duration::from_millis(1100);
/// Below this search score (0–100), a result is not trusted.
const MIN_SCORE: i64 = 90;
/// Idle wait when there is nothing to do (a like or enabling the setting wakes the worker earlier).
const IDLE_WAIT: Duration = Duration::from_secs(600);
/// Wait after a network error before trying anything again.
const ERROR_WAIT: Duration = Duration::from_secs(60);

/// Wakes the worker (a track was liked, the setting was turned on).
pub struct IdentifyWaker(pub Arc<Notify>);

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingMatch {
    pub recording_mbid: String,
    pub title: String,
    pub artist_mbid: String,
    pub artist_name: String,
    pub album: Option<Album>,
    pub first_release_date: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub mbid: String,
    pub title: String,
    pub primary_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistInfo {
    pub mbid: String,
    pub name: String,
    /// ISO 3166-1 code (`GB`, `FR`…).
    pub country: Option<String>,
    pub genres: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("MusicBrainz unreachable: {0}")]
    Network(String),
    #[error("Unexpected MusicBrainz response: {0}")]
    Invalid(String),
}

pub struct MusicBrainz {
    client: Client,
}

impl MusicBrainz {
    pub fn new() -> Result<Self, LookupError> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| LookupError::Network(e.to_string()))?;
        Ok(Self { client })
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, LookupError> {
        let mut url = Url::parse(API).and_then(|u| u.join(path)).map_err(|e| LookupError::Invalid(e.to_string()))?;
        url.query_pairs_mut().extend_pairs(params).append_pair("fmt", "json");
        let response = self.client.get(url).send().await.map_err(|e| LookupError::Network(e.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Value::Null);
        }
        let response = response.error_for_status().map_err(|e| LookupError::Network(e.to_string()))?;
        let bytes = response.bytes().await.map_err(|e| LookupError::Network(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| LookupError::Invalid(e.to_string()))
    }

    /// Best recording for an artist and a title, if one is trustworthy.
    pub async fn find_recording(&self, artist: &str, title: &str) -> Result<Option<RecordingMatch>, LookupError> {
        let query = format!(
            "recording:{} AND artist:{}",
            lucene_phrase(&tracks::clean_for_search(title)),
            lucene_phrase(&tracks::clean_for_search(artist)),
        );
        let body = self.get("recording", &[("query", &query), ("limit", "5")]).await?;
        Ok(best_recording(&body))
    }

    pub async fn artist(&self, mbid: &str) -> Result<Option<ArtistInfo>, LookupError> {
        // The ID ends up in the URL path: only a UUID is accepted.
        if !is_mbid(mbid) {
            return Err(LookupError::Invalid(format!("invalid artist ID: {mbid}")));
        }
        let body = self.get(&format!("artist/{mbid}"), &[("inc", "genres")]).await?;
        Ok(parse_artist(&body))
    }
}

/// Runs for the whole life of the app: identifies liked tracks one by one while the setting is on.
pub fn spawn_worker(app: tauri::AppHandle, wake: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        let mb = match MusicBrainz::new() {
            Ok(mb) => mb,
            Err(e) => return tracing::warn!("track identification disabled: {e}"),
        };
        loop {
            let db = app.state::<Database>();
            let next = match db.get_bool(SETTING_IDENTIFY, false) {
                Ok(true) => db.next_track_to_identify(),
                Ok(false) => Ok(None),
                Err(e) => Err(e),
            };
            let wait = match next {
                Ok(Some(track)) => identify(&db, &mb, &track).await,
                Ok(None) => IDLE_WAIT,
                Err(e) => {
                    tracing::warn!("track identification: {e}");
                    ERROR_WAIT
                }
            };
            let _ = tokio::time::timeout(wait, wake.notified()).await;
        }
    });
}

/// Identifies one track; returns how long to wait before the next request.
async fn identify(db: &Database, mb: &MusicBrainz, track: &TrackToIdentify) -> Duration {
    let Some(artist) = track.artist.as_deref() else {
        // Without an artist, a title-only search is too unreliable.
        log_db(db.track_not_found(&track.track_key));
        return Duration::ZERO;
    };
    let found = match mb.find_recording(artist, &track.title).await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!("MusicBrainz lookup failed: {e}");
            log_db(db.postpone_track(&track.track_key));
            return ERROR_WAIT;
        }
    };
    let Some(found) = found else {
        log_db(db.track_not_found(&track.track_key));
        return REQUEST_INTERVAL;
    };
    log_db(db.save_track_match(&track.track_key, &found));

    // Country and genres come from the artist, looked up once.
    if db.artist_needs_lookup(&found.artist_mbid).unwrap_or(false) {
        tokio::time::sleep(REQUEST_INTERVAL).await;
        match mb.artist(&found.artist_mbid).await {
            Ok(Some(info)) => log_db(db.save_artist(&info)),
            Ok(None) => {}
            Err(e) => tracing::debug!("MusicBrainz artist lookup failed: {e}"),
        }
    }
    REQUEST_INTERVAL
}

fn log_db(result: rusqlite::Result<()>) {
    if let Err(e) = result {
        tracing::warn!("track identification not saved: {e}");
    }
}

/// Quoted Lucene phrase (the search syntax of the API).
fn lucene_phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn str_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The score is a number in the current API, a string in older responses.
fn score_of(value: &Value) -> i64 {
    match value.get("score") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn names(value: &Value, key: &str) -> Vec<String> {
    let mut items: Vec<(i64, String)> = value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t.get("count").and_then(Value::as_i64).unwrap_or(0), str_of(t, "name")?)))
        .collect();
    items.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    items.into_iter().map(|(_, n)| n.to_lowercase()).take(5).collect()
}

/// Picks the first recording above [`MIN_SCORE`] (results come best first).
fn best_recording(body: &Value) -> Option<RecordingMatch> {
    let recording = body
        .get("recordings")?
        .as_array()?
        .iter()
        .find(|r| score_of(r) >= MIN_SCORE)?;
    let credit = recording.get("artist-credit")?.as_array()?.first()?;
    let artist = credit.get("artist")?;
    Some(RecordingMatch {
        recording_mbid: str_of(recording, "id")?,
        title: str_of(recording, "title")?,
        artist_mbid: str_of(artist, "id")?,
        artist_name: str_of(artist, "name").or_else(|| str_of(credit, "name"))?,
        album: best_album(recording),
        first_release_date: str_of(recording, "first-release-date"),
        tags: names(recording, "tags"),
    })
}

/// The release group that best stands for "the album": a plain album first, then any album or EP
/// that is not a compilation or a live recording, then single, then whatever comes first.
fn best_album(recording: &Value) -> Option<Album> {
    let groups: Vec<&Value> = recording
        .get("releases")?
        .as_array()?
        .iter()
        .filter_map(|r| r.get("release-group"))
        .collect();
    let primary = |g: &Value| g.get("primary-type").and_then(Value::as_str).map(str::to_owned);
    let secondary = |g: &Value| -> Vec<String> {
        g.get("secondary-types")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect()
    };
    let rank = |g: &Value| -> u8 {
        let second = secondary(g);
        let bad = second.iter().any(|s| s == "Compilation" || s == "Live");
        match primary(g).as_deref() {
            Some("Album") if second.is_empty() => 0,
            Some("Album" | "EP") if !bad => 1,
            Some("Single") if !bad => 2,
            _ => 3,
        }
    };
    let group = groups.into_iter().min_by_key(|g| rank(g))?;
    Some(Album { mbid: str_of(group, "id")?, title: str_of(group, "title")?, primary_type: primary(group) })
}

fn parse_artist(body: &Value) -> Option<ArtistInfo> {
    Some(ArtistInfo {
        mbid: str_of(body, "id")?,
        name: str_of(body, "name")?,
        country: str_of(body, "country"),
        genres: names(body, "genres"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"{"recordings":[
      {"id":"aaaaaaaa-0000-0000-0000-000000000001","score":100,"title":"Overture: Food, Glorious Food",
       "first-release-date":"1963",
       "artist-credit":[{"name":"Original Broadway Cast","artist":{"id":"bbbbbbbb-0000-0000-0000-000000000001","name":"The Original Broadway Cast of Oliver!"}}],
       "releases":[
         {"id":"r1","release-group":{"id":"cccccccc-0000-0000-0000-000000000001","title":"Broadway Hits","primary-type":"Album","secondary-types":["Compilation"]}},
         {"id":"r2","release-group":{"id":"cccccccc-0000-0000-0000-000000000002","title":"Oliver! (Original Broadway Cast)","primary-type":"Album","secondary-types":["Soundtrack"]}}
       ],
       "tags":[{"count":1,"name":"Musical"},{"count":3,"name":"Show Tunes"}]},
      {"id":"aaaaaaaa-0000-0000-0000-000000000002","score":95,"title":"Other"}
    ]}"#;

    #[test]
    fn picks_the_best_recording_and_album() {
        let found = best_recording(&serde_json::from_str(SEARCH).unwrap()).unwrap();
        assert_eq!(found.title, "Overture: Food, Glorious Food");
        assert_eq!(found.artist_name, "The Original Broadway Cast of Oliver!");
        assert_eq!(found.first_release_date.as_deref(), Some("1963"));
        assert_eq!(found.tags, ["show tunes", "musical"]);
        let album = found.album.unwrap();
        // The cast album, not the compilation.
        assert_eq!(album.title, "Oliver! (Original Broadway Cast)");
    }

    #[test]
    fn low_scores_are_not_trusted() {
        let body = serde_json::json!({"recordings":[{"id":"x","score":"72","title":"t",
            "artist-credit":[{"name":"a","artist":{"id":"y","name":"a"}}]}]});
        assert_eq!(best_recording(&body), None);
        assert_eq!(best_recording(&serde_json::json!({"recordings":[]})), None);
        assert_eq!(best_recording(&Value::Null), None);
    }

    #[test]
    fn parses_an_artist() {
        let body = serde_json::json!({"id":"bbbbbbbb-0000-0000-0000-000000000001","name":"Air","country":"FR",
            "genres":[{"count":2,"name":"Electronic"},{"count":5,"name":"downtempo"}]});
        let artist = parse_artist(&body).unwrap();
        assert_eq!(artist.country.as_deref(), Some("FR"));
        assert_eq!(artist.genres, ["downtempo", "electronic"]);
    }

    #[test]
    fn queries_are_escaped() {
        assert_eq!(lucene_phrase(r#"say "hi" \o/"#), r#""say \"hi\" \\o/""#);
        assert!(!is_mbid("../../x"));
        assert!(is_mbid("bbbbbbbb-0000-0000-0000-000000000001"));
    }

    /// Against the real API: `cargo test -- --ignored live_musicbrainz`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_musicbrainz_finds_a_classic() {
        let mb = MusicBrainz::new().unwrap();
        let found = mb.find_recording("Ben E. King", "Stand by Me").await.unwrap().unwrap();
        assert_eq!(found.artist_name, "Ben E. King");
        tokio::time::sleep(REQUEST_INTERVAL).await;
        let artist = mb.artist(&found.artist_mbid).await.unwrap().unwrap();
        assert_eq!(artist.country.as_deref(), Some("US"));
    }
}
