// SPDX-License-Identifier: MPL-2.0
//! Title from the web, for streams that do not announce it (see `docs/08-now-playing.md`).
//!
//! For each playback, in this order:
//! 1. a **recipe** matching the station (local recipes first, then the shipped ones): its page
//!    is read from the start;
//! 2. otherwise, once the stream is open and has announced nothing for a while, the streaming
//!    **server's status page** (Icecast, Shoutcast), if it has the expected shape.
//!
//! A title announced by the stream itself always wins: the task stops as soon as one arrives.
//! An **AzuraCast** server's API is still read then, once per ICY title, for the details of the
//! track heard (artist and title apart, start, length, cover, next track); the title shown stays
//! the stream's.
//! The page is read only while the station plays, at the rate the tracker decides (minimum
//! interval, hourly cap, jitter), with an identifiable User-Agent.
//!
//! When the recipes change (the user saved a local file), the playback switches to the new
//! recipe at once, without interrupting the sound, and the user is told what it read.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use soundfolk_nowplaying::adapters::{self, Adapter, Probe};
use soundfolk_nowplaying::check;
use soundfolk_nowplaying::clock::Offset;
use soundfolk_nowplaying::tracker::{OnAir, Policy, Scheduled, Tracker};
use soundfolk_nowplaying::{extract, RawTrack, SignedDuration, Snapshot, Start, Timestamp};
use stream_download::http::reqwest::{header, Client, Url};
use tokio::sync::watch;

use crate::audio::{Cancel, Reporter, StationRef, TitleSource, TrackInfo};
use crate::recipes::{Change, Entry, Origin, Recipes};

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
/// Longest wait for a time read on the wall clock: the timers stop while the computer sleeps, the
/// wall clock does not, so a wait counted before a sleep would end late after it.
const CLOCK_CHECK: Duration = Duration::from_secs(5);
/// An ICY title the server API does not know yet is looked up once more after this (the API may
/// lag behind the stream, a cache in front of it too).
const DETAILS_RETRY: Duration = Duration::from_secs(10);
/// Wait after a failed request for details.
const DETAILS_BACKOFF: Duration = Duration::from_secs(60);

/// Where the title of the current playback comes from, for the station sheet.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TitleStatus {
    pub state: SourceState,
    /// The recipe in use, or the one that would be used.
    pub recipe: Option<RecipeRef>,
    /// The server status adapter in use (`icecast`, `shoutcast-v2`…); with `Stream`, the one
    /// giving the details of the stream's titles (`azuracast`).
    pub adapter: Option<String>,
    /// Why the last reading failed (page unreachable, nothing found…).
    pub error: Option<String>,
    /// The station's local recipe file exists but cannot be used.
    pub local_error: Option<LocalError>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// Nothing playing yet.
    #[default]
    Idle,
    /// Web titles are turned off in the settings.
    Off,
    /// Looking for a source (waiting for the stream to announce a title, probing the server).
    Searching,
    /// The stream announces its titles itself.
    Stream,
    Recipe,
    Server,
    /// No source found.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecipeRef {
    pub id: String,
    pub name: String,
    pub origin: Origin,
    pub path: Option<String>,
}

impl From<&Entry> for RecipeRef {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.recipe.name.clone(),
            origin: entry.origin,
            path: entry.path.as_ref().map(|p| p.display().to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalError {
    pub path: String,
    pub error: String,
}

/// Feedback after saving a local recipe.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecipeNotice {
    /// The file name.
    pub file: String,
    pub ok: bool,
    pub message: String,
}

/// What a test reading of a recipe found.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RecipeTest {
    pub recipe: Option<RecipeRef>,
    pub local_error: Option<LocalError>,
    /// The page that was read.
    pub url: Option<String>,
    pub track: Option<TrackInfo>,
    pub next: Option<TrackInfo>,
    pub show: Option<String>,
    /// The station's UTC offset, in minutes, when the page gives local times.
    pub utc_offset_minutes: Option<i32>,
    /// What went wrong, step by step for the user.
    pub error: Option<String>,
}

/// Status changes and notices for the window.
pub enum WebEvent {
    Status(TitleStatus),
    Notice(RecipeNotice),
}

pub type WebListener = Box<dyn Fn(WebEvent) + Send + Sync>;

/// The playback being followed.
#[derive(Clone)]
struct Session {
    id: u64,
    station: StationRef,
    stream: Url,
}

/// Web title sources, shared by every playback.
pub struct WebTitles {
    recipes: Arc<Recipes>,
    enabled: AtomicBool,
    /// UTC offset learned per station (station ID or stream URL), for local `HH:MM:SS` times.
    offsets: Mutex<HashMap<String, Offset>>,
    /// Server status adapter found per stream origin (`None`: none works).
    servers: Mutex<HashMap<String, Option<Adapter>>>,
    /// Streams whose AzuraCast API turned out not to describe them: not read again for details.
    no_details: Mutex<std::collections::HashSet<String>>,
    client: OnceLock<Option<Client>>,
    sessions: AtomicU64,
    current: Mutex<Option<Session>>,
    status: Mutex<TitleStatus>,
    /// Recipe files saved while their station plays: told what they read at the next fetch.
    saved: Mutex<Vec<String>>,
    /// When each page was last read: the minimum interval holds across playbacks and recipe
    /// changes.
    last_fetch: Mutex<HashMap<String, Timestamp>>,
    listener: WebListener,
}

