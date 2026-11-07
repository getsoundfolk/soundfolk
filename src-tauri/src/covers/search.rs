// SPDX-License-Identifier: MPL-2.0
//! What the music catalogs searched for cover art (Deezer, iTunes) share: their HTTP client, and
//! the choice of the right result.
//!
//! Only a result for the same track (same comparison key as the history: artist and title, noise
//! ignored) is trusted. Among them, the studio album comes before live recordings, remixes and
//! compilations; otherwise the catalog's order (relevance, popularity) is kept.

use std::time::Duration;

use serde::de::DeserializeOwned;
use stream_download::http::reqwest::{Client, Url};

use crate::tracks::{self, ParsedTitle};

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

/// The HTTP client of a catalog (`name` is for the log).
pub fn client(name: &str) -> Option<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .inspect_err(|e| tracing::warn!("no HTTP client for {name}: {e}"))
        .ok()
}

/// A catalog's JSON answer.
pub async fn get_json<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T, String> {
    let bytes = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// A track found in a catalog, with its album's cover.
pub struct Candidate<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
    /// The catalog says the album is a compilation ("Various Artists" and the like).
    pub compilation: bool,
    pub cover: Option<String>,
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
fn rank(candidate: &Candidate) -> u8 {
    let other_version =
        mentions(candidate.title, &OTHER_VERSIONS) || mentions(candidate.album, &OTHER_VERSIONS);
    let compilation = candidate.compilation || mentions(candidate.album, &COMPILATIONS);
    u8::from(other_version) + u8::from(compilation)
}

/// The cover of the best result for the same track (an https address).
pub fn best_cover<'a>(
    parsed: &ParsedTitle,
    candidates: impl IntoIterator<Item = Candidate<'a>>,
) -> Option<String> {
    let wanted = tracks::track_key(parsed);
    candidates
        .into_iter()
        .filter(|c| {
            let found = ParsedTitle {
                artist: Some(c.artist.to_owned()),
                title: c.title.to_owned(),
            };
            tracks::track_key(&found) == wanted
        })
        .filter_map(|c| {
            let cover = c.cover.as_deref()?.trim();
            cover
                .starts_with("https://")
                .then(|| (rank(&c), cover.to_owned()))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, cover)| cover)
}

#[cfg(test)]
pub(super) fn wanted(artist: &str, title: &str) -> ParsedTitle {
    ParsedTitle {
        artist: Some(artist.into()),
        title: title.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(artist: &'a str, title: &'a str, album: &'a str, cover: &str) -> Candidate<'a> {
        Candidate {
            artist,
            title,
            album,
            compilation: false,
            cover: Some(cover.into()),
        }
    }

    fn results() -> Vec<Candidate<'static>> {
        vec![
            found(
                "Jack Johnson",
                "Wasting Time (Live In George, Washington/2008)",
                "En Concert",
                "https://cdn/live.jpg",
            ),
            found(
                "Music Box Mania",
                "Wasting Time",
                "Music Box Versions of Jack Johnson",
                "https://cdn/box.jpg",
            ),
            found(
                "Jack Johnson",
                "Wasting Time",
                "On And On",
                "https://cdn/studio.jpg",
            ),
        ]
    }

    #[test]
    fn the_studio_album_of_the_same_track_wins() {
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting time"), results()).as_deref(),
            Some("https://cdn/studio.jpg")
        );
        // Another artist's cover version is never taken
        assert_eq!(
            best_cover(&wanted("Ben Harper", "Wasting time"), results()),
            None
        );
    }

    #[test]
    fn a_live_cover_is_better_than_none() {
        let live_only = results().into_iter().take(1);
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting Time"), live_only).as_deref(),
            Some("https://cdn/live.jpg")
        );
    }

    #[test]
    fn compilations_come_after_the_album() {
        let mut various = found("Air", "Sexy Boy", "Now 40", "https://cdn/now.jpg");
        various.compilation = true;
        let album = found("Air", "Sexy Boy", "Moon Safari", "https://cdn/moon.jpg");
        assert_eq!(
            best_cover(&wanted("Air", "Sexy Boy"), [various, album]).as_deref(),
            Some("https://cdn/moon.jpg")
        );
    }

    #[test]
    fn only_https_covers_are_kept() {
        let http = found("Air", "Sexy Boy", "Moon Safari", "http://cdn/x.jpg");
        assert_eq!(best_cover(&wanted("Air", "Sexy Boy"), [http]), None);
    }

    #[test]
    fn version_words_are_whole_words() {
        assert!(mentions("Sexy Boy (Live BBC 1998)", &OTHER_VERSIONS));
        assert!(mentions("90s Classics 90s Hits", &COMPILATIONS));
        assert!(!mentions("Alive", &OTHER_VERSIONS));
        assert!(!mentions("Credit", &OTHER_VERSIONS));
    }
}
