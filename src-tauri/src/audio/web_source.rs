// SPDX-License-Identifier: MPL-2.0
//! Title from the web, for streams that do not announce it (see `docs/07-now-playing.md`).
//!
//! For each playback, in this order:
//! 1. a **recipe** matching the station (local recipes first, then the bundled list): its page
//!    is read from the start;
//! 2. otherwise, once the stream is open and has announced nothing for a while, the streaming
//!    **server's status page** (Icecast, Shoutcast), if it has the expected shape.
//!
//! A title announced by the stream itself always wins: the task stops as soon as one arrives.
//! The page is read only while the station plays, at the rate the tracker decides (minimum
//! interval, hourly cap, jitter), with an identifiable User-Agent.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use soundfolk_nowplaying::adapters::{self, Adapter};
use soundfolk_nowplaying::clock::Offset;
use soundfolk_nowplaying::recipe::{Recipe, RecipeBook};
use soundfolk_nowplaying::tracker::{Policy, Scheduled, Tracker};
use soundfolk_nowplaying::{extract, SignedDuration, Snapshot, Timestamp};
use stream_download::http::reqwest::{header, Client, Url};

use super::{Cancel, Reporter, StationRef, TitleSource};

/// Network and audio buffers between the broadcast and what the listener hears (the output
/// buffer holds up to 2 s, servers send a few seconds in a burst on connect).
const AUDIO_LATENCY: SignedDuration = SignedDuration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Larger pages are not read (a playlist page is a few dozen KB).
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
/// How long a stream that carries metadata gets to announce a title before the server's
/// status page is tried.
const INBAND_GRACE: Duration = Duration::from_secs(20);
/// Longest wait for the stream to open before giving up on the server's status page.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// The task checks for cancellation (stop, other station) at least this often.
const TICK: Duration = Duration::from_millis(500);

/// Produces the recipes to use, highest priority first (read at each playback, so a local
/// recipe applies from the next play without restarting).
pub type RecipeLoader = Box<dyn Fn() -> RecipeBook + Send + Sync>;

/// Web title sources, shared by every playback.
pub struct WebTitles {
    recipes: RecipeLoader,
    enabled: AtomicBool,
    /// UTC offset learned per station (station ID or stream URL), for local `HH:MM:SS` times.
    offsets: Mutex<HashMap<String, Offset>>,
    /// Server status adapter found per stream origin (`None`: none works).
    servers: Mutex<HashMap<String, Option<Adapter>>>,
    client: OnceLock<Option<Client>>,
}

enum Source {
    Recipe(Box<Recipe>),
    Server(Adapter),
}

impl WebTitles {
    pub fn new(recipes: RecipeLoader, enabled: bool) -> Self {
        Self {
            recipes,
            enabled: AtomicBool::new(enabled),
            offsets: Mutex::default(),
            servers: Mutex::default(),
            client: OnceLock::new(),
        }
    }

