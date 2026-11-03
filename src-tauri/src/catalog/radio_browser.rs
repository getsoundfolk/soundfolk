// SPDX-License-Identifier: MPL-2.0
//! Client for the public Radio-Browser API (https://api.radio-browser.info).
//!
//! Good-neighbor rules applied: explicit `User-Agent`, bounded timeouts, a single mirror
//! queried at a time, and all search calls paginated with an explicit limit.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use stream_download::http::reqwest::{Client, Url};
use tokio::sync::OnceCell;

use super::{
    rank, CatalogError, CatalogFuture, CatalogSource, Health, SearchPage, SearchQuery, Station,
};

/// Only guaranteed mirror; the full list is discovered via `/json/servers`.
const FALLBACK_SERVER: &str = "https://de1.api.radio-browser.info";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ATTEMPTS: usize = 3;
/// Catalog pages fetched at most per search: unsupported (HLS) stations are dropped, so a page can
/// come back short; we fetch a few more to fill it without hammering the API.
const MAX_PAGES_PER_SEARCH: usize = 3;

pub struct RadioBrowser {
    client: Client,
    servers: OnceCell<Vec<Url>>,
    /// Last mirror that responded, tried first.
    preferred: Mutex<Option<Url>>,
}

impl RadioBrowser {
    pub fn new() -> Result<Self, CatalogError> {
        let client = Client::builder()
            .user_agent(concat!("Soundfolk/", env!("CARGO_PKG_VERSION")))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        Ok(Self { client, servers: OnceCell::new(), preferred: Mutex::new(None) })
    }

    /// List of mirrors, discovered only once (falls back to the default mirror).
    async fn servers(&self) -> &[Url] {
        self.servers
            .get_or_init(|| async {
                let fallback = vec![Url::parse(FALLBACK_SERVER).expect("valid constant URL")];
                let Ok(url) = Url::parse(&format!("{FALLBACK_SERVER}/json/servers")) else {
                    return fallback;
                };
                let found: Result<Vec<ServerEntry>, _> = async {
                    let bytes = self.client.get(url).send().await?.error_for_status()?.bytes().await?;
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string().into())
                }
                .await
                .map_err(|e: Box<dyn std::error::Error + Send + Sync>| e);

                let mut hosts: Vec<Url> = found
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| Url::parse(&format!("https://{}", s.name)).ok())
                    .collect();
                hosts.dedup();
                if hosts.is_empty() {
                    return fallback;
                }
                // Spreads load across mirrors without a random-number dependency.
                let shift = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos() as usize)
                    % hosts.len();
                hosts.rotate_left(shift);
                hosts
            })
            .await
    }

    /// GET on an API path, trying the mirrors one after the other.
    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Vec<u8>, CatalogError> {
        let mut candidates: Vec<Url> = self.servers().await.to_vec();
        if let Some(first) = self.preferred.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            candidates.retain(|u| *u != first);
            candidates.insert(0, first);
        }

        let mut last_error = CatalogError::Network("no server available".into());
        for base in candidates.into_iter().take(MAX_ATTEMPTS) {
            let mut url = base.join(path).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));

            match self.client.get(url).send().await.and_then(|r| r.error_for_status()) {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => {
                        *self.preferred.lock().unwrap_or_else(|p| p.into_inner()) = Some(base);
                        return Ok(bytes.to_vec());
                    }
                    Err(e) => last_error = CatalogError::Network(e.to_string()),
                },
                Err(e) => last_error = CatalogError::Network(e.to_string()),
            }
            tracing::warn!(server = %base, "Radio-Browser mirror failed: {last_error}");
        }
        Err(last_error)
    }
}

impl CatalogSource for RadioBrowser {
    fn search<'a>(&'a self, query: &'a SearchQuery) -> CatalogFuture<'a, SearchPage> {
        Box::pin(async move {
            let q = query.sanitized()?;
            let limit = q.limit.unwrap_or(super::DEFAULT_LIMIT);
            let mut offset = q.offset.unwrap_or(0);
            let mut stations = Vec::new();
            let mut exhausted = false;

            for _ in 0..MAX_PAGES_PER_SEARCH {
                let params = search_params(&q, limit, offset);
                let body = self.get("json/stations/search", &params).await?;
                let (page, raw_len) = parse_stations(&body)?;
                offset += raw_len as u32;
                stations.extend(page.into_iter().filter(Station::is_supported));

                if raw_len < limit as usize {
                    exhausted = true;
                    break;
                }
                if stations.len() >= limit as usize {
                    break;
                }
            }

            rank(&mut stations);
            Ok(SearchPage { stations, next_offset: (!exhausted).then_some(offset) })
        })
    }