enum Source {
    Recipe(Entry),
    Server(Adapter),
}

impl WebTitles {
    pub fn new(recipes: Arc<Recipes>, enabled: bool, listener: WebListener) -> Self {
        Self {
            recipes,
            enabled: AtomicBool::new(enabled),
            offsets: Mutex::default(),
            servers: Mutex::default(),
            no_details: Mutex::default(),
            client: OnceLock::new(),
            sessions: AtomicU64::new(0),
            current: Mutex::default(),
            status: Mutex::default(),
            saved: Mutex::default(),
            last_fetch: Mutex::default(),
            listener,
        }
    }

    pub fn recipes(&self) -> &Arc<Recipes> {
        &self.recipes
    }

    /// Whether station websites (and the shared recipe list) may be read.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Turns web titles on or off (applies from the next playback).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Where the title of the current playback comes from.
    pub fn status(&self) -> TitleStatus {
        lock(&self.status).clone()
    }

    pub(crate) fn spawn(
        self: &Arc<Self>,
        stream: Url,
        station: StationRef,
        reporter: Reporter,
        cancel: Arc<Cancel>,
        runtime: &tokio::runtime::Handle,
    ) {
        let session = Session {
            id: self.sessions.fetch_add(1, Ordering::SeqCst) + 1,
            station,
            stream,
        };
        *lock(&self.current) = Some(session.clone());
        if !self.enabled.load(Ordering::SeqCst) {
            self.set_status(
                &session,
                TitleStatus {
                    state: SourceState::Off,
                    ..self.expected(&session)
                },
            );
            return;
        }
        self.set_status(
            &session,
            TitleStatus {
                state: SourceState::Searching,
                ..self.expected(&session)
            },
        );
        let this = self.clone();
        runtime.spawn(async move { this.run(session, reporter, cancel).await });
    }

    /// Reloads the local recipes if a file changed, and applies them (playback, notices).
    pub fn reload_recipes(&self) {
        if let Some(changes) = self.recipes.reload_if_changed() {
            self.recipes_changed(&changes);
        }
    }

    /// The local recipes changed: tells the user what became of the files they saved.
    pub fn recipes_changed(&self, changes: &[Change]) {
        let current = lock(&self.current).clone();
        let set = self.recipes.current();
        let playing = current
            .as_ref()
            .and_then(|s| set.find(s.station.id.as_deref(), s.stream.as_str()));
        for change in changes {
            let file = file_name(&change.path);
            let message = match (&change.error, playing) {
                (Some(error), _) => {
                    self.notify(&file, false, format!("Not used: {error}"));
                    continue;
                }
                // The station playing uses it: told what it reads at the next fetch.
                (None, Some(entry))
                    if entry.id == change.id && self.enabled.load(Ordering::SeqCst) =>
                {
                    lock(&self.saved).push(change.id.clone());
                    continue;
                }
                (None, _) if current.is_some() => {
                    "Loaded; it does not apply to the station playing".to_owned()
                }
                (None, _) => "Loaded".to_owned(),
            };
            self.notify(&file, true, message);
        }
        if let Some(session) = current {
            // Refreshes the local error shown for the station.
            let mut status = lock(&self.status).clone();
            status.local_error = self.local_error(&session);
            if status.state != SourceState::Recipe && status.state != SourceState::Server {
                status.recipe = self.expected(&session).recipe;
            }
            self.set_status(&session, status);
        }
    }

    /// Reads a page now: its text and HTTP `Date` (to save it as a recipe's fixture).
    pub async fn read_page(&self, url: &str) -> Result<(String, Option<Timestamp>), String> {
        let client = self.client().ok_or("cannot make web requests")?;
        fetch(client, url).await
    }

