// SPDX-License-Identifier: MPL-2.0
//! Client for the public Radio-Browser API (https://api.radio-browser.info).
//!
//! Good-neighbor rules applied: explicit `User-Agent`, bounded timeouts, a single mirror
//! queried at a time, and all search calls paginated with an explicit limit.
//!
//! Mirrors are found as the API documentation asks: a DNS lookup of `all.api.radio-browser.info`,
//! whose addresses are the mirrors (no mirror is asked for the list, none is named in the code).
//! The certificate covers that name on every mirror, so each one is reached under it, pinned to
//! its address. The list is shuffled, the mirror that answered is kept, and the next one is tried
//! when a request fails. It is looked up again when every mirror failed, when DNS gave nothing
//! (a known mirror stands in meanwhile), and once an hour, as mirrors come and go.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use stream_download::http::reqwest::{Client, Url};
use tokio::sync::OnceCell;

use super::dedup::dedup;
use super::name_query::NameQuery;
use super::{
    rank, CatalogError, CatalogFuture, CatalogSource, Health, SearchPage, SearchQuery, Station,
};

/// Name whose addresses are all the mirrors.
const ALL_MIRRORS: &str = "all.api.radio-browser.info";
/// Stands in when DNS gives nothing.
const FALLBACK_SERVER: &str = "https://de1.api.radio-browser.info/";
const USER_AGENT: &str = concat!("Soundfolk/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ATTEMPTS: usize = 3;
/// Mirrors are looked up again this often, as they come and go.
const REFRESH_AFTER: Duration = Duration::from_secs(60 * 60);
/// After DNS gave nothing, it is asked again this soon.
const RETRY_DNS_AFTER: Duration = Duration::from_secs(60);
/// Catalog pages fetched at most per search: unsupported (HLS) stations are dropped, so a page can
/// come back short; we fetch a few more to fill it without hammering the API.
const MAX_PAGES_PER_SEARCH: usize = 3;
/// UUIDs asked for in one `byuuid` request: a curated list holds 40 at most, its URL stays short.
const UUIDS_PER_REQUEST: usize = 50;

#[derive(Default)]
pub struct RadioBrowser {
    /// Mirrors found by the last lookup (`None`: to look up).
    mirrors: Mutex<Option<Mirrors>>,
    /// One lookup at a time.
    lookup: tokio::sync::Mutex<()>,
    /// Address of the last mirror that responded, tried first.
    preferred: Mutex<Option<String>>,
    /// Whether the catalog still treats `%` in a name search as "any characters".
    wildcards: OnceCell<bool>,
}

/// One mirror: its base URL and a client that reaches it.
struct Mirror {
    /// Its address (the fallback's host name), to remember it and in logs.
    key: String,
    base: Url,
    client: Client,
}

#[derive(Clone)]
struct Mirrors {
    list: Arc<Vec<Mirror>>,
    /// DNS gave nothing: the fallback stands in.
    fallback: bool,
    at: Instant,
}

impl Mirrors {
    fn stale(&self) -> bool {
        self.at.elapsed()
            > if self.fallback {
                RETRY_DNS_AFTER
            } else {
                REFRESH_AFTER
            }
    }
}

fn client(pinned: Option<SocketAddr>) -> Result<Client, CatalogError> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT);
    if let Some(addr) = pinned {
        builder = builder.resolve(ALL_MIRRORS, addr);
    }
    builder
        .build()
        .map_err(|e| CatalogError::Network(e.to_string()))
}

