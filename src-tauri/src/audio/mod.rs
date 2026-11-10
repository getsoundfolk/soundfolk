// SPDX-License-Identifier: MPL-2.0
//! Desktop audio engine (Linux / Windows / macOS), 100% Rust:
//!
//! ```text
//! HTTP ──▶ stream-download ──▶ symphonia ──▶ conversion ──▶ ring buffer ──▶ cpal ──▶ sound card
//!          (bounded buffer)    (decoding)    (channels+rate)   (SPSC rtrb)     (callback)
//!          └──────────── "audio-decoder" thread ───────────┘               └ cpal thread
//! ```
//!
//! This module does not depend on Tauri: the player state is published through a [`StateListener`]
//! that the application layer wires to the event bus.
//!
//! `cpal::Stream` is not `Send` on every platform: a dedicated thread (`audio-engine`) owns it,
//! and [`DesktopAudioEngine`] is only its remote control (`Send + Sync`).
//!
//! Resilience: a dropped stream is reopened by the decoder (see [`network`] for sleep and network
//! changes); a dead sound output (device gone, sound server restarted) is reopened by this thread.

mod connectivity;
mod decoder;
mod failure;
mod icy;
mod loudness;
mod network;
mod output;
mod playlist;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::now_playing::WebTitles;
use crate::tracks;
pub use reqwest::Url;

pub use decoder::probe as probe_stream;
pub use loudness::{LevelMemory, Recalled, StoredLevel};
pub use playlist::resolve_stream_url;

/// Common facade for all platforms (see `docs/04-audio-engine.md`).
pub trait AudioEngine {
    /// Hands the station to the engine and returns at once; [`PlayReply::wait`] gives the outcome
    /// (opening the sound output can take a few hundred ms). Requests play in the order made.
    /// `station` helps find the title of streams that do not announce it.
    fn play(&mut self, url: &str, station: StationRef) -> Result<PlayReply, String>;
    /// Pause of a live stream: the sound is muted but the stream keeps being consumed,
    /// so `resume` picks up **live**, not where playback was paused.
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    /// `volume` is the slider position, clamped to `0.0..=1.0`; the gain follows a
    /// perceptual curve (see [`gain`]), so the middle of the slider sounds half as loud.
    fn set_volume(&mut self, volume: f32) -> Result<(), String>;
}

/// Gain applied to the samples for a slider position in `0.0..=1.0`.
///
/// Loudness is perceived on a log scale: a linear `sample * position` crowds the whole range
/// into the first third of the slider. A cube (as PulseAudio / PipeWire do) is close to a
/// 60 dB dB-scale fader, and still reaches true silence at 0.
pub(crate) fn gain(position: f32) -> f32 {
    position.clamp(0.0, 1.0).powi(3)
}

/// State published to the frontend (`{ "status": "error", "message": "…" }`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "status", content = "message", rename_all = "snake_case")]
pub enum PlayerState {
    Idle,
    Connecting,
    /// The stream dropped after playing: reopening it, or waiting for the network.
    Reconnecting(String),
    Playing,
    Paused,
    Error(String),
}

/// What is on air: station name (`icy-name`) and current title.
#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct NowPlaying {
    pub station: Option<String>,
    /// The title as shown: `Artist - Title` (ICY `StreamTitle`, or rebuilt from `track`).
    pub title: Option<String>,
    /// Artist and title kept apart, when the source gives them that way.
    pub track: Option<TrackInfo>,
    /// The track announced next, when the source says.
    pub next: Option<TrackInfo>,
    /// The show on air, when the source says.
    pub show: Option<String>,
    pub source: Option<TitleSource>,
}

/// Where the title comes from (see `docs/08-now-playing.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    /// ICY `StreamTitle` in the stream.
    Icy,
    /// Tags carried by the stream itself (Ogg / Opus / FLAC Vorbis comments).
    StreamTags,
    /// The station's web page, read with a recipe.
    Recipe,
    /// The streaming server's status page (Icecast, Shoutcast, AzuraCast).
    Server,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct TrackInfo {
    pub artist: Option<String>,
    pub title: String,
    /// RFC 3339.
    pub started_at: Option<String>,
    pub duration_secs: Option<u32>,
    pub cover: Option<String>,
}

impl TrackInfo {
    fn display(&self) -> String {
        soundfolk_nowplaying::artist_title(self.artist.as_deref(), &self.title)
    }
}

impl NowPlaying {
    /// Artist and title of the track on air: as the source gives them apart, otherwise parsed
    /// from `raw`, the announced title.
    pub fn parsed(&self, raw: &str, station: Option<&str>) -> tracks::ParsedTitle {
        match &self.track {
            Some(track) => tracks::ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            },
            None => tracks::parse(raw, station),
        }
    }
}

impl From<&soundfolk_nowplaying::Track> for TrackInfo {
    fn from(track: &soundfolk_nowplaying::Track) -> Self {
        Self {
            artist: track.artist.clone(),
            title: track.title.clone(),
            started_at: track.started_at.map(|t| t.to_string()),
            duration_secs: track.duration.and_then(|d| u32::try_from(d.as_secs()).ok()),
            cover: track.cover.clone(),
        }
    }
}

/// The station being played, to find a web source for its title.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StationRef {
    /// Radio-Browser UUID, when the station comes from the catalog.
    pub id: Option<String>,
    /// Its stream may only reach public addresses: a catalog station, or one received in a link
    /// (`public_net`, docs/16-sharing.md §6.2).
    pub public_only: bool,
    /// Its remembered level, for *Constant volume*, read before the playback was asked for.
    pub level: Option<Box<loudness::Recalled>>,
}

/// What a station's level is remembered by: its Radio-Browser UUID (the level survives a change
/// of address), else its address.
pub fn level_key(catalog_id: Option<&str>, url: &Url) -> String {
    catalog_id.map_or_else(|| url.to_string(), str::to_owned)
}

pub type StateListener = Arc<dyn Fn(PlayerState) + Send + Sync>;
pub type NowPlayingListener = Arc<dyn Fn(NowPlaying) + Send + Sync>;

/// Parameters read by the real-time callback (atomics: no locks on the audio side).
pub(crate) struct Controls {
    /// Gain applied to the samples (already through [`gain`]), not the slider position.
    volume: AtomicU32,
    paused: AtomicBool,
    /// *Constant volume* (see [`loudness`]), read by the decoder thread.
    normalize: AtomicBool,
    /// Station gain applied now, dB (`f32` bits), NaN when nothing plays.
    station_gain: AtomicU32,
}