    /// Reads the station's recipe page now and says what it finds, for the station sheet.
    pub async fn test(&self, station: &StationRef, stream: &str) -> RecipeTest {
        let set = self.recipes.current();
        let entry = set.find(station.id.as_deref(), stream).cloned();
        let local_error = entry
            .as_ref()
            .and_then(|e| set.error_for(&e.id))
            .map(|e| LocalError {
                path: e.path.display().to_string(),
                error: e.error.clone(),
            });
        let mut report = RecipeTest {
            recipe: entry.as_ref().map(RecipeRef::from),
            local_error,
            ..Default::default()
        };
        let Some(entry) = entry else {
            report.error = Some("No recipe matches this station.".into());
            return report;
        };
        let Some(client) = self.client() else {
            report.error = Some("Cannot make web requests.".into());
            return report;
        };
        let url = entry.recipe.fetch.url.clone();
        report.url = Some(url.clone());
        let (page, date) = match fetch(client, &url).await {
            Ok(page) => page,
            Err(e) => {
                report.error = Some(format!("Could not read the page: {e}."));
                return report;
            }
        };
        let reading = match check::read(&entry.recipe, &page, date.unwrap_or_else(Timestamp::now)) {
            Ok(reading) => reading,
            Err(e) => {
                report.error = Some(format!("The page was read, but: {e}."));
                return report;
            }
        };
        report.utc_offset_minutes = reading.offset.map(|o| o / 60);
        match reading.on_air {
            Some(on_air) => {
                report.track = Some((&on_air.current).into());
                report.next = on_air.next.as_ref().map(TrackInfo::from);
                report.show = on_air.show;
            }
            None => report.error = Some(nothing_found(&reading.snapshot)),
        }
        report
    }

    fn client(&self) -> Option<&Client> {
        self.client
            .get_or_init(|| {
                Client::builder()
                    .user_agent(concat!(
                        "Soundfolk/",
                        env!("CARGO_PKG_VERSION"),
                        " (+https://github.com/getsoundfolk/soundfolk)"
                    ))
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .inspect_err(|e| tracing::warn!("web titles unavailable: {e}"))
                    .ok()
            })
            .as_ref()
    }

    /// The recipe that applies to the session, before anything is read.
    fn expected(&self, session: &Session) -> TitleStatus {
        let set = self.recipes.current();
        TitleStatus {
            recipe: set
                .find(session.station.id.as_deref(), session.stream.as_str())
                .map(RecipeRef::from),
            local_error: self.local_error(session),
            ..Default::default()
        }
    }

    fn local_error(&self, session: &Session) -> Option<LocalError> {
        self.local_error_for(&session.station, session.stream.as_str())
    }

    /// The station's local file that cannot be used: the one replacing the matching shipped
    /// recipe, or an invalid file naming this stream or station. No file is read: the async title
    /// tasks call it, so it works from the text kept when the recipes were loaded.
    pub fn local_error_for(&self, station: &StationRef, stream: &str) -> Option<LocalError> {
        let set = self.recipes.current();
        let by_id = set
            .find(station.id.as_deref(), stream)
            .and_then(|e| set.error_for(&e.id));
        let stream = soundfolk_nowplaying::recipe::stream_pattern(stream);
        let mentions = |text: &str| {
            let text = text.to_ascii_lowercase();
            stream.as_deref().is_some_and(|s| text.contains(s))
                || station
                    .id
                    .as_deref()
                    .is_some_and(|id| text.contains(&id.to_ascii_lowercase()))
        };
        by_id
            .or_else(|| set.errors().iter().find(|e| mentions(&e.text)))
            .map(|e| LocalError {
                path: e.path.display().to_string(),
                error: e.error.clone(),
            })
    }

    fn is_current(&self, session: &Session) -> bool {
        lock(&self.current)
            .as_ref()
            .is_some_and(|s| s.id == session.id)
    }

    fn set_status(&self, session: &Session, status: TitleStatus) {
        if !self.is_current(session) {
            return;
        }
        let mut current = lock(&self.status);
        if *current != status {
            *current = status.clone();
            drop(current);
            (self.listener)(WebEvent::Status(status));
        }
    }

    fn update_status(&self, session: &Session, change: impl FnOnce(&mut TitleStatus)) {
        let mut status = lock(&self.status).clone();
        change(&mut status);
        self.set_status(session, status);
    }

    fn notify(&self, file: &str, ok: bool, message: String) {
        (self.listener)(WebEvent::Notice(RecipeNotice {
            file: file.to_owned(),
            ok,
            message,
        }));
    }

    /// Tells the user what a recipe they just saved reads.
    fn notify_saved(&self, entry: &Entry, outcome: Result<&OnAir, &str>) {
        let saved = {
            let mut saved = lock(&self.saved);
            let before = saved.len();
            saved.retain(|id| *id != entry.id);
            saved.len() != before
        };
        if !saved {
            return;
        }
        let file = entry
            .path
            .as_deref()
            .map(file_name)
            .unwrap_or_else(|| format!("{}.toml", entry.id));
        match outcome {
            Ok(on_air) => self.notify(&file, true, format!("Reads: {}", on_air.current.display())),
            Err(error) => self.notify(&file, false, error.to_owned()),
        }
    }

