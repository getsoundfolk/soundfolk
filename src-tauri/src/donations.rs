// SPDX-License-Identifier: MPL-2.0
//! Stations' donation links, from `getsoundfolk/soundfolk-stations`: every entry of that repository,
//! compiled into one `stations.json` by its CI and published on GitHub Pages. See
//! `docs/12-donations.md`.
//!
//! * At startup, the copy cached by the last download is used right away (offline included).
//! * The list is downloaded again when that copy is a day old (see `daily_list.rs`), whatever the
//!   privacy settings: it is part of the catalog, and GitHub sees the IP address once a day at most.
//! * A list that does not parse, or of another `version`, is ignored and the cached one stays.
//!
//! `SOUNDFOLK_STATIONS_URL` replaces the list's address (a fork's list, a local file server).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use soundfolk_nowplaying::recipe::stream_pattern;

use crate::daily_list::{self, Files, CHECK_EVERY, FIRST_CHECK};

/// Where the list is published. `v1` is the list format: a change older apps can't read moves it.
pub const DEFAULT_URL: &str = "https://getsoundfolk.github.io/soundfolk-stations/v1/stations.json";
const VERSION: u32 = 1;

const FILES: Files = Files {
    list: "stations.json",
    etag: "stations.etag",
};

#[derive(Debug, Deserialize)]
struct List {
    version: u32,
    stations: Vec<Entry>,
}

/// One station of the list. Unknown keys are ignored: new optional fields keep `v1`.
#[derive(Debug, Clone, Deserialize)]
struct Entry {
    #[serde(rename = "match")]
    matches: Match,
    donate: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Match {
    #[serde(default)]
    stations: Vec<String>,
    #[serde(default)]
    streams: Vec<String>,
}

impl Match {
    /// Same rules as a recipe's `match`: a Radio-Browser UUID, or a stream (`host/path`, a trailing
    /// `*` for any path starting with it).
    fn matches(&self, station_id: Option<&str>, stream: &str) -> bool {
        if let Some(id) = station_id {
            if self.stations.iter().any(|s| s.eq_ignore_ascii_case(id)) {
                return true;
            }
        }
        let Some(stream) = stream_pattern(stream) else {
            return false;
        };
        self.streams.iter().any(|pattern| {
            let pattern = pattern.trim().to_ascii_lowercase();
            match pattern.strip_suffix('*') {
                Some(prefix) => stream.starts_with(prefix),
                None => stream == pattern.trim_end_matches('/'),
            }
        })
    }
}

/// A station's donation page, as the player shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DonationLink {
    pub url: String,
    /// Where the money goes, shown next to the link: `helloasso.com`.
    pub domain: String,
}

fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let list: List = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if list.version != VERSION {
        return Err(format!(
            "version {} (this app reads {VERSION})",
            list.version
        ));
    }
    Ok(list.stations)
}

/// The domain of an `https://` URL, without `www.`; `None` for anything else.
fn domain(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    (!host.is_empty()).then(|| host.to_owned())
}

/// The links known so far.
#[derive(Debug, Default)]
pub struct Donations(RwLock<Vec<Entry>>);

impl Donations {
    fn set(&self, entries: Vec<Entry>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = entries;
    }

    /// The donation page of this station (Radio-Browser UUID) or stream, if the list has one.
    pub fn find(&self, station_id: Option<&str>, stream: &str) -> Option<DonationLink> {
        let entries = self.0.read().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .filter(|e| e.matches.matches(station_id, stream))
            .find_map(|e| {
                Some(DonationLink {
                    domain: domain(&e.donate)?,
                    url: e.donate.clone(),
                })
            })
    }
}