impl Controls {
    fn new() -> Self {
        Self {
            volume: AtomicU32::new(1.0f32.to_bits()),
            paused: AtomicBool::new(false),
            normalize: AtomicBool::new(true),
            station_gain: AtomicU32::new(f32::NAN.to_bits()),
        }
    }

    pub(crate) fn normalize(&self) -> bool {
        self.normalize.load(Ordering::Relaxed)
    }

    pub(crate) fn set_station_gain(&self, db: f32) {
        self.station_gain.store(db.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Current state + listener. Notifications are emitted while holding the lock so that
/// events arrive in the order the state changed.
struct Status {
    state: Mutex<PlayerState>,
    listener: StateListener,
    now_playing: Mutex<NowPlaying>,
    now_playing_listener: NowPlayingListener,
}

impl Status {
    fn lock(&self) -> MutexGuard<'_, PlayerState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now_playing(&self) -> MutexGuard<'_, NowPlaying> {
        self.now_playing.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Clears the title (new playback or stop).
    fn clear_now_playing(&self) {
        let mut current = self.now_playing();
        if *current != NowPlaying::default() {
            *current = NowPlaying::default();
            (self.now_playing_listener)(NowPlaying::default());
        }
    }

    fn set(&self, new: PlayerState) {
        self.replace(&mut self.lock(), new);
    }

    /// [`Status::set`] through a guard already held: whoever must read the state, or the pause
    /// flag, and write the state in one step takes the lock once for both.
    fn replace(&self, state: &mut PlayerState, new: PlayerState) {
        if *state != new {
            *state = new.clone();
            (self.listener)(new);
        }
    }
}

/// Cancellation flag for a session, with hooks to interrupt a blocked network read
/// (without them, `Stop` would wait for the HTTP read timeout to expire).
#[derive(Default)]
pub(crate) struct Cancel {
    flag: AtomicBool,
    /// Each with its id, so that a [`ScopedHook`] can take its own back.
    hooks: Mutex<Vec<(u64, CancelHook)>>,
    next_hook: AtomicU64,
    /// The flag again, for the tasks that wait for it ([`Cancel::cancelled`]).
    done: tokio::sync::watch::Sender<bool>,
}

impl Cancel {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Returns once the session is cancelled (at once if it already is).
    pub(crate) async fn cancelled(&self) {
        // `self` holds the sender, so the channel cannot close while this waits.
        let _ = self.done.subscribe().wait_for(|&done| done).await;
    }

    fn cancel(&self) {
        // The flag is set before taking the lock, so a hook registered after the drain
        // sees it and runs itself: every hook runs exactly once.
        self.flag.store(true, Ordering::SeqCst);
        self.done.send_replace(true);
        let hooks = std::mem::take(&mut *self.hooks.lock().unwrap_or_else(|p| p.into_inner()));
        for (_, hook) in hooks {
            hook();
        }
    }

    /// Adds an action to run on cancellation (immediately if already cancelled), for one attempt
    /// within the session (a connection, a diagnosis): the hook is dropped with the returned
    /// guard, so a session that reconnects for hours does not keep one per attempt. Other hooks
    /// are kept: each one runs.
    pub(crate) fn on_cancel_scoped(
        self: &Arc<Self>,
        hook: impl FnOnce() + Send + 'static,
    ) -> ScopedHook {
        ScopedHook {
            id: self.add(Box::new(hook)),
            cancel: self.clone(),
        }
    }

    /// Keeps the hook, or runs it at once if already cancelled (`None`).
    fn add(&self, hook: CancelHook) -> Option<u64> {
        let mut hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner());
        if self.is_cancelled() {
            drop(hooks);
            hook();
            return None;
        }
        let id = self.next_hook.fetch_add(1, Ordering::Relaxed);
        hooks.push((id, hook));
        Some(id)
    }
}

type CancelHook = Box<dyn FnOnce() + Send>;

/// A hook of [`Cancel::on_cancel_scoped`], removed when this is dropped (if it has not run).
pub(crate) struct ScopedHook {
    cancel: Arc<Cancel>,
    id: Option<u64>,
}

impl Drop for ScopedHook {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.cancel
                .hooks
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retain(|(hook, _)| *hook != id);
        }
    }
}

/// Publishes state on behalf of a session. A cancelled session can no longer publish anything,
/// so an old decoder thread cannot overwrite the state of the next session.
#[derive(Clone)]
pub(crate) struct Reporter {
    status: Arc<Status>,
    controls: Arc<Controls>,
    cancel: Arc<Cancel>,
    stream: Arc<StreamMeta>,
}

/// What the decoder learned about the stream, for the web title task.
pub(crate) struct StreamMeta {
    pub(crate) created: Instant,
    /// The stream announced a title itself (ICY or tags): web sources stand down.
    inband_title: AtomicBool,
    /// The last ICY `StreamTitle`, for the details a server API gives about it.
    icy_title: Mutex<Option<String>>,
    /// Set once the stream is open.
    opened: Mutex<Option<OpenedInfo>>,
    /// Bumped each time one of the above changes, for the web title task that waits for it.
    changes: tokio::sync::watch::Sender<()>,
}

impl Default for StreamMeta {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            inband_title: AtomicBool::new(false),
            icy_title: Mutex::new(None),
            opened: Mutex::new(None),
            changes: tokio::sync::watch::Sender::new(()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenedInfo {
    pub at: Instant,
    /// The stream carries metadata (ICY `icy-metaint`, or an Ogg container).
    pub inband: bool,
    /// HTTP `Server` header.
    pub server: Option<String>,
}

impl StreamMeta {
    pub(crate) fn has_inband_title(&self) -> bool {
        self.inband_title.load(Ordering::SeqCst)
    }

    pub(crate) fn icy_title(&self) -> Option<String> {
        self.icy_title
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(crate) fn opened(&self) -> Option<OpenedInfo> {
        self.opened
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Sees each change made after this call: subscribe before reading what it guards, so a
    /// change between the reading and the waiting is not lost.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_replace(());
    }
}

impl Reporter {
    fn publish(&self, new: PlayerState) {
        self.publish_with(|| new);
    }

    /// `new` runs under the state lock, so it reads the pause flag as `pause` and `resume`
    /// leave it.
    fn publish_with(&self, new: impl FnOnce() -> PlayerState) {
        let mut state = self.status.lock();
        if self.cancel.is_cancelled() {
            return;
        }
        self.status.replace(&mut state, new());
    }

    pub(crate) fn reconnecting(&self, message: &str) {
        self.publish(PlayerState::Reconnecting(message.to_owned()));
    }

    pub(crate) fn audible(&self) {
        self.publish_with(|| {
            if self.controls.is_paused() {
                PlayerState::Paused
            } else {
                PlayerState::Playing
            }
        });
    }

    pub(crate) fn failed(&self, message: String) {
        self.publish(PlayerState::Error(message));
    }

    pub(crate) fn station(&self, name: Option<String>) {
        self.update_now_playing(|np| np.station = name);
    }

    /// ICY `StreamTitle`.
    pub(crate) fn title(&self, title: String) {
        self.stream.inband_title.store(true, Ordering::SeqCst);
        *self
            .stream
            .icy_title
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(title.clone());
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(title),
                source: Some(TitleSource::Icy),
                ..Default::default()
            };
        });
        self.stream.changed();
    }