    async fn run(self: Arc<Self>, session: Session, reporter: Reporter, cancel: Arc<Cancel>) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        let mut server: Option<Option<Adapter>> = None;
        let mut changes = Changes::new(&self.recipes, &reporter, &cancel);
        loop {
            if cancel.is_cancelled() {
                return;
            }
            if reporter.stream_meta().has_inband_title() {
                self.set_status(
                    &session,
                    TitleStatus {
                        state: SourceState::Stream,
                        ..self.expected(&session)
                    },
                );
                self.icy_details(&session, &client, &reporter, &cancel)
                    .await;
                return;
            }
            let generation = self.recipes.generation();
            let set = self.recipes.current();
            let source = match set.find(session.station.id.as_deref(), session.stream.as_str()) {
                Some(entry) => Some(Source::Recipe(entry.clone())),
                None => {
                    if server.is_none() {
                        server = Some(
                            self.find_server(&client, &session.stream, &reporter, &cancel)
                                .await,
                        );
                    }
                    server.flatten().map(Source::Server)
                }
            };
            match source {
                Some(source) => {
                    self.follow(&session, &client, source, generation, &reporter, &cancel)
                        .await
                }
                None => {
                    if !cancel.is_cancelled() && !reporter.stream_meta().has_inband_title() {
                        self.set_status(
                            &session,
                            TitleStatus {
                                state: SourceState::None,
                                ..self.expected(&session)
                            },
                        );
                    }
                    // A recipe may still be written for the station.
                    while self.recipes.generation() == generation
                        && !cancel.is_cancelled()
                        && !reporter.stream_meta().has_inband_title()
                    {
                        changes.next(None).await;
                    }
                }
            }
        }
    }

    /// Reads the source until the playback ends, the stream announces a title or the recipes
    /// change.
    async fn follow(
        &self,
        session: &Session,
        client: &Client,
        source: Source,
        generation: u64,
        reporter: &Reporter,
        cancel: &Cancel,
    ) {
        let stream = session.stream.as_str();
        let (url, every, kind) = match &source {
            Source::Recipe(entry) => {
                tracing::info!(recipe = %entry.id, "title from the station's web page");
                (
                    entry.recipe.fetch.url.clone(),
                    entry.recipe.every(),
                    TitleSource::Recipe,
                )
            }
            Source::Server(adapter) => {
                tracing::info!(
                    adapter = adapter.name(),
                    "title from the server's status page"
                );
                let Some(probe) = adapter.probe(stream) else {
                    return;
                };
                (probe.url, None, TitleSource::Server)
            }
        };
        self.set_status(
            session,
            match &source {
                Source::Recipe(entry) => TitleStatus {
                    state: SourceState::Recipe,
                    recipe: Some(entry.into()),
                    local_error: self.local_error(session),
                    ..Default::default()
                },
                Source::Server(adapter) => TitleStatus {
                    state: SourceState::Server,
                    adapter: Some(adapter.name().to_owned()),
                    local_error: self.local_error(session),
                    ..Default::default()
                },
            },
        );

        let key = session
            .station
            .id
            .clone()
            .unwrap_or_else(|| stream.to_owned());
        let remembered = lock(&self.offsets).get(&key).copied();
        let policy = Policy {
            latency: AUDIO_LATENCY,
            ..Policy::default()
        };
        let mut tracker = Tracker::new(policy, every, remembered, seed(&key));
        let mut pending: VecDeque<Scheduled> = VecDeque::new();
        let floor = Policy::default().floor;
        let mut next_fetch = lock(&self.last_fetch)
            .get(&url)
            .and_then(|last| last.checked_add(floor).ok())
            .map_or_else(Timestamp::now, |earliest| earliest.max(Timestamp::now()));
        let mut generation = generation;
        let mut changes = Changes::new(&self.recipes, reporter, cancel);

        while !cancel.is_cancelled() && !reporter.stream_meta().has_inband_title() {
            if self.recipes.generation() != generation {
                // Keeps going when the recipes changed but not the one in use.
                let set = self.recipes.current();
                let same = match (&source, set.find(session.station.id.as_deref(), stream)) {
                    (Source::Recipe(entry), Some(now)) => {
                        entry.id == now.id && entry.recipe == now.recipe
                    }
                    (Source::Server(_), None) => true,
                    _ => false,
                };
                if !same {
                    return;
                }
                generation = self.recipes.generation();
            }
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
                lock(&self.last_fetch).insert(url.clone(), now);
                let snapshot = match fetch(client, &url).await {
                    Ok((page, date)) => read(&source, stream, &page).map(|s| (s, date)),
                    Err(e) => Err(format!("could not read {url}: {e}")),
                };
                match snapshot {
                    Ok((snapshot, date)) => {
                        let found = snapshot.current.is_some();
                        let error = (!found).then(|| nothing_found(&snapshot));
                        let plan = tracker.on_snapshot(snapshot, Timestamp::now(), date);
                        if let Source::Recipe(entry) = &source {
                            match plan.publish.last() {
                                Some(scheduled) => self.notify_saved(entry, Ok(&scheduled.on_air)),
                                None => self.notify_saved(
                                    entry,
                                    Err(error.as_deref().unwrap_or("Nothing on air")),
                                ),
                            }
                        }
                        self.update_status(session, |s| s.error = error);
                        pending = plan.publish.into();
                        next_fetch = plan.next_fetch;
                        if let Some(offset) = tracker.offset() {
                            lock(&self.offsets).insert(key.clone(), offset);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(%url, "web title unavailable: {e}");
                        if let Source::Recipe(entry) = &source {
                            self.notify_saved(entry, Err(&e));
                        }
                        self.update_status(session, |s| s.error = Some(e));
                        next_fetch = tracker.on_failure(now);
                    }
                }
                continue; // what is due right away is published at once
            }

            let wake = pending.front().map_or(next_fetch, |s| s.at.min(next_fetch));
            changes.next(Some(until(wake, now))).await;
        }
        if reporter.stream_meta().has_inband_title() && !cancel.is_cancelled() {
            self.update_status(session, |s| s.state = SourceState::Stream);
        }
    }

    /// For streams one of whose adapters [enriches](Adapter::enriches) titles (AzuraCast): the
    /// details of each ICY title, from the server's API, until the playback ends. One request
    /// per title (one more if the API does not know it yet), never closer than the minimum
    /// interval. Stops for good if the API does not describe the stream.
    async fn icy_details(
        &self,
        session: &Session,
        client: &Client,
        reporter: &Reporter,
        cancel: &Cancel,
    ) {
        let stream = session.stream.as_str();
        let Some(adapter) = adapters::candidates(stream, None)
            .into_iter()
            .find(|a| a.enriches())
        else {
            return;
        };
        if lock(&self.no_details).contains(stream) {
            return;
        }
        let Some(Probe { url, .. }) = adapter.probe(stream) else {
            return;
        };
        let floor = Policy::default().floor;
        // The ICY title being looked up, when to try, and the lookups left.
        let mut target: Option<(String, Timestamp, u8)> = None;
        let mut confirmed = false;
        let mut changes = Changes::new(&self.recipes, reporter, cancel);
        while !cancel.is_cancelled() {
            let now = Timestamp::now();
            let heard = reporter.stream_meta().icy_title();
            if heard.is_some() && heard.as_deref() != target.as_ref().map(|(t, ..)| t.as_str()) {
                target = heard.map(|h| (h, now, 2));
            }
            let earliest = lock(&self.last_fetch)
                .get(&url)
                .and_then(|last| last.checked_add(floor).ok());
            let due = target
                .as_ref()
                .is_some_and(|(_, at, left)| *left > 0 && *at <= now)
                && earliest.is_none_or(|e| e <= now);
            if let (true, Some((heard, at, left))) = (due, target.as_mut()) {
                lock(&self.last_fetch).insert(url.clone(), now);
                *left -= 1;
                let page = match fetch(client, &url).await {
                    Ok((page, _)) => page,
                    Err(e) => {
                        tracing::debug!(%url, "track details unavailable: {e}");
                        *left += 1; // the network, not the title
                        *at = now.checked_add(DETAILS_BACKOFF).unwrap_or(now);
                        continue;
                    }
                };
                match adapter.details(stream, &page, heard) {
                    Ok(Some(snapshot)) => {
                        if let Some(track) = snapshot.current.as_ref().and_then(track_info) {
                            let next = snapshot.next.first().and_then(track_info);
                            reporter.icy_details(heard, track, next, snapshot.show);
                        }
                        *left = 0;
                        if !confirmed {
                            confirmed = true;
                            tracing::info!(
                                adapter = adapter.name(),
                                "track details from the server's API"
                            );
                            self.update_status(session, |s| {
                                s.adapter = Some(adapter.name().to_owned())
                            });
                        }
                    }
                    // Not known yet, or not a track (jingle, live show the API does not see).
                    Ok(None) => *at = now.checked_add(DETAILS_RETRY).unwrap_or(now),
                    Err(e) if !confirmed => {
                        tracing::debug!(%url, "no track details for this stream: {e}");
                        lock(&self.no_details).insert(stream.to_owned());
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(%url, "track details unavailable: {e}");
                        *at = now.checked_add(DETAILS_BACKOFF).unwrap_or(now);
                    }
                }
                continue;
            }
            // Next: a new ICY title, or the lookup to try again once due.
            let retry = target
                .as_ref()
                .filter(|(_, _, left)| *left > 0)
                .map(|(_, at, _)| {
                    let wake = earliest.map_or(*at, |e| e.max(*at));
                    until(wake, now)
                });
            changes.next(retry).await;
        }
    }

    /// The streaming server's status adapter, once the stream turned out not to announce titles.
    async fn find_server(
        &self,
        client: &Client,
        stream: &Url,
        reporter: &Reporter,
        cancel: &Cancel,
    ) -> Option<Adapter> {
        let meta = reporter.stream_meta();
        let mut changes = Changes::new(&self.recipes, reporter, cancel);
        let opened = loop {
            if cancel.is_cancelled() {
                return None;
            }
            if let Some(opened) = meta.opened() {
                break opened;
            }
            // `opened` is set by the decoder; give up if it never comes (failed connection).
            let left = OPEN_TIMEOUT.checked_sub(reporter_age(reporter))?;
            changes.next(Some(left)).await;
        };
        if opened.inband {
            // The stream can carry titles: give it time to announce one.
            while let Some(left) = INBAND_GRACE.checked_sub(opened.at.elapsed()) {
                if cancel.is_cancelled() || meta.has_inband_title() {
                    return None;
                }
                changes.next(Some(left)).await;
            }
            if meta.has_inband_title() {
                return None;
            }
        }

        let origin = origin_key(stream);
        if let Some(known) = lock(&self.servers).get(&origin) {
            return *known;
        }
        let mut found = None;
        for adapter in adapters::candidates(stream.as_str(), opened.server.as_deref()) {
            if cancel.is_cancelled() {
                return None;
            }
            let Some(probe) = adapter.probe(stream.as_str()) else {
                continue;
            };
            // A matching URL is not proof: the document must have the adapter's shape.
            if let Ok((page, _)) = fetch(client, &probe.url).await
                && adapter.read(stream.as_str(), &page).is_ok()
            {
                found = Some(adapter);
                break;
            }
        }
        lock(&self.servers).insert(origin, found);
        found
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A track as a source gave it, for the player (AzuraCast gives absolute starts only).
fn track_info(raw: &RawTrack) -> Option<TrackInfo> {
    Some(TrackInfo {
        artist: raw.artist.clone(),
        title: raw.title.clone()?,
        started_at: match raw.start {
            Some(Start::At(at)) => Some(at.to_string()),
            _ => None,
        },
        duration_secs: raw.duration.and_then(|d| u32::try_from(d.as_secs()).ok()),
        cover: raw.cover.clone(),
    })
}

/// Why a page that was read gave no track, in words a recipe author can act on.
fn nothing_found(snapshot: &Snapshot) -> String {
    if snapshot.next.is_empty() {
        "The page was read, but nothing matched: check `item` and `title` (or `text`)".into()
    } else {
        "Upcoming tracks were found, but not the one on air: check `item`".into()
    }
}

fn read(source: &Source, stream: &str, page: &str) -> Result<Snapshot, String> {
    match source {
        Source::Recipe(entry) => extract::read(&entry.recipe, page),
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
    let charset = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|t| t.to_str().ok())
        .and_then(charset_param)
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()));
    let date = response
        .headers()
        .get(header::DATE)
        .and_then(|d| d.to_str().ok())
        .and_then(|d| {
            jiff::fmt::rfc2822::DateTimeParser::new()
                .parse_timestamp(d)
                .ok()
        });
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_PAGE_BYTES {
            return Err("page too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((decode(&body, charset), date))
}

/// The page as text, in the charset it is written in, chosen as a browser would: a byte order
/// mark, then the `Content-Type` header, then a `<meta>` or `<?xml?>` declaration near the
/// start; without any, UTF-8 when the bytes are valid UTF-8, windows-1252 (Latin-1's superset)
/// otherwise.
fn decode(body: &[u8], header: Option<&'static encoding_rs::Encoding>) -> String {
    use encoding_rs::{UTF_8, WINDOWS_1252};
    let encoding = header
        .or_else(|| declared_charset(body))
        .unwrap_or_else(|| {
            if std::str::from_utf8(body).is_ok() {
                UTF_8
            } else {
                WINDOWS_1252
            }
        });
    // `decode` lets a byte order mark override the choice, and replaces invalid sequences.
    encoding.decode(body).0.into_owned()
}

/// The `charset` parameter of a `Content-Type` value.
fn charset_param(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']))
    })
}

