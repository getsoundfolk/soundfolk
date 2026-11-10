// SPDX-License-Identifier: MPL-2.0
//! Identification of liked tracks on MusicBrainz (https://musicbrainz.org), the open music
//! database: canonical artist, album (release group), release date and genres, for reliable stats.
//!
//! Opt-in (setting off by default): only the artist and title of liked tracks are sent. Good-neighbor
//! rules of the API: an identifying `User-Agent`, at most one request per second, and backing off on
//! errors. Offline-tolerant: a failed lookup is retried later, with a growing delay.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode, Url};
use serde_json::Value;
use tauri::Manager;
use tokio::sync::Notify;

use crate::db::{settings, Database, TrackToIdentify};
use crate::state::with_db;
use crate::tracks;

const API: &str = "https://musicbrainz.org/ws/2/";
const COVER_ART_ARCHIVE: &str = "https://coverartarchive.org/";
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
    /// When the last request left: the identification worker and the cover lookups share the
    /// one-request-per-second budget.
    last_request: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl MusicBrainz {
    pub fn new() -> Result<Self, LookupError> {
        let client = crate::http::client().map_err(LookupError::Network)?.clone();
        Ok(Self {
            client,
            last_request: tokio::sync::Mutex::new(None),
        })
    }

    /// Waits until a request to MusicBrainz is allowed (at most one per [`REQUEST_INTERVAL`]).
    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep_until(at + REQUEST_INTERVAL).await;
        }
        *last = Some(tokio::time::Instant::now());
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, LookupError> {
        let mut url = Url::parse(API)
            .and_then(|u| u.join(path))
            .map_err(|e| LookupError::Invalid(e.to_string()))?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("fmt", "json");
        self.throttle().await;
        let response = self
            .client
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| LookupError::Network(e.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Value::Null);
        }
        let response = response
            .error_for_status()
            .map_err(|e| LookupError::Network(e.to_string()))?;
        let bytes = crate::http::body_at_most(response, MAX_ANSWER)
            .await
            .map_err(LookupError::Network)?;
        serde_json::from_slice(&bytes).map_err(|e| LookupError::Invalid(e.to_string()))
    }

    /// Best recording for an artist and a title, if one is trustworthy.
    async fn find_recording(
        &self,
        artist: &str,
        title: &str,
    ) -> Result<Option<RecordingMatch>, LookupError> {
        let query = format!(
            "recording:{} AND artist:{}",
            lucene_phrase(&tracks::clean_for_search(title)),
            lucene_phrase(&tracks::clean_for_search(artist)),
        );
        let body = self
            .get("recording", &[("query", &query), ("limit", "5")])
            .await?;
        Ok(best_recording(&body))
    }

    /// Release groups that may show a track's cover art, best first (see [`cover_candidates`]).
    pub async fn albums_of(&self, artist: &str, title: &str) -> Result<Vec<String>, LookupError> {
        let query = format!(
            "recording:{} AND artist:{}",
            lucene_phrase(&tracks::clean_for_search(title)),
            lucene_phrase(&tracks::clean_for_search(artist)),
        );
        let body = self
            .get("recording", &[("query", &query), ("limit", "25")])
            .await?;
        Ok(cover_candidates(&body))
    }

    pub async fn artist(&self, mbid: &str) -> Result<Option<ArtistInfo>, LookupError> {
        // The ID ends up in the URL path: only a UUID is accepted.
        if !is_mbid(mbid) {
            return Err(LookupError::Invalid(format!("invalid artist ID: {mbid}")));
        }
        let body = self
            .get(&format!("artist/{mbid}"), &[("inc", "genres")])
            .await?;
        Ok(parse_artist(&body))
    }

    /// Front cover of a release group on the Cover Art Archive (https://coverartarchive.org), as a
    /// 500 px thumbnail address, if it has one.
    pub async fn release_group_cover(&self, mbid: &str) -> Result<Option<String>, LookupError> {
        if !is_mbid(mbid) {
            return Err(LookupError::Invalid(format!(
                "invalid release group ID: {mbid}"
            )));
        }
        let url = format!("{COVER_ART_ARCHIVE}release-group/{mbid}");
        let response = self
            .client
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| LookupError::Network(e.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .map_err(|e| LookupError::Network(e.to_string()))?;
        let bytes = crate::http::body_at_most(response, MAX_ANSWER)
            .await
            .map_err(LookupError::Network)?;
        let body: Value =
            serde_json::from_slice(&bytes).map_err(|e| LookupError::Invalid(e.to_string()))?;
        Ok(front_cover(&body))
    }
}