    /// Artist and title from the stream's own tags (Vorbis comments).
    pub(crate) fn stream_tags(&self, artist: Option<String>, title: String) {
        self.stream.inband_title.store(true, Ordering::SeqCst);
        let track = TrackInfo {
            artist,
            title,
            started_at: None,
            duration_secs: None,
            cover: None,
        };
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(track.display()),
                track: Some(track),
                source: Some(TitleSource::StreamTags),
                ..Default::default()
            };
        });
        self.stream.changed();
    }

    /// What a web source says is on air. Ignored once the stream announced a title itself.
    pub(crate) fn web_title(
        &self,
        on_air: &soundfolk_nowplaying::tracker::OnAir,
        source: TitleSource,
    ) {
        if self.stream.has_inband_title() {
            return;
        }
        let track = TrackInfo::from(&on_air.current);
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(track.display()),
                track: Some(track),
                next: on_air.next.as_ref().map(TrackInfo::from),
                show: on_air.show.clone(),
                source: Some(source),
            };
        });
    }

    /// Details a server API gives about the ICY title `heard` (artist and title apart, start,
    /// length, cover, next track, show). The title shown stays the stream's; ignored once the
    /// stream moved on to another title.
    pub(crate) fn icy_details(
        &self,
        heard: &str,
        track: TrackInfo,
        next: Option<TrackInfo>,
        show: Option<String>,
    ) {
        self.update_now_playing(|np| {
            if np.source == Some(TitleSource::Icy) && np.title.as_deref() == Some(heard) {
                np.track = Some(track);
                np.next = next;
                np.show = show;
            }
        });
    }

    /// The stream is open: whether it carries metadata, and which server sends it.
    pub(crate) fn stream_opened(&self, inband: bool, server: Option<String>) {
        *self.stream.opened.lock().unwrap_or_else(|p| p.into_inner()) = Some(OpenedInfo {
            at: Instant::now(),
            inband,
            server,
        });
        self.stream.changed();
    }

    pub(crate) fn stream_meta(&self) -> &StreamMeta {
        &self.stream
    }

    fn update_now_playing(&self, change: impl FnOnce(&mut NowPlaying)) {
        let mut current = self.status.now_playing();
        if self.cancel.is_cancelled() {
            return;
        }
        let mut next = current.clone();
        change(&mut next);
        if next != *current {
            *current = next.clone();
            (self.status.now_playing_listener)(next);
        }
    }
}

enum Command {
    Play {
        url: Url,
        station: StationRef,
        reply: Sender<Result<(), String>>,
    },
    Stop,
}

/// A playback in progress. `Drop` silences the sound (the cpal `Stream` is destroyed) and cancels the network.
struct Session {
    cancel: Arc<Cancel>,
    output: output::Output,
}

/// What plays, to start it again on a new sound output.
struct Playback {
    url: Url,
    station: StationRef,
    /// The output was lost and could not be reopened yet: since when.
    output_lost: Option<Instant>,
}

/// How often the sound output is checked.
const OUTPUT_CHECK: Duration = Duration::from_secs(1);
/// How long to try reopening a lost sound output (a device can take a moment to come back after
/// a wake from sleep) before reporting the error.
const OUTPUT_RETRY: Duration = Duration::from_secs(15);

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct DesktopAudioEngine {
    commands: Sender<Command>,
    controls: Arc<Controls>,
    status: Arc<Status>,
    levels: Levels,
}

/// Where station levels are remembered, set once by the application.
type Levels = Arc<OnceLock<Arc<dyn LevelMemory>>>;

impl DesktopAudioEngine {
    /// `web`: where to look for the title of streams that do not announce it.
    pub fn new(
        listener: StateListener,
        now_playing_listener: NowPlayingListener,
        web: Arc<WebTitles>,
    ) -> Self {
        let controls = Arc::new(Controls::new());
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener,
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener,
        });
        let (commands, rx) = mpsc::channel();
        let levels = Levels::default();

        let (c, s, l) = (controls.clone(), status.clone(), levels.clone());
        thread::Builder::new()
            .name("audio-engine".into())
            .spawn(move || {
                let Some(runtime) = network_runtime() else {
                    return;
                };
                EngineCtx {
                    controls: c,
                    status: s,
                    web,
                    levels: l,
                    runtime: runtime.handle().clone(),
                }
                .run(rx);
            })
            .expect("failed to start the audio-engine thread");

        Self {
            commands,
            controls,
            status,
            levels,
        }
    }

    /// Where station levels are remembered, for *Constant volume*. Without it, every station
    /// starts as unknown.
    /// Its writes run on their own thread ([`loudness::WriteBehind`]): the decoder never waits
    /// for them.
    pub fn set_level_memory(&self, memory: Arc<dyn LevelMemory>) {
        if self
            .levels
            .set(loudness::WriteBehind::wrap(memory))
            .is_err()
        {
            tracing::warn!("station level memory already set");
        }
    }

    /// *Constant volume*: one gain per station so all play near the same loudness. Switching it
    /// takes effect on the sound within a second.
    pub fn set_normalize(&self, enabled: bool) {
        self.controls.normalize.store(enabled, Ordering::Relaxed);
    }

    /// Gain *Constant volume* applies to the station now, dB; `None` when it is off or nothing
    /// plays.
    pub fn level_adjustment(&self) -> Option<f32> {
        let db = f32::from_bits(self.controls.station_gain.load(Ordering::Relaxed));
        (self.controls.normalize() && db.is_finite()).then_some(db)
    }

    pub fn state(&self) -> PlayerState {
        self.status.lock().clone()
    }

    pub fn now_playing(&self) -> NowPlaying {
        self.status.now_playing().clone()
    }

    /// Shows `message` as the playback error, even when that error already is the state: the
    /// listener may have held it back (see `crate::favorite_repair`).
    pub(crate) fn show_error(&self, message: String) {
        let mut state = self.status.lock();
        *state = PlayerState::Error(message.clone());
        (self.status.listener)(PlayerState::Error(message));
    }
}

/// The engine's answer to [`AudioEngine::play`], once it has opened the sound output (or failed
/// to).
#[must_use = "the playback may still fail: wait for the answer"]
pub struct PlayReply(mpsc::Receiver<Result<(), String>>);