/// The charset an HTML page (`<meta charset>`, `<meta http-equiv content="…; charset=…">`) or
/// an XML document (`<?xml … encoding="…"?>`) declares in its first kilobyte.
fn declared_charset(body: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let head = body[..body.len().min(1024)].to_ascii_lowercase();
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    // The label after `key=` in `tag`, quoted or not.
    let label = |tag: &[u8], key: &[u8]| {
        let rest = &tag[find(tag, key)? + key.len()..];
        let rest = rest
            .strip_prefix(b"\"")
            .or(rest.strip_prefix(b"'"))
            .unwrap_or(rest);
        let end = rest
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || b"-_:.".contains(b)))
            .unwrap_or(rest.len());
        encoding_rs::Encoding::for_label(&rest[..end])
    };
    if head.starts_with(b"<?xml") {
        return label(&head[..find(&head, b"?>")?], b"encoding=");
    }
    let mut rest = &head[..];
    while let Some(start) = find(rest, b"<meta") {
        rest = &rest[start..];
        let end = rest.iter().position(|&b| b == b'>').unwrap_or(rest.len());
        if let Some(encoding) = label(&rest[..end], b"charset=") {
            // A page cannot declare UTF-16 from inside itself: what it says is read as UTF-8.
            let utf16 = encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE;
            return Some(if utf16 { encoding_rs::UTF_8 } else { encoding });
        }
        rest = &rest[end..];
    }
    None
}