/// Loads the cached list, then keeps it up to date in the background. `on_change` runs after each
/// download that changed the list (the player asks again for its link).
pub fn start(
    donations: Arc<Donations>,
    cache_dir: Option<PathBuf>,
    on_change: impl Fn() + Send + 'static,
) {
    let Some(dir) = cache_dir else { return };
    if let Some(entries) = read_cached(&dir) {
        tracing::info!("donation links: {} from the cache", entries.len());
        donations.set(entries);
    }
    let url = std::env::var("SOUNDFOLK_STATIONS_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    tauri::async_runtime::spawn(async move {
        let client = match daily_list::client() {
            Ok(client) => client,
            Err(e) => return tracing::warn!("donation links unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if daily_list::is_due(&dir, FILES) {
                match daily_list::refresh(&client, &url, &dir, FILES, parse).await {
                    Ok(Some(entries)) => {
                        tracing::info!("donation links: {} downloaded", entries.len());
                        donations.set(entries);
                        on_change();
                    }
                    Ok(None) => tracing::debug!("donation links: unchanged"),
                    Err(e) => tracing::warn!("donation links: {e}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The donation page of the station being played or shown, if the list has one.
#[tauri::command]
pub async fn get_donation_link(
    donations: tauri::State<'_, Arc<Donations>>,
    station_id: Option<String>,
    url: String,
) -> crate::error::AppResult<Option<DonationLink>> {
    Ok(donations.find(station_id.as_deref(), &url))
}

fn read_cached(dir: &Path) -> Option<Vec<Entry>> {
    daily_list::read_cached(dir, FILES, parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"version": 1, "stations": [
        {"id": "radio-zinzine", "name": "Radio Zinzine", "future": true,
         "match": {"stations": ["9615628c-0601-11e8-ae97-52543be04c81", "960bff9d-0601-11e8-ae97-52543be04c81"]},
         "donate": "https://www.helloasso.com/associations/association-radio-zinzine/formulaires/1"},
        {"id": "by-stream", "name": "By stream",
         "match": {"stations": ["00000000-0000-0000-0000-000000000000"], "streams": ["live.example.org/radio*"]},
         "donate": "https://example.org/don"},
        {"id": "not-https", "name": "Not https",
         "match": {"stations": ["11111111-1111-1111-1111-111111111111"]},
         "donate": "http://example.net/don"}
    ]}"#;

    fn donations() -> Donations {
        let donations = Donations::default();
        donations.set(parse(LIST).unwrap());
        donations
    }

    #[test]
    fn finds_a_station_by_any_of_its_uuids() {
        let link = donations()
            .find(
                Some("960BFF9D-0601-11E8-AE97-52543BE04C81"),
                "https://other.example/live",
            )
            .unwrap();
        assert_eq!(
            link.url,
            "https://www.helloasso.com/associations/association-radio-zinzine/formulaires/1"
        );
        assert_eq!(link.domain, "helloasso.com");
    }

    #[test]
    fn finds_a_personal_station_by_its_stream() {
        let donations = donations();
        let link = donations.find(Some("personal-1"), "http://live.example.org/radio.mp3?x=1");
        assert_eq!(link.unwrap().domain, "example.org");
        assert_eq!(donations.find(None, "http://live.example.org/other"), None);
    }

    #[test]
    fn a_link_that_is_not_https_is_never_shown() {
        assert_eq!(
            donations().find(Some("11111111-1111-1111-1111-111111111111"), ""),
            None
        );
    }

    #[test]
    fn another_version_or_a_broken_list_is_refused() {
        assert!(parse(r#"{"version": 2, "stations": []}"#).is_err());
        assert!(parse("<html>").is_err());
        assert_eq!(parse(r#"{"version": 1, "stations": []}"#).unwrap().len(), 0);
    }

    #[test]
    fn domains_drop_www_ports_and_credentials() {
        assert_eq!(
            domain("https://www.HelloAsso.com/x").as_deref(),
            Some("helloasso.com")
        );
        assert_eq!(
            domain("https://u@radio.example:8443/don?a=1").as_deref(),
            Some("radio.example")
        );
        assert_eq!(domain("http://example.org/"), None);
        assert_eq!(domain("https:///x"), None);
    }

    #[test]
    fn the_cache_is_read_back() {
        let dir = std::env::temp_dir().join(format!("soundfolk-donations-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_cached(&dir).is_none());
        std::fs::write(dir.join(FILES.list), LIST).unwrap();
        assert_eq!(read_cached(&dir).unwrap().len(), 3);
        std::fs::write(dir.join(FILES.list), "<html>").unwrap();
        assert!(read_cached(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