/// Bigger than this, a MusicBrainz answer is not one (a search returns a few dozen KiB).
const MAX_ANSWER: usize = 2 * 1024 * 1024;

/// Runs for the whole life of the app: identifies liked tracks one by one while the setting is on.
pub fn spawn_worker(app: tauri::AppHandle, mb: Arc<MusicBrainz>, wake: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let db = app.state::<Database>();
            let next = with_db(&db, |db| {
                Ok(match db.flag(settings::IDENTIFY_TRACKS)? {
                    true => db.next_track_to_identify()?,
                    false => None,
                })
            })
            .await;
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
    let key = track.track_key.clone();
    let Some(artist) = track.artist.as_deref() else {
        // Without an artist, a title-only search is too unreliable.
        save(db, move |db| db.track_not_found(&key)).await;
        return Duration::ZERO;
    };
    let found = match mb.find_recording(artist, &track.title).await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!("MusicBrainz lookup failed: {e}");
            save(db, move |db| db.postpone_track(&key)).await;
            return ERROR_WAIT;
        }
    };
    let Some(found) = found else {
        save(db, move |db| db.track_not_found(&key)).await;
        return REQUEST_INTERVAL;
    };
    let matched = found.clone();
    save(db, move |db| db.save_track_match(&key, &matched)).await;

    // Country and genres come from the artist, looked up once.
    let mbid = found.artist_mbid.clone();
    let needs_lookup = with_db(db, move |db| Ok(db.artist_needs_lookup(&mbid)?)).await;
    if needs_lookup.unwrap_or(false) {
        tokio::time::sleep(REQUEST_INTERVAL).await;
        match mb.artist(&found.artist_mbid).await {
            Ok(Some(info)) => save(db, move |db| db.save_artist(&info)).await,
            Ok(None) => {}
            Err(e) => tracing::debug!("MusicBrainz artist lookup failed: {e}"),
        }
    }
    REQUEST_INTERVAL
}

