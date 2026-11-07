// SPDX-License-Identifier: MPL-2.0
//! Cover art from Deezer's public search API (https://developers.deezer.com/api/search): one
//! request gives the track's album and its cover, served by Deezer's image CDN. No account or key.
//!
//! Only a result for the same track (same comparison key as the history: artist and title, noise
//! ignored) is trusted. Among them, the studio album comes before live recordings, remixes and
//! compilations; otherwise Deezer's order (popularity) is kept.

use std::time::Duration;

use serde::Deserialize;
use stream_download::http::reqwest::{Client, Url};

use crate::tracks::{self, ParsedTitle};

const SEARCH: &str = "https://api.deezer.com/search";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!(
    "Soundfolk/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/getsoundfolk/soundfolk )"
);
/// Words that mark a version other than the studio one, in a track or album title.
const OTHER_VERSIONS: [&str; 9] = [
    "live",
    "concert",
    "remix",
    "dub",
    "version",
    "edit",
    "acoustic",
    "karaoke",
    "instrumental",
];
/// Words that mark a compilation in an album title.
const COMPILATIONS: [&str; 6] = [
    "best of",
    "greatest",
    "hits",
    "classics",
    "anthology",
    "essential",
];

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    data: Vec<Track>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Track {
    title: String,
    #[serde(default)]
    title_short: Option<String>,
    artist: Artist,
    album: Album,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Album {
    #[serde(default)]
    title: String,
    cover_big: Option<String>,
    cover_medium: Option<String>,
}

pub struct Deezer {
    client: Client,
}

impl Deezer {
    pub fn new() -> Option<Self> {
        Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .inspect_err(|e| tracing::warn!("no HTTP client for Deezer: {e}"))
            .ok()
            .map(|client| Self { client })
    }

    /// The cover of the track's album (500 px), if Deezer has the track.
    pub async fn find_cover(&self, parsed: &ParsedTitle) -> Result<Option<String>, String> {
        let mut url = Url::parse(SEARCH).map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("q", &tracks::search_query(parsed))
            .append_pair("limit", "25");
        let bytes = self
            .client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        let response: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        // Quota exceeded and the like: tried again later, not remembered as "no cover".
        if let Some(error) = response.error {
            return Err(format!("Deezer error: {error}"));
        }
        Ok(best_cover(parsed, &response.data))
    }
}

fn mentions(text: &str, words: &[&str]) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|w| {
        text.match_indices(w).any(|(i, _)| {
            let before = text[..i].chars().next_back();
            let after = text[i + w.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    })
}

/// How far a result is from the studio album (lower is better).
fn rank(track: &Track) -> u8 {
    let other_version =
        mentions(&track.title, &OTHER_VERSIONS) || mentions(&track.album.title, &OTHER_VERSIONS);
    let compilation = mentions(&track.album.title, &COMPILATIONS);
    u8::from(other_version) + u8::from(compilation)
}

/// The cover of the best result for the same track.
fn best_cover(parsed: &ParsedTitle, results: &[Track]) -> Option<String> {
    let wanted = tracks::track_key(parsed);
    results
        .iter()
        .filter(|t| {
            let title = t.title_short.as_deref().unwrap_or(&t.title);
            let found = ParsedTitle {
                artist: Some(t.artist.name.clone()),
                title: title.to_owned(),
            };
            tracks::track_key(&found) == wanted
        })
        .filter_map(|t| {
            let cover = t
                .album
                .cover_big
                .as_deref()
                .or(t.album.cover_medium.as_deref())?;
            let cover = cover.trim();
            cover
                .starts_with("https://")
                .then(|| (rank(t), cover.to_owned()))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, cover)| cover)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(artist: &str, title: &str) -> ParsedTitle {
        ParsedTitle {
            artist: Some(artist.into()),
            title: title.into(),
        }
    }

    const RESULTS: &str = r#"{"data":[
      {"title":"Wasting Time (Live In George, Washington/2008)","title_short":"Wasting Time",
       "artist":{"name":"Jack Johnson"},"album":{"title":"En Concert","cover_big":"https://cdn/live.jpg"}},
      {"title":"Wasting Time","title_short":"Wasting Time",
       "artist":{"name":"Music Box Mania"},"album":{"title":"Music Box Versions of Jack Johnson","cover_big":"https://cdn/box.jpg"}},
      {"title":"Wasting Time","title_short":"Wasting Time",
       "artist":{"name":"Jack Johnson"},"album":{"title":"On And On","cover_big":"https://cdn/studio.jpg"}}
    ],"total":3}"#;

    #[test]
    fn the_studio_album_of_the_same_track_wins() {
        let response: Response = serde_json::from_str(RESULTS).unwrap();
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting time"), &response.data).as_deref(),
            Some("https://cdn/studio.jpg")
        );
        // Another artist's cover version is never taken
        assert_eq!(
            best_cover(&wanted("Ben Harper", "Wasting time"), &response.data),
            None
        );
    }

    #[test]
    fn a_live_cover_is_better_than_none() {
        let response: Response = serde_json::from_str(RESULTS).unwrap();
        let live_only: Vec<Track> = response.data.into_iter().take(1).collect();
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting Time"), &live_only).as_deref(),
            Some("https://cdn/live.jpg")
        );
    }

    #[test]
    fn errors_and_odd_covers_are_not_kept() {
        let quota: Response = serde_json::from_str(
            r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#,
        )
        .unwrap();
        assert!(quota.error.is_some() && quota.data.is_empty());
        let http: Response = serde_json::from_str(
            r#"{"data":[{"title":"Sexy Boy","artist":{"name":"Air"},"album":{"title":"Moon Safari","cover_big":"http://cdn/x.jpg"}}]}"#,
        )
        .unwrap();
        assert_eq!(best_cover(&wanted("Air", "Sexy Boy"), &http.data), None);
    }

    #[test]
    fn version_words_are_whole_words() {
        assert!(mentions("Sexy Boy (Live BBC 1998)", &OTHER_VERSIONS));
        assert!(mentions("90s Classics 90s Hits", &COMPILATIONS));
        assert!(!mentions("Alive", &OTHER_VERSIONS));
        assert!(!mentions("Credit", &OTHER_VERSIONS));
    }

    /// Against the real API: `cargo test -- --ignored live_deezer`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_deezer_finds_the_album() {
        let deezer = Deezer::new().unwrap();
        let url = deezer
            .find_cover(&wanted("Air", "Sexy Boy"))
            .await
            .unwrap()
            .unwrap();
        assert!(
            url.starts_with("https://") && url.contains("dzcdn"),
            "{url}"
        );
    }
}