fn origin_key(url: &Url) -> String {
    format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(0)
    )
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

/// From `now` to `wake` (nothing if already past), at most [`CLOCK_CHECK`].
fn until(wake: Timestamp, now: Timestamp) -> Duration {
    wake.duration_since(now)
        .max(SignedDuration::ZERO)
        .unsigned_abs()
        .min(CLOCK_CHECK)
}

/// What a title task waits for besides time: the playback ending (stop, other station), the
/// stream opening or announcing a title, the recipes changing. Create it before reading what it
/// watches: a change made in between then ends the next wait at once instead of being missed.
struct Changes<'a> {
    cancel: &'a Cancel,
    stream: watch::Receiver<()>,
    recipes: watch::Receiver<u64>,
}

impl<'a> Changes<'a> {
    fn new(recipes: &Recipes, reporter: &Reporter, cancel: &'a Cancel) -> Self {
        Self {
            cancel,
            stream: reporter.stream_meta().changes(),
            recipes: recipes.changes(),
        }
    }

    /// Returns on the next change, or once `timeout` has passed (`None`: no limit). The caller
    /// checks again what it waits for: a change may concern something else.
    async fn next(&mut self, timeout: Option<Duration>) {
        let timeout = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        // Both senders outlive the task (the reporter and the recipes it holds): `changed` does
        // not fail.
        tokio::select! {
            () = self.cancel.cancelled() => {}
            _ = self.stream.changed() => {}
            _ = self.recipes.changed() => {}
            () = timeout => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_read_in_their_charset() {
        let latin1 = b"<p>\xc0 l'\xe9t\xe9</p>";
        assert_eq!(decode(latin1, None), "<p>À l'été</p>");
        let utf8 = "<p>Café</p>".as_bytes();
        assert_eq!(decode(utf8, None), "<p>Café</p>");

        let header = charset_param("text/html; Charset=\"ISO-8859-1\"")
            .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()));
        assert_eq!(header, Some(encoding_rs::WINDOWS_1252));
        assert_eq!(charset_param("text/html"), None);

        let meta = b"<html><head><meta charset=\"iso-8859-15\"><title>\xa4</title>";
        assert_eq!(
            decode(meta, None),
            "<html><head><meta charset=\"iso-8859-15\"><title>€</title>"
        );
        let equiv = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">\x93x\x94";
        assert!(decode(equiv, None).ends_with("“x”"));
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>\xe9</t>";
        assert!(decode(xml, None).ends_with("<t>é</t>"));
        // The header wins over the page; a byte order mark over both.
        assert!(decode(meta, Some(encoding_rs::UTF_8)).contains('\u{fffd}'));
        assert_eq!(
            decode(
                b"\xef\xbb\xbf\xc3\xa9t\xc3\xa9",
                Some(encoding_rs::WINDOWS_1252)
            ),
            "été"
        );
        // UTF-16 declared inside the page is read as UTF-8.
        assert_eq!(
            decode("<meta charset=utf-16>é".as_bytes(), None),
            "<meta charset=utf-16>é"
        );
    }