/// Saves what was found, off the async runtime ([`with_db`]); a failure is logged.
async fn save(
    db: &Database,
    work: impl FnOnce(&Database) -> rusqlite::Result<()> + Send + 'static,
) {
    if let Err(e) = with_db(db, move |db| Ok(work(db)?)).await {
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
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
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
        .filter_map(|t| {
            Some((
                t.get("count").and_then(Value::as_i64).unwrap_or(0),
                str_of(t, "name")?,
            ))
        })
        .collect();
    items.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    items
        .into_iter()
        .map(|(_, n)| n.to_lowercase())
        .take(5)
        .collect()
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
    let primary = |g: &Value| {
        g.get("primary-type")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let group = groups.into_iter().min_by_key(|g| album_rank(g))?;
    Some(Album {
        mbid: str_of(group, "id")?,
        title: str_of(group, "title")?,
        primary_type: primary(group),
    })
}

/// How well a release group stands for "the album" (lower is better): a plain album, then an album
/// or EP that is not a compilation or a live recording, then a single, then anything else.
fn album_rank(group: &Value) -> u8 {
    let primary = group.get("primary-type").and_then(Value::as_str);
    let secondary: Vec<&str> = group
        .get("secondary-types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let bad = secondary
        .iter()
        .any(|s| *s == "Compilation" || *s == "Live");
    match primary {
        Some("Album") if secondary.is_empty() => 0,
        Some("Album" | "EP") if !bad => 1,
        Some("Single") if !bad => 2,
        _ => 3,
    }
}

/// Most release groups a cover lookup tries.
pub const MAX_COVER_CANDIDATES: usize = 3;

/// Release groups of every trustworthy recording, best first: by [`album_rank`], then by how many
/// releases of them carry the track (the well-known album has many editions), then as listed.
/// The best-scored recording alone is often an obscure compilation without cover art.
fn cover_candidates(body: &Value) -> Vec<String> {
    let mut groups: Vec<(String, u8, usize, usize)> = Vec::new();
    let recordings = body
        .get("recordings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for recording in recordings.filter(|r| score_of(r) >= MIN_SCORE) {
        let releases = recording
            .get("releases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for group in releases.filter_map(|r| r.get("release-group")) {
            let Some(id) = str_of(group, "id").filter(|id| is_mbid(id)) else {
                continue;
            };
            match groups.iter_mut().find(|g| g.0 == id) {
                Some(seen) => seen.2 += 1,
                None => {
                    let order = groups.len();
                    groups.push((id, album_rank(group), 1, order));
                }
            }
        }
    }
    groups.sort_by_key(|&(_, rank, count, order)| (rank, std::cmp::Reverse(count), order));
    groups
        .into_iter()
        .map(|g| g.0)
        .take(MAX_COVER_CANDIDATES)
        .collect()
}

/// The image marked as front in a Cover Art Archive listing: its 500 px thumbnail, or the largest
/// one given. Addresses are moved to https.
fn front_cover(body: &Value) -> Option<String> {
    let images = body.get("images")?.as_array()?;
    let front = images
        .iter()
        .find(|i| i.get("front").and_then(Value::as_bool) == Some(true))?;
    let thumbnails = front.get("thumbnails");
    let url = ["500", "large", "250", "small"]
        .iter()
        .find_map(|size| thumbnails.and_then(|t| str_of(t, size)))
        .or_else(|| str_of(front, "image"))?;
    let url = match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url,
    };
    url.starts_with("https://").then_some(url)
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
    fn cover_candidates_prefer_the_well_known_album() {
        let group = |id: u32, kind: &str, second: &[&str]| {
            serde_json::json!({"release-group":{"id":format!("cccccccc-0000-0000-0000-{id:012}"),"title":"t",
                "primary-type":kind,"secondary-types":second}})
        };
        let body = serde_json::json!({"recordings":[
            {"id":"a","score":100,"releases":[group(1, "Album", &["Compilation"]), group(2, "Album", &["Compilation"])]},
            {"id":"b","score":98,"releases":[group(3, "Single", &[]), group(4, "Album", &[]), group(5, "Album", &[])]},
            {"id":"c","score":95,"releases":[group(5, "Album", &[])]},
            {"id":"d","score":60,"releases":[group(6, "Album", &[])]}
        ]});
        let ids: Vec<String> = cover_candidates(&body)
            .into_iter()
            .map(|id| id[24..].to_string())
            .collect();
        // Two plain albums, the one with more releases first, then the single; never the low score
        assert_eq!(ids, ["000000000005", "000000000004", "000000000003"]);
        assert!(cover_candidates(&Value::Null).is_empty());
    }

    #[test]
    fn picks_the_front_cover() {
        let body = serde_json::json!({"images":[
            {"front":false,"image":"http://coverartarchive.org/release/r/back.jpg","thumbnails":{}},
            {"front":true,"image":"http://coverartarchive.org/release/r/1.jpg",
             "thumbnails":{"250":"http://coverartarchive.org/release/r/1-250.jpg","500":"http://coverartarchive.org/release/r/1-500.jpg"}}
        ]});
        assert_eq!(
            front_cover(&body).as_deref(),
            Some("https://coverartarchive.org/release/r/1-500.jpg")
        );
        let only_image = serde_json::json!({"images":[{"front":true,"image":"https://x/1.jpg"}]});
        assert_eq!(front_cover(&only_image).as_deref(), Some("https://x/1.jpg"));
        assert_eq!(
            front_cover(&serde_json::json!({"images":[{"front":false,"image":"https://x/1.jpg"}]})),
            None
        );
        assert_eq!(
            front_cover(&serde_json::json!({"images":[{"front":true,"image":"ftp://x/1.jpg"}]})),
            None
        );
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
        let found = mb
            .find_recording("Ben E. King", "Stand by Me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.artist_name, "Ben E. King");
        tokio::time::sleep(REQUEST_INTERVAL).await;
        let artist = mb.artist(&found.artist_mbid).await.unwrap().unwrap();
        assert_eq!(artist.country.as_deref(), Some("US"));
    }
}