    fn count_click<'a>(&'a self, station_id: &'a str) -> CatalogFuture<'a, ()> {
        Box::pin(async move {
            // The ID ends up in the URL path: only a UUID is accepted.
            if !is_uuid(station_id) {
                return Err(CatalogError::BadRequest(format!("Invalid ID: {station_id}")));
            }
            self.get(&format!("json/url/{station_id}"), &[]).await.map(|_| ())
        })
    }
}

#[derive(Deserialize)]
struct ServerEntry {
    name: String,
}

/// Station as returned by the API (fields often `null` or missing).
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStation {
    stationuuid: String,
    name: Option<String>,
    url: Option<String>,
    url_resolved: Option<String>,
    homepage: Option<String>,
    favicon: Option<String>,
    tags: Option<String>,
    country: Option<String>,
    countrycode: Option<String>,
    language: Option<String>,
    codec: Option<String>,
    bitrate: Option<u32>,
    votes: Option<i64>,
    hls: Option<u8>,
    lastcheckok: Option<u8>,
    lastcheckoktime_iso8601: Option<String>,
}

/// Query parameters of one catalog request.
fn search_params(q: &SearchQuery, limit: u32, offset: u32) -> Vec<(&'static str, String)> {
    let mut params = vec![
        // Descending popularity: dead stations (0 clicks) naturally end up at the bottom.
        ("order", "clickcount".to_string()),
        ("reverse", "true".to_string()),
        // We want to show suspect stations (see `Health::Unreachable`), not hide them.
        ("hidebroken", "false".to_string()),
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
    ];
    if let Some(text) = &q.text {
        params.push(("name", text.clone()));
    }
    if let Some(code) = &q.country_code {
        params.push(("countrycode", code.clone()));
    }
    if let Some(tag) = &q.tag {
        params.push(("tag", tag.clone()));
    }
    params
}