/// Mirrors from the addresses DNS gave, in a shuffled order; the fallback when there are none.
fn mirrors_from(addrs: impl IntoIterator<Item = SocketAddr>) -> Result<Mirrors, CatalogError> {
    let mut addrs: Vec<SocketAddr> = addrs.into_iter().collect();
    addrs.sort();
    addrs.dedup();
    if addrs.is_empty() {
        let fallback = Mirror {
            key: FALLBACK_SERVER.to_owned(),
            base: Url::parse(FALLBACK_SERVER).expect("valid constant URL"),
            client: client(None)?,
        };
        return Ok(Mirrors {
            list: Arc::new(vec![fallback]),
            fallback: true,
            at: Instant::now(),
        });
    }
    // Spreads load across mirrors without a random-number dependency.
    let shift = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize)
        % addrs.len();
    addrs.rotate_left(shift);
    let base = Url::parse(&format!("https://{ALL_MIRRORS}/")).expect("valid constant URL");
    let list = addrs
        .into_iter()
        .map(|addr| {
            Ok(Mirror {
                key: addr.ip().to_string(),
                base: base.clone(),
                client: client(Some(addr))?,
            })
        })
        .collect::<Result<Vec<_>, CatalogError>>()?;
    Ok(Mirrors {
        list: Arc::new(list),
        fallback: false,
        at: Instant::now(),
    })
}

/// `count` mirrors to try, the one that last responded first.
fn candidates<'a>(list: &'a [Mirror], preferred: Option<&str>, count: usize) -> Vec<&'a Mirror> {
    let mut ordered: Vec<&Mirror> = list.iter().collect();
    if let Some(i) = ordered
        .iter()
        .position(|m| Some(m.key.as_str()) == preferred)
    {
        let first = ordered.remove(i);
        ordered.insert(0, first);
    }
    ordered.truncate(count);
    ordered
}

impl RadioBrowser {
    /// The mirrors, looked up again when needed (see the module).
    async fn mirrors(&self) -> Result<Arc<Vec<Mirror>>, CatalogError> {
        let fresh =
            |slot: &Option<Mirrors>| slot.as_ref().filter(|m| !m.stale()).map(|m| m.list.clone());
        if let Some(list) = fresh(&self.mirrors.lock().unwrap_or_else(|p| p.into_inner())) {
            return Ok(list);
        }
        let _one_at_a_time = self.lookup.lock().await;
        // Looked up by another request meanwhile.
        if let Some(list) = fresh(&self.mirrors.lock().unwrap_or_else(|p| p.into_inner())) {
            return Ok(list);
        }
        let found = match tokio::net::lookup_host((ALL_MIRRORS, 443)).await {
            Ok(addrs) => mirrors_from(addrs)?,
            Err(e) => {
                tracing::warn!("cannot look up the Radio-Browser mirrors: {e}");
                mirrors_from([])?
            }
        };
        tracing::debug!(mirrors = ?found.list.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), "Radio-Browser mirrors");
        let list = found.list.clone();
        *self.mirrors.lock().unwrap_or_else(|p| p.into_inner()) = Some(found);
        Ok(list)
    }

    /// Checks once that `%` still means "any characters" in a name search. It does today (the
    /// catalog passes the text to SQL `LIKE`), but the documentation does not promise it.
    /// Thousands of names hold "radio" then an "a"; none holds the text `radio%a` itself.
    async fn wildcards(&self) -> Result<bool, CatalogError> {
        self.wildcards
            .get_or_try_init(|| async {
                let params = [
                    ("name", "radio%a".to_string()),
                    ("limit", "1".to_string()),
                    ("hidebroken", "false".to_string()),
                ];
                let body = self.get("json/stations/search", &params).await?;
                let (_, found) = parse_stations(&body)?;
                if found == 0 {
                    tracing::warn!(
                        "the catalog no longer reads % as a wildcard: slower name search"
                    );
                }
                Ok(found > 0)
            })
            .await
            .copied()
    }

    /// GET on an API path, trying the mirrors one after the other.
    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Vec<u8>, CatalogError> {
        self.request(path, params, MAX_ATTEMPTS).await
    }

    /// GET on an API path, on one mirror only, the one that last responded.
    async fn get_once(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<u8>, CatalogError> {
        self.request(path, params, 1).await
    }

    async fn request(
        &self,
        path: &str,
        params: &[(&str, String)],
        attempts: usize,
    ) -> Result<Vec<u8>, CatalogError> {
        let list = self.mirrors().await?;
        let preferred = self
            .preferred
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut last_error = CatalogError::Network("no server available".into());
        for mirror in candidates(&list, preferred.as_deref(), attempts) {
            let mut url = mirror
                .base
                .join(path)
                .map_err(|e| CatalogError::Invalid(e.to_string()))?;
            url.query_pairs_mut()
                .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));

            match mirror
                .client
                .get(url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => {
                        *self.preferred.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(mirror.key.clone());
                        return Ok(bytes.to_vec());
                    }
                    Err(e) => last_error = CatalogError::Network(e.to_string()),
                },
                Err(e) => last_error = CatalogError::Network(e.to_string()),
            }
            tracing::warn!(server = %mirror.key, "Radio-Browser mirror failed: {last_error}");
        }
        // Every mirror tried failed: the list may be out of date.
        if attempts > 1 || list.len() == 1 {
            *self.mirrors.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
        Err(last_error)
    }
}