    #[test]
    fn tracks_keep_only_what_the_player_can_use() {
        let at: Timestamp = "2026-11-08T12:00:00Z".parse().unwrap();
        let raw = RawTrack {
            artist: Some("Air".into()),
            title: Some("Sexy Boy".into()),
            start: Some(Start::At(at)),
            duration: Some(SignedDuration::from_secs(298)),
            cover: None,
        };
        let info = track_info(&raw).unwrap();
        assert_eq!(info.started_at.as_deref(), Some("2026-11-08T12:00:00Z"));
        assert_eq!(info.duration_secs, Some(298));
        // A wall-clock start has no date: the player is not given it.
        let local = RawTrack {
            start: Some(Start::Local("12:00".parse().unwrap())),
            ..raw.clone()
        };
        assert_eq!(track_info(&local).unwrap().started_at, None);
        // A negative duration is no duration; no title, no track.
        let odd = RawTrack {
            duration: Some(SignedDuration::from_secs(-5)),
            ..raw.clone()
        };
        assert_eq!(track_info(&odd).unwrap().duration_secs, None);
        let untitled = RawTrack { title: None, ..raw };
        assert!(track_info(&untitled).is_none());
    }

    #[test]
    fn an_empty_read_says_what_to_check() {
        assert!(nothing_found(&Snapshot::default()).contains("`item` and `title`"));
        let only_next = Snapshot {
            next: vec![RawTrack::default()],
            ..Snapshot::default()
        };
        assert!(nothing_found(&only_next).contains("not the one on air"));
    }