/// Converts a catalog response. Also returns how many entries the catalog sent, including the
/// unusable ones that were dropped: pagination must advance by that number, not by the usable count.
fn parse_stations(body: &[u8]) -> Result<(Vec<Station>, usize), CatalogError> {
    let raw: Vec<RawStation> =
        serde_json::from_slice(body).map_err(|e| CatalogError::Invalid(e.to_string()))?;
    let raw_len = raw.len();
    Ok((raw.into_iter().filter_map(convert).collect(), raw_len))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// `None` if the station is unusable (no name, http(s) URL or ID).
fn convert(raw: RawStation) -> Option<Station> {
    let name = non_empty(raw.name)?;
    let url = non_empty(raw.url_resolved).or_else(|| non_empty(raw.url))?;
    let parsed = Url::parse(&url).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
    if !is_uuid(&raw.stationuuid) {
        return None;
    }

    let health = if raw.hls == Some(1) || parsed.path().to_ascii_lowercase().ends_with(".m3u8") {
        Health::Unsupported { reason: "hls".into() }
    } else if raw.lastcheckok == Some(0) {
        Health::Unreachable { last_ok: non_empty(raw.lastcheckoktime_iso8601) }
    } else {
        Health::Ok
    };

    Some(Station {
        id: raw.stationuuid,
        name,
        url,
        homepage: non_empty(raw.homepage),
        favicon: non_empty(raw.favicon),
        tags: raw
            .tags
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .take(8)
            .map(str::to_owned)
            .collect(),
        country: non_empty(raw.country),
        country_code: non_empty(raw.countrycode),
        language: non_empty(raw.language),
        codec: non_empty(raw.codec),
        bitrate: raw.bitrate.unwrap_or(0),
        votes: raw.votes.unwrap_or(0).max(0) as u32,
        health,
    })
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {"stationuuid":"11111111-1111-1111-1111-111111111111","name":"\tDead Radio","url":"http://a.example/x","url_resolved":"http://a.example/x","tags":"jazz, ,soul","countrycode":"FR","bitrate":128,"votes":3,"hls":0,"lastcheckok":0,"lastcheckoktime_iso8601":"2026-10-01T10:00:00Z"},
      {"stationuuid":"22222222-2222-2222-2222-222222222222","name":"HLS Channel","url":"https://b.example/index.m3u8","url_resolved":"https://b.example/index.m3u8","hls":1,"lastcheckok":1},
      {"stationuuid":"33333333-3333-3333-3333-333333333333","name":"RDWA","url":"https://c.example/x","url_resolved":"","homepage":"","favicon":null,"codec":"MP3","bitrate":192,"votes":11,"hls":0,"lastcheckok":1},
      {"stationuuid":"44444444-4444-4444-4444-444444444444","name":"","url":"https://d.example/x"},
      {"stationuuid":"not-a-uuid","name":"Bad id","url":"https://e.example/x"},
      {"stationuuid":"55555555-5555-5555-5555-555555555555","name":"FTP","url":"ftp://f.example/x"}
    ]"#;

    #[test]
    fn converts_and_drops_unusable_entries() {
        let (stations, raw_len) = parse_stations(FIXTURE.as_bytes()).unwrap();
        // Six entries sent, three usable: pagination must advance by six.
        assert_eq!(raw_len, 6);
        let names: Vec<_> = stations.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Dead Radio", "HLS Channel", "RDWA"]);

        let dead = &stations[0];
        assert_eq!(dead.tags, ["jazz", "soul"]);
        assert_eq!(dead.health, Health::Unreachable { last_ok: Some("2026-10-01T10:00:00Z".into()) });

        assert_eq!(stations[1].health, Health::Unsupported { reason: "hls".into() });

        // Empty `url_resolved`: fall back to `url`; empty fields = None.
        assert_eq!(stations[2].url, "https://c.example/x");
        assert_eq!(stations[2].homepage, None);
        assert_eq!(stations[2].health, Health::Ok);
    }

    #[test]
    fn ranking_puts_healthy_first_and_keeps_popularity_order() {
        let (mut stations, _) = parse_stations(FIXTURE.as_bytes()).unwrap();
        rank(&mut stations);
        let names: Vec<_> = stations.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["RDWA", "Dead Radio", "HLS Channel"]);
    }

    #[test]
    fn query_is_sanitized() {
        let q = SearchQuery {
            text: Some("  jazz  ".into()),
            country_code: Some("fr".into()),
            tag: Some(" Soul ".into()),
            limit: Some(10_000),
            offset: None,
        }
        .sanitized()
        .unwrap();
        assert_eq!(q.text.as_deref(), Some("jazz"));
        assert_eq!(q.country_code.as_deref(), Some("FR"));
        assert_eq!(q.tag.as_deref(), Some("soul"));
        assert_eq!(q.limit, Some(super::super::MAX_LIMIT));

        let empty = SearchQuery { text: Some("   ".into()), ..Default::default() }.sanitized().unwrap();
        assert_eq!(empty.text, None);
        assert!(SearchQuery { country_code: Some("FRA".into()), ..Default::default() }
            .sanitized()
            .is_err());
    }

    #[test]
    fn only_uuids_reach_the_click_url() {
        assert!(is_uuid("ab30ca84-c228-4f57-8cac-816c69ef651d"));
        assert!(!is_uuid("../../etc/passwd"));
        assert!(!is_uuid("ab30ca84-c228-4f57-8cac-816c69ef651"));
    }

    /// Against the real API: `cargo test -- --ignored live_search`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_search_finds_rdwa() {
        let rb = RadioBrowser::new().unwrap();
        let query = SearchQuery { text: Some("rdwa".into()), ..Default::default() };
        let page = rb.search(&query).await.unwrap();
        assert!(page.stations.iter().any(|s| s.id == "ab30ca84-c228-4f57-8cac-816c69ef651d"));
    }

    #[test]
    fn unsupported_stations_are_filtered_out() {
        let (stations, _) = parse_stations(FIXTURE.as_bytes()).unwrap();
        let supported: Vec<_> =
            stations.iter().filter(|s| s.is_supported()).map(|s| s.name.as_str()).collect();
        assert_eq!(supported, ["Dead Radio", "RDWA"]); // unreachable stays, HLS goes
    }

    #[test]
    fn search_params_carry_the_filters_and_the_offset() {
        let q = SearchQuery {
            text: Some("jazz".into()),
            country_code: Some("FR".into()),
            tag: Some("soul".into()),
            ..Default::default()
        };
        let params = search_params(&q, 30, 60);
        let get = |key: &str| params.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("limit"), Some("30"));
        assert_eq!(get("offset"), Some("60"));
        assert_eq!(get("name"), Some("jazz"));
        assert_eq!(get("countrycode"), Some("FR"));
        assert_eq!(get("tag"), Some("soul"));
        assert_eq!(get("hidebroken"), Some("false"));
    }

    /// Against the real API: HLS stations never come back, and paging moves forward.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_search_hides_hls_and_paginates() {
        let rb = RadioBrowser::new().unwrap();
        // Korean broadcasters are mostly HLS: a good stress test for short pages.
        let query = SearchQuery { text: Some("kbs".into()), limit: Some(10), ..Default::default() };
        let first = rb.search(&query).await.unwrap();
        assert!(first.stations.iter().all(|s| s.is_supported()));
        if let Some(next) = first.next_offset {
            let second = rb
                .search(&SearchQuery { offset: Some(next), ..query.clone() })
                .await
                .unwrap();
            assert!(second.stations.iter().all(|s| s.is_supported()));
            let ids: std::collections::HashSet<_> = first.stations.iter().map(|s| &s.id).collect();
            assert!(second.stations.iter().all(|s| !ids.contains(&s.id)), "pages overlap");
        }
    }
}
