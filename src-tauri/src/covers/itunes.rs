// SPDX-License-Identifier: MPL-2.0
//! Cover art from Apple's iTunes Search API
//! (https://performance-partners.apple.com/search-api): one request gives the track's album and
//! its artwork, served by Apple's image CDN. No account or key; about 20 requests a minute.
//! The result is chosen as for every catalog (see [`super::search`]).

use serde::Deserialize;
use stream_download::http::reqwest::{Client, Url};

use super::search::{self, Candidate};
use crate::tracks::{self, ParsedTitle};

const SEARCH: &str = "https://itunes.apple.com/search";
/// The artwork address names its size: the 100 px given is asked at this size instead.
const ARTWORK_SIZE: &str = "500x500bb";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    #[serde(default)]
    results: Vec<Track>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Track {
    artist_name: String,
    track_name: String,
    #[serde(default)]
    collection_name: String,
    /// Set when the album's artist is not the track's: "Various Artists" on a compilation.
    collection_artist_name: Option<String>,
    artwork_url100: Option<String>,
}

pub struct Itunes {
    client: Client,
}

impl Itunes {
    pub fn new() -> Option<Self> {
        search::client("iTunes").map(|client| Self { client })
    }

    /// The artwork of the track's album (500 px), if the iTunes Store has the track.
    pub async fn find_cover(&self, parsed: &ParsedTitle) -> Result<Option<String>, String> {
        let mut url = Url::parse(SEARCH).map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("term", &tracks::search_query(parsed))
            .append_pair("media", "music")
            .append_pair("entity", "song")
            .append_pair("limit", "25");
        // Over the rate limit, the API answers 403: an error, tried again later.
        let response: Response = search::get_json(&self.client, url).await?;
        Ok(best_cover(parsed, &response.results))
    }
}

/// The artwork address at [`ARTWORK_SIZE`] (its last path segment is the size: `100x100bb.jpg`).
fn artwork(url: &str) -> Option<String> {
    let (base, file) = url.trim().rsplit_once('/')?;
    let (size, ext) = file.split_once('.')?;
    size.starts_with("100x100")
        .then(|| format!("{base}/{ARTWORK_SIZE}.{ext}"))
}

fn best_cover(parsed: &ParsedTitle, results: &[Track]) -> Option<String> {
    let candidates = results.iter().map(|t| Candidate {
        artist: &t.artist_name,
        title: &t.track_name,
        album: &t.collection_name,
        compilation: t
            .collection_artist_name
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("various artists")),
        cover: t.artwork_url100.as_deref().and_then(artwork),
    });
    search::best_cover(parsed, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covers::search::wanted;

    #[test]
    fn artwork_is_asked_at_500_px() {
        assert_eq!(
            artwork("https://is1-ssl.mzstatic.com/image/thumb/Music124/v4/7b/ef/x.rgb.jpg/100x100bb.jpg")
                .as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music124/v4/7b/ef/x.rgb.jpg/500x500bb.jpg")
        );
        assert_eq!(artwork("https://x/cover.jpg"), None);
    }

    #[test]
    fn results_are_read() {
        let response: Response = serde_json::from_str(
            r#"{"resultCount":3,"results":[
              {"artistName":"Jack Johnson","trackName":"Wasting Time (Live in George, Washington/2008)",
               "collectionName":"En Concert (Live)","artworkUrl100":"https://cdn/live/100x100bb.jpg"},
              {"artistName":"Jack Johnson","trackName":"Wasting Time","collectionName":"Surf Hits",
               "collectionArtistName":"Various Artists","artworkUrl100":"https://cdn/various/100x100bb.jpg"},
              {"artistName":"Jack Johnson","trackName":"Wasting Time","collectionName":"On and On",
               "artworkUrl100":"https://cdn/studio/100x100bb.jpg"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            best_cover(&wanted("Jack Johnson", "Wasting time"), &response.results).as_deref(),
            Some("https://cdn/studio/500x500bb.jpg")
        );
    }

    /// Against the real API: `cargo test -- --ignored live_itunes`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_itunes_finds_the_album() {
        let itunes = Itunes::new().unwrap();
        let url = itunes
            .find_cover(&wanted("Air", "Sexy Boy"))
            .await
            .unwrap()
            .unwrap();
        assert!(
            url.starts_with("https://")
                && url.contains("mzstatic")
                && url.ends_with("500x500bb.jpg"),
            "{url}"
        );
    }
}