    #[test]
    fn waits_are_capped_and_never_negative() {
        let now: Timestamp = "2026-11-08T12:00:00Z".parse().unwrap();
        assert_eq!(
            until(now - SignedDuration::from_secs(10), now),
            Duration::ZERO
        );
        assert_eq!(
            until(now + SignedDuration::from_secs(2), now),
            Duration::from_secs(2)
        );
        assert_eq!(until(now + SignedDuration::from_hours(1), now), CLOCK_CHECK);
    }

    /// Serves one raw HTTP response on a local port; its address.
    async fn serve(response: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(&response).await;
        });
        format!("http://{address}/")
    }

    #[tokio::test]
    async fn pages_come_with_the_site_clock_and_their_charset() {
        let page = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\n\
            Date: Sun, 08 Nov 2026 12:00:00 GMT\r\nContent-Length: 4\r\n\r\n\xe9t\xe9!";
        let url = serve(page.to_vec()).await;
        let (text, date) = fetch(&Client::new(), &url).await.unwrap();
        assert_eq!(text, "été!");
        assert_eq!(date, Some("2026-11-08T12:00:00Z".parse().unwrap()));
    }

    #[tokio::test]
    async fn errors_and_pages_too_large_are_refused() {
        let missing = serve(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        assert_eq!(
            fetch(&Client::new(), &missing).await.unwrap_err(),
            "HTTP 404 Not Found"
        );
        let mut huge = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_PAGE_BYTES + 1
        )
        .into_bytes();
        huge.resize(huge.len() + MAX_PAGE_BYTES + 1, b'x');
        let url = serve(huge).await;
        assert_eq!(
            fetch(&Client::new(), &url).await.unwrap_err(),
            "page too large"
        );
    }

    #[test]
    fn origins_ignore_paths() {
        let a: Url = "https://radio.example.org/live.mp3".parse().unwrap();
        let b: Url = "https://radio.example.org:443/hd.aac".parse().unwrap();
        assert_eq!(origin_key(&a), origin_key(&b));
    }

    fn web(dir: Option<std::path::PathBuf>) -> (Arc<WebTitles>, Arc<Mutex<Vec<RecipeNotice>>>) {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = notices.clone();
        let web = WebTitles::new(
            Arc::new(Recipes::new(dir)),
            true,
            Box::new(move |event| {
                if let WebEvent::Notice(n) = event {
                    sink.lock().unwrap().push(n);
                }
            }),
        );
        (Arc::new(web), notices)
    }

    #[test]
    fn saving_a_recipe_tells_whether_it_applies() {
        let dir =
            std::env::temp_dir().join(format!("soundfolk-web-notices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (web, notices) = web(Some(dir.clone()));
        *lock(&web.current) = Some(Session {
            id: 1,
            station: StationRef::default(),
            stream: "https://live.aurafm.org/Frequence7".parse().unwrap(),
        });

        std::fs::write(dir.join("broken.toml"), "schema = 1\nname = ").unwrap();
        let other = crate::recipes::template(&crate::recipes::Template {
            name: "Other",
            station_id: None,
            stream: "https://example.org/live",
            homepage: Some("https://example.org/"),
        })
        .replace("item = \"\"", "item = \"li\"")
        .replace("artist = \"\"", "artist = \".a\"")
        .replace("title = \"\"", "title = \".t\"");
        std::fs::write(dir.join("other.toml"), other).unwrap();
        std::fs::write(
            dir.join("frequence7.toml"),
            crate::recipes::local_copy(web.recipes.bundled_text("frequence7").unwrap()),
        )
        .unwrap();
        let changes = web.recipes.reload_if_changed().unwrap();
        web.recipes_changed(&changes);

        let notices = notices.lock().unwrap();
        let by_file = |f: &str| notices.iter().find(|n| n.file == f).cloned();
        assert!(!by_file("broken.toml").unwrap().ok);
        assert!(by_file("other.toml")
            .unwrap()
            .message
            .contains("does not apply"));
        // The station's own recipe: reported after the next reading of its page.
        assert_eq!(by_file("frequence7.toml"), None);
        assert_eq!(*lock(&web.saved), vec!["frequence7".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_local_files_naming_the_stream_are_shown_for_the_station() {
        let dir =
            std::env::temp_dir().join(format!("soundfolk-web-local-error-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("my-radio.toml"),
            "schema = 1\nmatch = { streams = [\"example.org/live\"] }\n",
        )
        .unwrap();
        let (web, _) = web(Some(dir.clone()));
        let session = Session {
            id: 1,
            station: StationRef::default(),
            stream: "http://example.org/live".parse().unwrap(),
        };
        let error = web.local_error(&session).unwrap();
        assert!(error.path.ends_with("my-radio.toml"));

        // Told from the text kept at load time: no file is read again (the async title tasks
        // ask this on every status update).
        std::fs::remove_dir_all(&dir).unwrap();
        let error = web.local_error(&session).unwrap();
        assert!(error.path.ends_with("my-radio.toml"));
    }
}