impl PlayReply {
    /// Blocks until the engine answers (opening the sound output can take a few hundred ms):
    /// call it from a blocking thread, never from an async task.
    pub fn wait(self) -> Result<(), String> {
        self.0
            .recv()
            .map_err(|_| "The audio engine is stopped".to_string())?
    }
}

impl AudioEngine for DesktopAudioEngine {
    fn play(&mut self, url: &str, station: StationRef) -> Result<PlayReply, String> {
        let url: Url = url
            .trim()
            .parse()
            .map_err(|e| format!("Invalid URL: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("Unsupported protocol: {}", url.scheme()));
        }

        let (reply, wait) = mpsc::channel();
        self.commands
            .send(Command::Play {
                url,
                station,
                reply,
            })
            .map_err(|_| "The audio engine is stopped".to_string())?;
        Ok(PlayReply(wait))
    }

    // Both hold the state lock from the read to the write: an error, a stop or a new station
    // landing in between would otherwise be overwritten by a stale Paused or Playing.
    fn pause(&mut self) -> Result<(), String> {
        let mut state = self.status.lock();
        if *state != PlayerState::Playing {
            return Ok(());
        }
        self.controls.paused.store(true, Ordering::Relaxed);
        self.status.replace(&mut state, PlayerState::Paused);
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        let mut state = self.status.lock();
        self.controls.paused.store(false, Ordering::Relaxed);
        if *state == PlayerState::Paused {
            self.status.replace(&mut state, PlayerState::Playing);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.commands
            .send(Command::Stop)
            .map_err(|_| "The audio engine is stopped".to_string())
    }

    fn set_volume(&mut self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("Invalid volume".into());
        }
        self.controls
            .volume
            .store(gain(volume).to_bits(), Ordering::Relaxed);
        Ok(())
    }
}

/// Dedicated runtime: it drives the HTTP download while the decoder thread blocks on
/// synchronous reads from the `stream-download` buffer.
fn network_runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("audio-net")
        .enable_all()
        .build()
        .inspect_err(|e| tracing::error!("tokio runtime unavailable: {e}"))
        .ok()
}

/// What the engine thread hands to every playback session.
struct EngineCtx {
    controls: Arc<Controls>,
    status: Arc<Status>,
    web: Arc<WebTitles>,
    levels: Levels,
    runtime: tokio::runtime::Handle,
}

