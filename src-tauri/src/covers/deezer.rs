// SPDX-License-Identifier: MPL-2.0
//! Cover art from Deezer's public search API (https://developers.deezer.com/api/search): one
//! request gives the track's album and its cover, served by Deezer's image CDN. No account or key.
//! The result is chosen as for every catalog (see [`super::search`]).

use serde::Deserialize;
use stream_download::http::reqwest::{Client, Url};

use super::search::{self, Candidate};
use crate::tracks::{self, ParsedTitle};

const SEARCH: &str = "https://api.deezer.com/search";

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    data: Vec<Track>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Track {
    title: String,
    /// The title without its version ("Live in…", "Remastered").
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
        search::client("Deezer").map(|client| Self { client })
    }

    /// The cover of the track's album (500 px), if Deezer has the track.
    pub async fn find_cover(&self, parsed: &ParsedTitle) -> Result<Option<String>, String> {
        let mut url = Url::parse(SEARCH).map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("q", &tracks::search_query(parsed))
            .append_pair("limit", "25");
        let response: Response = search::get_json(&self.client, url).await?;
        // Quota exceeded and the like: tried again later, not remembered as "no cover".
        if let Some(error) = response.error {
            return Err(format!("Deezer error: {error}"));
        }
        Ok(best_cover(parsed, &response.data))
    }
}

fn best_cover(parsed: &ParsedTitle, results: &[Track]) -> Option<String> {
    let candidates = results.iter().map(|t| Candidate {
        artist: &t.artist.name,
        title: t.title_short.as_deref().unwrap_or(&t.title),
        album: &t.album.title,
        compilation: false,
        cover: t.album.cover_big.clone().or(t.album.cover_medium.clone()),
    });
    search::best_cover(parsed, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covers::search::wanted;

    #[test]
    fn results_are_read() {
        let response: Response = serde_json::from_str(
            r#"{"data":[
              {"title":"Wasting Time (Live In George, Washington/2008)","title_short":"Wasting Time",
               "artist":{"name":"Jack Johnson"},"album":{"title":"En Concert","cover_big":"https://cdn/live.jpg"}},
              {"title":"Wasting Time","title_short":"Wasting Time",
               "artist":{"name":"Jack Johnson"},"album":{"title":"On And On","cover_medium":"https://cdn/studio.jpg"}}
            ],"total":2}"#,
        )
        .unwrap();
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting time"), &response.data).as_deref(),
            Some("https://cdn/studio.jpg")
        );
    }

    #[test]
    fn errors_are_seen() {
        let quota: Response = serde_json::from_str(
            r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#,
        )
        .unwrap();
        assert!(quota.error.is_some() && quota.data.is_empty());
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