    /// Turns web titles on or off (applies from the next playback).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn spawn(
        self: &Arc<Self>,
        stream: Url,
        station: StationRef,
        reporter: Reporter,
        cancel: Arc<Cancel>,
        runtime: &tokio::runtime::Handle,
    ) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        runtime.spawn(async move { this.run(stream, station, reporter, cancel).await });
    }

    fn client(&self) -> Option<&Client> {
        self.client
            .get_or_init(|| {
                Client::builder()
                    .user_agent(concat!(
                        "Soundfolk/",
                        env!("CARGO_PKG_VERSION"),
                        " (+https://github.com/raphaelh/soundfolk)"
                    ))
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .inspect_err(|e| tracing::warn!("web titles unavailable: {e}"))
                    .ok()
            })
            .as_ref()
    }

    async fn run(self: Arc<Self>, stream: Url, station: StationRef, reporter: Reporter, cancel: Arc<Cancel>) {
        let Some(client) = self.client().cloned() else { return };
        let book = (self.recipes)();
        let source = match book.find(station.id.as_deref(), stream.as_str()) {
            Some(recipe) => Source::Recipe(Box::new(recipe.clone())),
            None => match self.find_server(&client, &stream, &reporter, &cancel).await {
                Some(adapter) => Source::Server(adapter),
                None => return,
            },
        };
        let (url, every, kind) = match &source {
            Source::Recipe(recipe) => {
                tracing::info!(recipe = %recipe.name, "title from the station's web page");
                (recipe.fetch.url.clone(), recipe.every(), TitleSource::Recipe)
            }
            Source::Server(adapter) => {
                tracing::info!(adapter = adapter.name(), "title from the server's status page");
                let Some(probe) = adapter.probe(stream.as_str()) else { return };
                (probe.url, None, TitleSource::Server)
            }
        };

        let key = station.id.clone().unwrap_or_else(|| stream.to_string());
        let remembered = self.offsets.lock().unwrap_or_else(|p| p.into_inner()).get(&key).copied();
        let policy = Policy { latency: AUDIO_LATENCY, ..Policy::default() };
        let mut tracker = Tracker::new(policy, every, remembered, seed(&key));
        let mut pending: VecDeque<Scheduled> = VecDeque::new();
        let mut next_fetch = Timestamp::now();

        while !cancel.is_cancelled() && !reporter.stream_meta().has_inband_title() {
            let now = Timestamp::now();
            // Only the latest due change matters.
            let mut due = None;
            while pending.front().is_some_and(|s| s.at <= now) {
                due = pending.pop_front();
            }
            if let Some(scheduled) = due {
                reporter.web_title(&scheduled.on_air, kind);
            }

            if now >= next_fetch {
                let snapshot = match fetch(&client, &url).await {
                    Ok((page, date)) => read(&source, stream.as_str(), &page).map(|s| (s, date)),
                    Err(e) => Err(e),
                };
                match snapshot {
                    Ok((snapshot, date)) => {
                        let plan = tracker.on_snapshot(snapshot, Timestamp::now(), date);
                        pending = plan.publish.into();
                        next_fetch = plan.next_fetch;
                        if let Some(offset) = tracker.offset() {
                            self.offsets.lock().unwrap_or_else(|p| p.into_inner()).insert(key.clone(), offset);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(%url, "web title unavailable: {e}");
                        next_fetch = tracker.on_failure(now);
                    }
                }
                continue; // what is due right away is published at once
            }

            let wake = pending.front().map_or(next_fetch, |s| s.at.min(next_fetch));
            let wait = wake.duration_since(now).unsigned_abs().min(TICK);
            tokio::time::sleep(wait).await;
        }
    }

    /// The streaming server's status adapter, once the stream turned out not to announce titles.
    async fn find_server(&self, client: &Client, stream: &Url, reporter: &Reporter, cancel: &Cancel) -> Option<Adapter> {
        let meta = reporter.stream_meta();
        let opened = loop {
            if cancel.is_cancelled() {
                return None;
            }
            if let Some(opened) = meta.opened() {
                break opened;
            }
            tokio::time::sleep(TICK).await;
            // `opened` is set by the decoder; give up if it never comes (failed connection).
            if reporter_age(reporter) > OPEN_TIMEOUT {
                return None;
            }
        };
        if opened.inband {
            // The stream can carry titles: give it time to announce one.
            while opened.at.elapsed() < INBAND_GRACE {
                if cancel.is_cancelled() || meta.has_inband_title() {
                    return None;
                }
                tokio::time::sleep(TICK).await;
            }
            if meta.has_inband_title() {
                return None;
            }
        }

        let origin = origin_key(stream);
        if let Some(known) = self.servers.lock().unwrap_or_else(|p| p.into_inner()).get(&origin) {
            return *known;
        }
        let mut found = None;
        for adapter in adapters::candidates(opened.server.as_deref()) {
            if cancel.is_cancelled() {
                return None;
            }
            let Some(probe) = adapter.probe(stream.as_str()) else { continue };
            // A matching URL is not proof: the document must have the adapter's shape.
            if let Ok((page, _)) = fetch(client, &probe.url).await {
                if adapter.read(stream.as_str(), &page).is_ok() {
                    found = Some(adapter);
                    break;
                }
            }
        }
        self.servers.lock().unwrap_or_else(|p| p.into_inner()).insert(origin, found);
        found
    }
}

fn read(source: &Source, stream: &str, page: &str) -> Result<Snapshot, String> {
    match source {
        Source::Recipe(recipe) => extract::read(recipe, page),
        Source::Server(adapter) => adapter.read(stream, page),
    }
    .map_err(|e| e.to_string())
}

/// The page and its HTTP `Date` (the site's clock, used to resolve local times).
async fn fetch(client: &Client, url: &str) -> Result<(String, Option<Timestamp>), String> {
    let mut response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let date = response
        .headers()
        .get(header::DATE)
        .and_then(|d| d.to_str().ok())
        .and_then(|d| jiff::fmt::rfc2822::DateTimeParser::new().parse_timestamp(d).ok());
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_PAGE_BYTES {
            return Err("page too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((String::from_utf8_lossy(&body).into_owned(), date))
}

fn origin_key(url: &Url) -> String {
    format!("{}://{}:{}", url.scheme(), url.host_str().unwrap_or_default(), url.port_or_known_default().unwrap_or(0))
}

/// Jitter seed: different for every playback and station.
fn seed(key: &str) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write(key.as_bytes());
    hasher.finish()
}

fn reporter_age(reporter: &Reporter) -> Duration {
    reporter.stream_meta().created.elapsed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_ignore_paths() {
        let a: Url = "https://radio.example.org/live.mp3".parse().unwrap();
        let b: Url = "https://radio.example.org:443/hd.aac".parse().unwrap();
        assert_eq!(origin_key(&a), origin_key(&b));
    }
}