impl CatalogSource for RadioBrowser {
    fn random<'a>(&'a self, limit: u32) -> CatalogFuture<'a, Vec<Station>> {
        Box::pin(async move {
            let params = [
                ("order", "random".to_owned()),
                ("hidebroken", "true".to_owned()),
                ("limit", limit.clamp(1, super::MAX_LIMIT).to_string()),
            ];
            let body = self.get("json/stations/search", &params).await?;
            Ok(parse_stations(&body)?.0)
        })
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> CatalogFuture<'a, SearchPage> {
        Box::pin(async move {
            let q = query.sanitized()?;
            let limit = q.limit.unwrap_or(super::DEFAULT_LIMIT);
            let mut offset = q.offset.unwrap_or(0);
            let mut stations = Vec::new();
            let mut exhausted = false;

            // What to send as `name`, and how to filter what comes back (see `NameQuery`).
            let name_query = q.text.as_deref().and_then(NameQuery::new);
            let (name, filter) = match &name_query {
                None => (None, None),
                Some(n) if n.is_single() => (Some(n.longest_fragment()), None),
                Some(n) if self.wildcards().await? => (Some(n.wildcard_term()), Some((n, true))),
                Some(n) => (Some(n.longest_fragment()), Some((n, false))),
            };
            let wanted = |s: &Station| {
                s.is_supported() && filter.is_none_or(|(n, trust)| n.matches(&s.name, trust))
            };

            for _ in 0..MAX_PAGES_PER_SEARCH {
                let params = search_params(&q, name.as_deref(), limit, offset);
                let body = self.get("json/stations/search", &params).await?;
                let (page, raw_len) = parse_stations(&body)?;
                offset += raw_len as u32;
                stations.extend(page.into_iter().filter(wanted));
                if name_query.is_some() {
                    // Healthy entries first, so that a duplicate group keeps a playable one.
                    rank(&mut stations);
                    dedup(&mut stations, &q.favorites);
                }

                if raw_len < limit as usize {
                    exhausted = true;
                    break;
                }
                if stations.len() >= limit as usize {
                    break;
                }
            }

            rank(&mut stations);
            Ok(SearchPage {
                stations,
                next_offset: (!exhausted).then_some(offset),
            })
        })
    }

    fn station<'a>(&'a self, id: &'a str) -> CatalogFuture<'a, Option<Station>> {
        Box::pin(async move {
            // The ID ends up in the URL path: only a UUID is accepted.
            if !is_uuid(id) {
                return Err(CatalogError::BadRequest(format!("Invalid ID: {id}")));
            }
            let body = self.get(&format!("json/stations/byuuid/{id}"), &[]).await?;
            let (stations, _) = parse_stations(&body)?;
            Ok(stations.into_iter().next())
        })
    }

    fn stations<'a>(&'a self, ids: &'a [String]) -> CatalogFuture<'a, Vec<Station>> {
        Box::pin(async move {
            if let Some(id) = ids.iter().find(|id| !is_uuid(id)) {
                return Err(CatalogError::BadRequest(format!("Invalid ID: {id}")));
            }
            let mut found = Vec::new();
            for chunk in ids.chunks(UUIDS_PER_REQUEST) {
                let body = self
                    .get("json/stations/byuuid", &[("uuids", chunk.join(","))])
                    .await?;
                found.extend(parse_stations(&body)?.0);
            }
            Ok(ids
                .iter()
                .filter_map(|id| {
                    found
                        .iter()
                        .find(|s| s.id.eq_ignore_ascii_case(id))
                        .cloned()
                })
                .collect())
        })
    }

    fn by_stream<'a>(&'a self, url: &'a str) -> CatalogFuture<'a, Option<Station>> {
        Box::pin(async move {
            let body = self
                .get("json/stations/byurl", &[("url", url.to_owned())])
                .await?;
            let (mut stations, _) = parse_stations(&body)?;
            stations.retain(Station::is_supported);
            rank(&mut stations);
            Ok(stations.into_iter().next())
        })
    }

    fn count_click<'a>(&'a self, station_id: &'a str) -> CatalogFuture<'a, ()> {
        Box::pin(async move {
            // The ID ends up in the URL path: only a UUID is accepted.
            if !is_uuid(station_id) {
                return Err(CatalogError::BadRequest(format!(
                    "Invalid ID: {station_id}"
                )));
            }
            // One mirror, no retry: each mirror counts a listener once a day on its own, so a
            // retry elsewhere (after a timeout the first one may have counted) would count twice.
            self.get_once(&format!("json/url/{station_id}"), &[])
                .await
                .map(|_| ())
        })
    }
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
/// `name` replaces `q.text`, which [`NameQuery`] rewrites.
fn search_params(
    q: &SearchQuery,
    name: Option<&str>,
    limit: u32,
    offset: u32,
) -> Vec<(&'static str, String)> {
    let mut params = vec![
        // Descending popularity: dead stations (0 clicks) naturally end up at the bottom.
        ("order", "clickcount".to_string()),
        ("reverse", "true".to_string()),
        // We want to show suspect stations (see `Health::Unreachable`), not hide them.
        ("hidebroken", "false".to_string()),
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
    ];
    if let Some(name) = name {
        params.push(("name", name.to_owned()));
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
    let parsed = Url::parse(&url)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))?;
    if !is_uuid(&raw.stationuuid) {
        return None;
    }

    let health = if raw.hls == Some(1) || parsed.path().to_ascii_lowercase().ends_with(".m3u8") {
        Health::Unsupported {
            reason: "hls".into(),
        }
    } else if raw.lastcheckok == Some(0) {
        Health::Unreachable {
            last_ok: non_empty(raw.lastcheckoktime_iso8601),
        }
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

/// A Radio-Browser station id: a UUID in its usual text form.
pub(crate) fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn mirrors_are_reached_under_the_common_name_pinned_to_their_address() {
        let found = mirrors_from([
            addr("91.98.4.78:443"),
            addr("10.0.0.2:443"),
            addr("91.98.4.78:443"),
        ])
        .unwrap();
        assert!(!found.fallback);
        let mut keys: Vec<_> = found.list.iter().map(|m| m.key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["10.0.0.2", "91.98.4.78"]); // duplicates dropped
        assert!(found
            .list
            .iter()
            .all(|m| m.base.as_str() == "https://all.api.radio-browser.info/"));
    }

    #[test]
    fn without_dns_a_known_mirror_stands_in_and_dns_is_asked_again_soon() {
        let mut found = mirrors_from([]).unwrap();
        assert!(found.fallback);
        assert_eq!(found.list[0].base.as_str(), FALLBACK_SERVER);
        assert!(!found.stale());
        found.at = Instant::now() - RETRY_DNS_AFTER - Duration::from_secs(1);
        assert!(found.stale());

        let mut dns = mirrors_from([addr("10.0.0.2:443")]).unwrap();
        dns.at = Instant::now() - RETRY_DNS_AFTER - Duration::from_secs(1);
        assert!(!dns.stale(), "a DNS list lasts longer");
        dns.at = Instant::now() - REFRESH_AFTER - Duration::from_secs(1);
        assert!(dns.stale());
    }

    #[test]
    fn the_mirror_that_answered_is_tried_first() {
        let found = mirrors_from([
            addr("10.0.0.1:443"),
            addr("10.0.0.2:443"),
            addr("10.0.0.3:443"),
        ])
        .unwrap();
        let keys = |preferred: Option<&str>, count| -> Vec<String> {
            candidates(&found.list, preferred, count)
                .iter()
                .map(|m| m.key.clone())
                .collect()
        };
        assert_eq!(keys(Some("10.0.0.2"), 3)[0], "10.0.0.2");
        assert_eq!(keys(Some("10.0.0.2"), 1), ["10.0.0.2"]); // a click goes to that one only
        assert_eq!(keys(Some("10.9.9.9"), 3).len(), 3); // gone from the list: any order
        assert_eq!(keys(None, 2).len(), 2);
    }

    /// Against the real API: the mirrors come from DNS, and they answer under the common name.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_mirrors_come_from_dns() {
        let rb = RadioBrowser::default();
        let list = rb.mirrors().await.unwrap();
        assert!(!list.is_empty());
        assert!(
            list.iter().all(|m| m.base.host_str() == Some(ALL_MIRRORS)),
            "fallback used"
        );
        let body = rb.get("json/stats", &[]).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("stations"));
    }

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
        assert_eq!(
            dead.health,
            Health::Unreachable {
                last_ok: Some("2026-10-01T10:00:00Z".into())
            }
        );

        assert_eq!(
            stations[1].health,
            Health::Unsupported {
                reason: "hls".into()
            }
        );

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
            ..Default::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(q.text.as_deref(), Some("jazz"));
        assert_eq!(q.country_code.as_deref(), Some("FR"));
        assert_eq!(q.tag.as_deref(), Some("soul"));
        assert_eq!(q.limit, Some(super::super::MAX_LIMIT));

        let pasted = |text: &str| {
            SearchQuery {
                text: Some(text.into()),
                ..Default::default()
            }
            .sanitized()
            .unwrap()
            .text
        };
        assert_eq!(
            pasted("Wonder 80’s “Flashback”").as_deref(),
            Some("Wonder 80's \"Flashback\"")
        );
        assert_eq!(
            pasted("Radio–Canada — Ici…").as_deref(),
            Some("Radio-Canada - Ici...")
        );
        assert_eq!(
            pasted("\u{feff} Radio\t\u{a0} Fr\u{ad}ance\u{200b}\n").as_deref(),
            Some("Radio France")
        );
        assert_eq!(pasted(" \u{200b} "), None);

        let empty = SearchQuery {
            text: Some("   ".into()),
            ..Default::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(empty.text, None);
        assert!(SearchQuery {
            country_code: Some("FRA".into()),
            ..Default::default()
        }
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
    async fn live_search_finds_every_spelling() {
        let rb = RadioBrowser::default();
        assert!(rb.wildcards().await.unwrap());
        // The catalog stores "Wonder 80's Flashback".
        assert!(names(&rb, "Wonder 80’s Flashback")
            .await
            .contains(&"Wonder 80's Flashback".to_owned()));
        // Both apostrophes and the spaced form, nothing else in between.
        let rock = names(&rb, "Rock'n'Roll").await;
        assert!(rock.iter().any(|n| n.contains('’')), "{rock:?}");
        assert!(rock.iter().any(|n| n.contains("Rock 'n' Roll")), "{rock:?}");
        assert!(
            !rock.iter().any(|n| n.contains("Rock and Roll")),
            "{rock:?}"
        );
        // "Radio-Canada" and "Radio–Canada".
        let canada = names(&rb, "Radio-Canada").await;
        assert!(
            canada.iter().any(|n| n.contains("Radio-Canada")),
            "{canada:?}"
        );
    }

    /// Against the real API: `%` is searched as typed, `_` as a space.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_search_reads_percent_and_underscore_as_typed() {
        let rb = RadioBrowser::default();
        let percent = names(&rb, "100%").await;
        assert!(!percent.is_empty());
        assert!(percent.iter().all(|n| n.contains('%')), "{percent:?}");
        let underscore = names(&rb, "radio_x").await;
        assert!(
            underscore.iter().any(|n| n.contains("Radio X")),
            "{underscore:?}"
        );
    }

    async fn names(rb: &RadioBrowser, text: &str) -> Vec<String> {
        let query = SearchQuery {
            text: Some(text.into()),
            limit: Some(100),
            ..Default::default()
        };
        rb.search(&query)
            .await
            .unwrap()
            .stations
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    /// Against the real API: `cargo test -- --ignored live_search`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_search_finds_rdwa() {
        let rb = RadioBrowser::default();
        let query = SearchQuery {
            text: Some("rdwa".into()),
            ..Default::default()
        };
        let page = rb.search(&query).await.unwrap();
        assert!(page
            .stations
            .iter()
            .any(|s| s.id == "ab30ca84-c228-4f57-8cac-816c69ef651d"));
    }

    #[test]
    fn unsupported_stations_are_filtered_out() {
        let (stations, _) = parse_stations(FIXTURE.as_bytes()).unwrap();
        let supported: Vec<_> = stations
            .iter()
            .filter(|s| s.is_supported())
            .map(|s| s.name.as_str())
            .collect();
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
        let params = search_params(&q, Some("jazz"), 30, 60);
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("limit"), Some("30"));
        assert_eq!(get("offset"), Some("60"));
        assert_eq!(get("name"), Some("jazz"));
        assert_eq!(get("countrycode"), Some("FR"));
        assert_eq!(get("tag"), Some("soul"));
        assert_eq!(get("hidebroken"), Some("false"));
    }

    /// Against the real API: `cargo test -- --ignored live_station`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_station_by_id() {
        let rb = RadioBrowser::default();
        let station = rb
            .station("ab30ca84-c228-4f57-8cac-816c69ef651d")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(station.id, "ab30ca84-c228-4f57-8cac-816c69ef651d");
        assert!(rb.station("../../etc/passwd").await.is_err());
    }

    /// Against the real API: `cargo test -- --ignored live_stations`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_stations_by_id_keep_the_order_asked() {
        let rb = RadioBrowser::default();
        let ids = [
            "9615628c-0601-11e8-ae97-52543be04c81", // Radio Zinzine
            "00000000-0000-4000-8000-000000000000", // not in the catalog
            "ab30ca84-c228-4f57-8cac-816c69ef651d", // RDWA
        ]
        .map(String::from);
        let found = rb.stations(&ids).await.unwrap();
        assert_eq!(
            found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            [ids[0].as_str(), ids[2].as_str()]
        );
        assert!(rb.stations(&["a,b".to_owned()]).await.is_err());
    }

    /// Against the real API: HLS stations never come back, and paging moves forward.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_search_hides_hls_and_paginates() {
        let rb = RadioBrowser::default();
        // Korean broadcasters are mostly HLS: a good stress test for short pages.
        let query = SearchQuery {
            text: Some("kbs".into()),
            limit: Some(10),
            ..Default::default()
        };
        let first = rb.search(&query).await.unwrap();
        assert!(first.stations.iter().all(|s| s.is_supported()));
        if let Some(next) = first.next_offset {
            let second = rb
                .search(&SearchQuery {
                    offset: Some(next),
                    ..query.clone()
                })
                .await
                .unwrap();
            assert!(second.stations.iter().all(|s| s.is_supported()));
            let ids: std::collections::HashSet<_> = first.stations.iter().map(|s| &s.id).collect();
            assert!(
                second.stations.iter().all(|s| !ids.contains(&s.id)),
                "pages overlap"
            );
        }
    }
}