impl EngineCtx {
    fn run(&self, rx: mpsc::Receiver<Command>) {
        let mut session: Option<Session> = None;
        let mut playback: Option<Playback> = None;

        loop {
            // Nothing playing: nothing to watch either, so wait for the next command.
            let next = if playback.is_some() {
                rx.recv_timeout(OUTPUT_CHECK)
            } else {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            let command = match next {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    self.reopen_lost_output(&mut session, &mut playback);
                    continue;
                }
                // The engine was dropped.
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // In every case, the previous session is stopped first.
            session = None;
            playback = None;
            self.controls.set_station_gain(f32::NAN);

            match command {
                Command::Stop => {
                    self.status.clear_now_playing();
                    self.unpause(PlayerState::Idle);
                }
                Command::Play {
                    url,
                    station,
                    reply,
                } => {
                    self.status.clear_now_playing();
                    self.unpause(PlayerState::Connecting);
                    match self.start_session(url.clone(), station.clone()) {
                        Ok(new) => {
                            session = Some(new);
                            playback = Some(Playback {
                                url,
                                station,
                                output_lost: None,
                            });
                            let _ = reply.send(Ok(()));
                        }
                        Err(message) => {
                            self.status.set(PlayerState::Error(message.clone()));
                            let _ = reply.send(Err(message));
                        }
                    }
                }
            }
        }
        drop(session);
    }

    /// Clears the pause and sets `new` in one step: a `pause` between the two would see the old
    /// session's Playing and leave the next station paused.
    fn unpause(&self, new: PlayerState) {
        let mut state = self.status.lock();
        self.controls.paused.store(false, Ordering::Relaxed);
        self.status.replace(&mut state, new);
    }

    /// The sound output died (device unplugged, sound server restarted, wake from sleep): plays
    /// the station again on a new one, keeping the pause and the title.
    fn reopen_lost_output(&self, session: &mut Option<Session>, playback: &mut Option<Playback>) {
        let Some(play) = playback else { return };
        if session.as_ref().is_some_and(|s| !s.output.health.is_lost()) {
            return;
        }
        // The stream was given up: nothing to play again.
        if matches!(*self.status.lock(), PlayerState::Error(_)) {
            return;
        }
        if session.take().is_some() {
            tracing::warn!("sound output lost: reopening it");
            play.output_lost = Some(Instant::now());
            self.status.set(PlayerState::Reconnecting(
                "Reconnecting the sound output…".into(),
            ));
        }
        match self.start_session(play.url.clone(), play.station.clone()) {
            Ok(new) => {
                *session = Some(new);
                play.output_lost = None;
            }
            Err(message)
                if play
                    .output_lost
                    .is_some_and(|since| since.elapsed() > OUTPUT_RETRY) =>
            {
                tracing::warn!("sound output not reopened: {message}");
                *playback = None;
                self.status.set(PlayerState::Error(message));
            }
            Err(message) => tracing::debug!("sound output not reopened yet: {message}"),
        }
    }

    fn start_session(&self, url: Url, station: StationRef) -> Result<Session, String> {
        let (output, producer) = output::open(self.controls.clone())?;
        let level = loudness::StationLevel::new(
            self.levels.get().cloned(),
            level_key(station.id.as_deref(), &url),
            station.level.as_deref(),
            output.format.sample_rate,
            output.format.channels,
            self.controls.normalize(),
        );
        self.controls
            .set_station_gain(level.leveler.gain_db() as f32);
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status: self.status.clone(),
            controls: self.controls.clone(),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };

        let public_only = station.public_only;
        // Streams that do not announce their title: look for it on the web.
        self.web.spawn(
            url.clone(),
            station,
            reporter.clone(),
            cancel.clone(),
            &self.runtime,
        );

        decoder::spawn(decoder::Job {
            url,
            public_only,
            format: output.format,
            producer,
            level,
            reporter,
            cancel: cancel.clone(),
            runtime: self.runtime.clone(),
            // The network is watched while the decoder holds this (until the playback stops).
            net: network::subscribe(),
        })
        .map_err(|e| format!("Failed to start the decoder: {e}"))?;

        Ok(Session { cancel, output })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn volume_curve_is_perceptual() {
        use super::gain;
        assert_eq!(gain(0.0), 0.0);
        assert_eq!(gain(1.0), 1.0);
        assert_eq!(gain(-1.0), 0.0);
        assert_eq!(gain(2.0), 1.0);
        // Half the slider is about -18 dB, not the -6 dB of a linear gain.
        let half_db = 20.0 * gain(0.5).log10();
        assert!((-19.0..-17.0).contains(&half_db), "{half_db}");
        // Monotonic: moving the slider up never lowers the sound.
        let steps: Vec<f32> = (0..=100).map(|i| gain(i as f32 / 100.0)).collect();
        assert!(steps.windows(2).all(|w| w[0] < w[1]));
    }

    use super::*;
    use crate::now_playing::WebEvent;

    #[test]
    fn play_reply_gives_the_engine_answer() {
        let (reply, wait) = mpsc::channel();
        reply.send(Err("No sound output".to_owned())).unwrap();
        assert_eq!(PlayReply(wait).wait(), Err("No sound output".to_owned()));

        let (reply, wait) = mpsc::channel();
        reply.send(Ok(())).unwrap();
        assert_eq!(PlayReply(wait).wait(), Ok(()));

        // The engine thread is gone: no answer will come.
        let (reply, wait) = mpsc::channel::<Result<(), String>>();
        drop(reply);
        assert_eq!(
            PlayReply(wait).wait(),
            Err("The audio engine is stopped".to_owned())
        );
    }

    /// The request returns before the engine has answered: the caller may drop its locks and
    /// wait elsewhere.
    #[test]
    fn play_returns_before_the_engine_answers() {
        let (commands, received) = mpsc::channel();
        let mut engine = DesktopAudioEngine {
            commands,
            controls: Arc::new(Controls::new()),
            status: Arc::new(Status {
                state: Mutex::new(PlayerState::Idle),
                listener: Arc::new(|_| {}),
                now_playing: Mutex::new(NowPlaying::default()),
                now_playing_listener: Arc::new(|_| {}),
            }),
            levels: Levels::default(),
        };
        assert!(engine
            .play("ftp://example.org/live", StationRef::default())
            .is_err());

        let pending = engine
            .play(" https://example.org/live ", StationRef::default())
            .unwrap();
        let Ok(Command::Play { url, reply, .. }) = received.try_recv() else {
            panic!("no play command sent");
        };
        assert_eq!(url.as_str(), "https://example.org/live");
        reply.send(Ok(())).unwrap();
        assert_eq!(pending.wait(), Ok(()));
    }

    #[test]
    fn cancel_runs_hook_once_even_if_registered_late() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));

        let h = hits.clone();
        let _first = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        cancel.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let h = hits.clone();
        let _late = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancel_runs_every_registered_hook() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));
        let hooks: Vec<_> = (0..3)
            .map(|_| {
                let h = hits.clone();
                cancel.on_cancel_scoped(move || {
                    h.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        cancel.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        drop(hooks);
    }

    #[test]
    fn an_attempt_takes_its_hook_back_when_it_ends() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));
        for _ in 0..100 {
            let h = hits.clone();
            let _attempt = cancel.on_cancel_scoped(move || {
                h.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert!(cancel.hooks.lock().unwrap().is_empty(), "no hook piles up");
        let h = hits.clone();
        let _current = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "only the attempt still going runs"
        );
    }

    #[test]
    fn cancelled_reporter_cannot_publish() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(move |s| sink.lock().unwrap().push(s)),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(|_| {}),
        });
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status,
            controls: Arc::new(Controls::new()),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };

        reporter.reconnecting("Reconnecting…");
        reporter.reconnecting("Reconnecting…"); // repeated while waiting: published once
        cancel.cancel();
        reporter.audible();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![PlayerState::Reconnecting("Reconnecting…".into())]
        );
    }

    #[test]
    fn state_serializes_for_the_frontend() {
        let json = |s: &PlayerState| serde_json::to_string(s).unwrap();
        assert_eq!(json(&PlayerState::Idle), r#"{"status":"idle"}"#);
        assert_eq!(
            json(&PlayerState::Error("boom".into())),
            r#"{"status":"error","message":"boom"}"#
        );
        assert_eq!(
            json(&PlayerState::Reconnecting(
                "Waiting for the network…".into()
            )),
            r#"{"status":"reconnecting","message":"Waiting for the network…"}"#
        );
    }

    #[test]
    fn now_playing_is_deduplicated_and_gated_by_cancel() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(|_| {}),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(move |np| sink.lock().unwrap().push(np)),
        });
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status: status.clone(),
            controls: Arc::new(Controls::new()),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };

        reporter.station(Some("Radio".into()));
        reporter.title("A - B".into());
        reporter.title("A - B".into()); // duplicate (e.g. after a reconnect): ignored
        cancel.cancel();
        reporter.title("late".into()); // cancelled session: ignored

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[1],
            NowPlaying {
                station: Some("Radio".into()),
                title: Some("A - B".into()),
                source: Some(TitleSource::Icy),
                ..Default::default()
            }
        );
        assert!(reporter.stream_meta().has_inband_title());
    }

    #[tokio::test]
    async fn cancelled_returns_once_the_session_is_cancelled() {
        let cancel = Arc::new(Cancel::default());
        let waiter = tokio::spawn({
            let cancel = cancel.clone();
            async move { cancel.cancelled().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("still waiting after the cancel")
            .unwrap();
        // Already cancelled: returns at once.
        tokio::time::timeout(Duration::from_secs(1), cancel.cancelled())
            .await
            .unwrap();
    }

    #[test]
    fn what_the_stream_learns_is_signalled() {
        let reporter = Reporter {
            status: Arc::new(Status {
                state: Mutex::new(PlayerState::Idle),
                listener: Arc::new(|_| {}),
                now_playing: Mutex::new(NowPlaying::default()),
                now_playing_listener: Arc::new(|_| {}),
            }),
            controls: Arc::new(Controls::new()),
            cancel: Arc::new(Cancel::default()),
            stream: Arc::new(StreamMeta::default()),
        };
        let mut changes = reporter.stream_meta().changes();
        assert!(!changes.has_changed().unwrap());
        reporter.stream_opened(true, None);
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        reporter.title("A - B".into());
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        reporter.stream_tags(None, "C".into());
        assert!(changes.has_changed().unwrap());
    }

    #[test]
    fn icy_details_complete_the_title_heard_only() {
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(|_| {}),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(|_| {}),
        });
        let reporter = Reporter {
            status: status.clone(),
            controls: Arc::new(Controls::new()),
            cancel: Arc::new(Cancel::default()),
            stream: Arc::new(StreamMeta::default()),
        };
        let track = |artist: &str, title: &str| TrackInfo {
            artist: Some(artist.into()),
            title: title.into(),
            started_at: None,
            duration_secs: Some(245),
            cover: None,
        };
        reporter.title("082 - Dru Down - Pimp of the Year".into());
        assert_eq!(
            reporter.stream_meta().icy_title().as_deref(),
            Some("082 - Dru Down - Pimp of the Year")
        );
        reporter.icy_details(
            "082 - Dru Down - Pimp of the Year",
            track("Dru Down", "Pimp of the Year"),
            Some(track("Kurupt", "We Can Freak It")),
            None,
        );
        let np = status.now_playing().clone();
        assert_eq!(
            np.title.as_deref(),
            Some("082 - Dru Down - Pimp of the Year")
        ); // the stream's
        assert_eq!(np.track.unwrap().artist.as_deref(), Some("Dru Down"));
        assert_eq!(np.next.unwrap().title, "We Can Freak It");
        assert_eq!(np.source, Some(TitleSource::Icy));

        // Details arriving after the stream moved on are dropped.
        reporter.title("Air - Sexy Boy".into());
        reporter.icy_details(
            "082 - Dru Down - Pimp of the Year",
            track("Dru Down", "Pimp of the Year"),
            None,
            None,
        );
        let np = status.now_playing().clone();
        assert_eq!(
            (np.title.as_deref(), np.track),
            (Some("Air - Sexy Boy"), None)
        );
    }

    /// Plays Fréquence 7 muted: its stream has no metadata, the title comes from its web page.
    #[test]
    #[ignore = "network and sound card"]
    fn live_title_from_a_recipe() {
        let seen = Arc::new(Mutex::new(Vec::<NowPlaying>::new()));
        let sink = seen.clone();
        let recipes = Arc::new(crate::recipes::Recipes::new(None));
        let web = Arc::new(WebTitles::new(recipes, true, Box::new(|_| {})));
        let mut engine = DesktopAudioEngine::new(
            Arc::new(|_| {}),
            Arc::new(move |np| sink.lock().unwrap().push(np)),
            web,
        );
        engine.set_volume(0.0).unwrap();
        engine
            .play("https://live.aurafm.org/Frequence7", StationRef::default())
            .unwrap()
            .wait()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if seen
                .lock()
                .unwrap()
                .iter()
                .any(|np| np.source == Some(TitleSource::Recipe))
            {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(200));
        }
        engine.stop().unwrap();
        let seen = seen.lock().unwrap();
        let np = seen
            .iter()
            .rev()
            .find(|np| np.source == Some(TitleSource::Recipe))
            .expect("a title from the recipe");
        println!("{np:#?}");
        let track = np.track.as_ref().unwrap();
        assert!(
            track.started_at.is_some(),
            "start resolved from the page's local time"
        );
    }

    /// Plays Fréquence 7 muted, then saves a local copy of its recipe: the playback switches to
    /// it without reconnecting and the notice says what it reads.
    #[test]
    #[ignore = "network and sound card"]
    fn live_saved_recipe_applies_while_playing() {
        let dir =
            std::env::temp_dir().join(format!("soundfolk-live-recipes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = events.clone();
        let recipes = Arc::new(crate::recipes::Recipes::new(Some(dir.clone())));
        let web = Arc::new(WebTitles::new(
            recipes.clone(),
            true,
            Box::new(move |event| {
                let line = match event {
                    WebEvent::Status(s) => format!(
                        "status {:?} {:?} {:?}",
                        s.state,
                        s.recipe.map(|r| (r.id, r.origin)),
                        s.error
                    ),
                    WebEvent::Notice(n) => format!("notice {} ok={} {}", n.file, n.ok, n.message),
                };
                sink.lock().unwrap().push(line);
            }),
        ));
        let titles = Arc::new(Mutex::new(Vec::<NowPlaying>::new()));
        let title_sink = titles.clone();
        let states = Arc::new(Mutex::new(Vec::<PlayerState>::new()));
        let state_sink = states.clone();
        let mut engine = DesktopAudioEngine::new(
            Arc::new(move |s| state_sink.lock().unwrap().push(s)),
            Arc::new(move |np| title_sink.lock().unwrap().push(np)),
            web.clone(),
        );
        engine.set_volume(0.0).unwrap();
        engine
            .play("https://live.aurafm.org/Frequence7", StationRef::default())
            .unwrap()
            .wait()
            .unwrap();
        let wait_for = |what: &dyn Fn() -> bool, secs: u64| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
            while std::time::Instant::now() < deadline && !what() {
                thread::sleep(std::time::Duration::from_millis(200));
            }
        };
        wait_for(
            &|| {
                titles
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|np| np.source == Some(TitleSource::Recipe))
            },
            20,
        );

        // Broken first: reported, the shipped recipe stays in use.
        std::fs::write(dir.join("frequence7.toml"), "schema = 1\nname = ").unwrap();
        web.recipes_changed(&recipes.reload_if_changed().unwrap());
        // Then a valid copy: applied to the playback, read within the minimum interval.
        let copy = crate::recipes::local_copy(recipes.bundled_text("frequence7").unwrap());
        std::fs::write(dir.join("frequence7.toml"), copy).unwrap();
        web.recipes_changed(&recipes.reload_if_changed().unwrap());
        wait_for(
            &|| {
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("notice frequence7.toml ok=true"))
            },
            25,
        );
        engine.stop().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let events = events.lock().unwrap();
        println!("{events:#?}\nstates: {:?}", states.lock().unwrap());
        assert!(
            events
                .iter()
                .any(|e| e.starts_with("notice frequence7.toml ok=false")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| e.starts_with("notice frequence7.toml ok=true Reads: ")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| e.contains("Some((\"frequence7\", Local))")),
            "{events:?}"
        );
        // Switching recipes never touches the stream: no reconnection.
        assert_eq!(
            *states.lock().unwrap(),
            vec![PlayerState::Connecting, PlayerState::Playing]
        );
    }

    /// Plays an Ogg Vorbis stream muted: the title comes from its Vorbis comments. Set
    /// `SOUNDFOLK_OGG_SECS` to listen longer and see chain changes (one per track).
    #[test]
    #[ignore = "network and sound card"]
    fn live_title_from_ogg_tags() {
        let seen = Arc::new(Mutex::new(Vec::<NowPlaying>::new()));
        let states = Arc::new(Mutex::new(Vec::<PlayerState>::new()));
        let (sink, state_sink) = (seen.clone(), states.clone());
        let web = Arc::new(WebTitles::new(
            Arc::new(crate::recipes::Recipes::new(None)),
            false,
            Box::new(|_| {}),
        ));
        let mut engine = DesktopAudioEngine::new(
            Arc::new(move |s| state_sink.lock().unwrap().push(s)),
            Arc::new(move |np| sink.lock().unwrap().push(np)),
            web,
        );
        engine.set_volume(0.0).unwrap();
        engine
            .play(
                "http://stream.metacast.eu/radio1rock.ogg",
                StationRef::default(),
            )
            .unwrap()
            .wait()
            .unwrap();
        let secs = std::env::var("SOUNDFOLK_OGG_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        thread::sleep(std::time::Duration::from_secs(secs));
        engine.stop().unwrap();
        let titles: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|np| np.title.clone().zip(np.source))
            .collect();
        println!("titles: {titles:#?}\nstates: {:?}", states.lock().unwrap());
        assert!(
            titles
                .iter()
                .any(|(_, source)| *source == TitleSource::StreamTags),
            "{titles:?}"
        );
        assert!(!states
            .lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, PlayerState::Error(_))));
    }

    /// A local server sends 1 s of a WAV stream then hangs up, every time: the engine
    /// reconnects each time on its own (no 5-attempt limit any more) and plays again.
    #[test]
    #[ignore = "sound card"]
    fn live_reconnects_every_time_the_server_hangs_up() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/live.wav", listener.local_addr().unwrap());
        let served = Arc::new(AtomicU32::new(0));
        let count = served.clone();
        thread::spawn(move || {
            for mut client in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                let mut request = [0u8; 1024];
                let _ = client.read(&mut request);
                let rate = 16_000u32;
                let mut wav = Vec::new();
                wav.extend_from_slice(b"RIFF\xff\xff\xff\x7fWAVEfmt ");
                wav.extend_from_slice(&16u32.to_le_bytes());
                wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
                wav.extend_from_slice(&1u16.to_le_bytes()); // mono
                wav.extend_from_slice(&rate.to_le_bytes());
                wav.extend_from_slice(&(rate * 2).to_le_bytes());
                wav.extend_from_slice(&2u16.to_le_bytes());
                wav.extend_from_slice(&16u16.to_le_bytes());
                wav.extend_from_slice(b"data\x00\x00\xf0\x7f");
                wav.resize(wav.len() + rate as usize * 2, 0); // 1 s of silence
                let _ = client.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n",
                );
                let _ = client.write_all(&wav);
                // Hangs up: the stream ends.
            }
        });

        let states = Arc::new(Mutex::new(Vec::<PlayerState>::new()));
        let sink = states.clone();
        let web = Arc::new(WebTitles::new(
            Arc::new(crate::recipes::Recipes::new(None)),
            false,
            Box::new(|_| {}),
        ));
        let mut engine = DesktopAudioEngine::new(
            Arc::new(move |s| sink.lock().unwrap().push(s)),
            Arc::new(|_| {}),
            web,
        );
        engine.set_volume(0.0).unwrap();
        engine
            .play(&url, StationRef::default())
            .unwrap()
            .wait()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
        while std::time::Instant::now() < deadline && served.load(Ordering::SeqCst) < 8 {
            thread::sleep(std::time::Duration::from_millis(200));
        }
        thread::sleep(std::time::Duration::from_millis(1500));
        let seen = states.lock().unwrap().clone();
        engine.stop().unwrap();
        println!(
            "connections: {}\nstates: {seen:?}",
            served.load(Ordering::SeqCst)
        );
        assert!(
            served.load(Ordering::SeqCst) >= 8,
            "more than the old 5-attempt limit"
        );
        assert!(
            !seen.iter().any(|s| matches!(s, PlayerState::Error(_))),
            "{seen:?}"
        );
        let replays = seen
            .windows(2)
            .filter(|w| {
                matches!(w[0], PlayerState::Reconnecting(_)) && w[1] == PlayerState::Playing
            })
            .count();
        assert!(replays >= 6, "{seen:?}");
    }

    /// Serves 1 s of silent WAV per connection, then hangs up; counts the connections.
    fn hanging_up_station() -> (String, Arc<AtomicU32>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/live.wav", listener.local_addr().unwrap());
        let served = Arc::new(AtomicU32::new(0));
        let count = served.clone();
        thread::spawn(move || {
            for mut client in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                let mut request = [0u8; 1024];
                let _ = client.read(&mut request);
                let rate = 16_000u32;
                let mut wav = Vec::new();
                wav.extend_from_slice(b"RIFF\xff\xff\xff\x7fWAVEfmt ");
                wav.extend_from_slice(&16u32.to_le_bytes());
                wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
                wav.extend_from_slice(&1u16.to_le_bytes()); // mono
                wav.extend_from_slice(&rate.to_le_bytes());
                wav.extend_from_slice(&(rate * 2).to_le_bytes());
                wav.extend_from_slice(&2u16.to_le_bytes());
                wav.extend_from_slice(&16u16.to_le_bytes());
                wav.extend_from_slice(b"data\x00\x00\xf0\x7f");
                wav.resize(wav.len() + rate as usize * 2, 0);
                let _ = client.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n",
                );
                let _ = client.write_all(&wav);
            }
        });
        (url, served)
    }

    /// The decoder's reconnection loop, without a sound card: its samples go to a buffer this
    /// test empties. Each time the server hangs up, the player reconnects and plays again; it
    /// never gives up on a station that played.
    #[test]
    fn the_decoder_reconnects_every_time_the_server_hangs_up() {
        let (url, served) = hanging_up_station();
        let states = Arc::new(Mutex::new(Vec::<PlayerState>::new()));
        let sink = states.clone();
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(move |s| sink.lock().unwrap().push(s)),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(|_| {}),
        });
        let cancel = Arc::new(Cancel::default());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let format = output::OutputFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(48_000 * 2);
        let net = tokio::sync::watch::Sender::new(network::NetState {
            online: true,
            ..Default::default()
        });
        let drained = cancel.clone();
        let drain = thread::spawn(move || {
            while !drained.is_cancelled() {
                while consumer.pop().is_ok() {}
                thread::sleep(Duration::from_millis(5));
            }
        });
        decoder::spawn(decoder::Job {
            url: url.parse().unwrap(),
            public_only: false,
            level: loudness::StationLevel::new(None, url.clone(), None, 48_000, 2, false),
            format,
            producer,
            reporter: Reporter {
                status,
                controls: Arc::new(Controls::new()),
                cancel: cancel.clone(),
                stream: Arc::new(StreamMeta::default()),
            },
            cancel: cancel.clone(),
            runtime: runtime.handle().clone(),
            // Online, whatever the machine running the test says: the test is about the loop.
            net: net.subscribe(),
        })
        .unwrap();

        let replays = || {
            states
                .lock()
                .unwrap()
                .windows(2)
                .filter(|w| {
                    matches!(w[0], PlayerState::Reconnecting(_)) && w[1] == PlayerState::Playing
                })
                .count()
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline && replays() < 3 {
            thread::sleep(Duration::from_millis(50));
        }
        cancel.cancel();
        drain.join().unwrap();
        let seen = states.lock().unwrap().clone();
        assert!(replays() >= 3, "{seen:?}");
        assert!(served.load(Ordering::SeqCst) >= 4);
        assert!(
            !seen.iter().any(|s| matches!(s, PlayerState::Error(_))),
            "{seen:?}"
        );
    }

    /// A local station plays for 1 s, then its server stops for good: the player says whose
    /// fault it is, only when proven. With Internet: the station's. With no route at all
    /// (`unshare -rn`, `SOUNDFOLK_EXPECT=no-network`): the connection's. In a network namespace
    /// whose default route leads nowhere while the desktop still sees Internet
    /// (`SOUNDFOLK_EXPECT=unknown`): the signals disagree, nobody is blamed.
    #[test]
    #[ignore = "sound card"]
    fn live_tells_whether_the_station_or_internet_is_down() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/live.wav", listener.local_addr().unwrap());
        thread::spawn(move || {
            // One listener only, then the "server" is gone: later connections are refused.
            let (mut client, _) = listener.accept().unwrap();
            let _ = client.read(&mut [0u8; 1024]);
            let mut wav = b"RIFF\xff\xff\xff\x7fWAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\xf0\x7f".to_vec();
            wav.resize(wav.len() + 32_000, 0);
            let _ = client.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\n\r\n");
            let _ = client.write_all(&wav);
        });

        let states = Arc::new(Mutex::new(Vec::<PlayerState>::new()));
        let sink = states.clone();
        let web = Arc::new(WebTitles::new(
            Arc::new(crate::recipes::Recipes::new(None)),
            false,
            Box::new(|_| {}),
        ));
        let mut engine = DesktopAudioEngine::new(
            Arc::new(move |s| sink.lock().unwrap().push(s)),
            Arc::new(|_| {}),
            web,
        );
        engine.set_volume(0.0).unwrap();
        engine
            .play(&url, StationRef::default())
            .unwrap()
            .wait()
            .unwrap();
        thread::sleep(std::time::Duration::from_secs(15));
        let seen = states.lock().unwrap().clone();
        engine.stop().unwrap();
        println!("states: {seen:#?}");
        let expected = match std::env::var("SOUNDFOLK_EXPECT").as_deref() {
            Ok("unknown") => {
                "Cannot play the station (its server refuses connections) — retrying…".to_owned()
            }
            Ok("no-network") => "No network — waiting…".to_owned(),
            _ => "The station is not broadcasting (its server refuses connections) — retrying…"
                .to_owned(),
        };
        assert!(seen.contains(&PlayerState::Playing), "{seen:?}");
        assert!(
            seen.contains(&PlayerState::Reconnecting(expected.clone())),
            "expected {expected:?} in {seen:?}"
        );
    }

    /// Against the real network: `cargo test -- --ignored live_`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "network"]
    async fn live_resolves_a_pls_then_probes_the_stream() {
        let pls: Url = "https://somafm.com/groovesalad.pls".parse().unwrap();
        let stream = resolve_stream_url(pls, false).await.unwrap();
        assert_ne!(stream.path(), "/groovesalad.pls");

        let handle = tokio::runtime::Handle::current();
        let info = tokio::task::spawn_blocking(move || probe_stream(&stream, &handle, false))
            .await
            .unwrap()
            .unwrap();
        assert!(info.sample_rate >= 22_050 && info.channels >= 1, "{info:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "network"]
    async fn live_rejects_non_audio_and_dead_urls() {
        let handle = tokio::runtime::Handle::current();
        for url in ["https://example.com/", "http://127.0.0.1:9/"] {
            let url: Url = url.parse().unwrap();
            let h = handle.clone();
            let result = tokio::task::spawn_blocking(move || probe_stream(&url, &h, false))
                .await
                .unwrap();
            assert!(result.is_err(), "{result:?}");
        }
    }

    /// Plays real stations muted and prints the loudness *Constant volume* learns for each
    /// (`docs/11-loudness.md` §6): `cargo test -- --ignored live_loudness --nocapture`. Other
    /// streams: `SOUNDFOLK_STREAMS="url url…"`, `SOUNDFOLK_SECONDS` each (default 20).
    #[test]
    #[ignore = "network and sound card"]
    fn live_loudness() {
        #[derive(Default)]
        struct Memory(Mutex<Vec<(String, StoredLevel)>>);
        impl LevelMemory for Memory {
            fn recall(&self, _: &str) -> Option<StoredLevel> {
                None
            }
            fn guess(&self) -> Option<f64> {
                None
            }
            fn remember(&self, station: &str, level: &StoredLevel) {
                self.0.lock().unwrap().push((station.to_owned(), *level));
            }
        }

        let streams = std::env::var("SOUNDFOLK_STREAMS").unwrap_or_else(|_| {
            "https://icecast.radiofrance.fr/fip-midfi.mp3 http://stream.radioparadise.com/mp3-128 \
             http://stream.metacast.eu/radio1rock.ogg https://live.aurafm.org/Frequence7"
                .into()
        });
        let seconds = std::env::var("SOUNDFOLK_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let memory = Arc::new(Memory::default());
        let web = Arc::new(WebTitles::new(
            Arc::new(crate::recipes::Recipes::new(None)),
            false,
            Box::new(|_| {}),
        ));
        let mut engine = DesktopAudioEngine::new(Arc::new(|_| {}), Arc::new(|_| {}), web);
        engine.set_level_memory(memory.clone());
        engine.set_volume(0.0).unwrap();
        for url in streams.split_whitespace() {
            engine
                .play(url, StationRef::default())
                .unwrap()
                .wait()
                .unwrap();
            let mut gains = Vec::new();
            for _ in 0..seconds {
                thread::sleep(Duration::from_secs(1));
                gains.push(
                    engine
                        .level_adjustment()
                        .map_or("-".into(), |g| format!("{g:+.1}")),
                );
            }
            engine.stop().unwrap();
            thread::sleep(Duration::from_millis(500));
            println!("{url}\n  gain each second: {}", gains.join(" "));
        }
        let learned = memory.0.lock().unwrap();
        for (url, level) in learned.iter() {
            println!(
                "{:>6.1} LUFS  peak {:>5.1} dBFS  {url}",
                level.loudness_lufs, level.peak_dbfs
            );
            assert!((-60.0..0.0).contains(&level.loudness_lufs), "{level:?}");
        }
        assert!(!learned.is_empty(), "no station learned");
    }
}
