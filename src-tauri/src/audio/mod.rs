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

mod decoder;
mod icy;
mod output;
mod playlist;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde::Serialize;
pub use stream_download::http::reqwest::Url;

pub use decoder::probe as probe_stream;
pub use playlist::resolve_stream_url;

/// Common facade for all platforms (see `docs/04-audio-engine.md`).
pub trait AudioEngine {
    fn play(&mut self, url: &str) -> Result<(), String>;
    /// Pause of a live stream: the sound is muted but the stream keeps being consumed,
    /// so `resume` picks up **live**, not where playback was paused.
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    /// `volume` is clamped to `0.0..=1.0`.
    fn set_volume(&mut self, volume: f32) -> Result<(), String>;
}

/// State published to the frontend (`{ "status": "error", "message": "…" }`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", content = "message", rename_all = "snake_case")]
pub enum PlayerState {
    Idle,
    Connecting,
    Playing,
    Paused,
    Error(String),
}

/// What is on air: station name (`icy-name`) and current title (`StreamTitle`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NowPlaying {
    pub station: Option<String>,
    pub title: Option<String>,
}

pub type StateListener = Arc<dyn Fn(PlayerState) + Send + Sync>;
pub type NowPlayingListener = Arc<dyn Fn(NowPlaying) + Send + Sync>;

/// Parameters read by the real-time callback (atomics: no locks on the audio side).
pub(crate) struct Controls {
    volume: AtomicU32,
    paused: AtomicBool,
}

impl Controls {
    fn new() -> Self {
        Self { volume: AtomicU32::new(1.0f32.to_bits()), paused: AtomicBool::new(false) }
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
        let mut state = self.lock();
        if *state != new {
            *state = new.clone();
            (self.listener)(new);
        }
    }
}

/// Cancellation flag for a session, with a hook to interrupt a blocked network read
/// (without it, `Stop` would wait for the HTTP read timeout to expire).
#[derive(Default)]
pub(crate) struct Cancel {
    flag: AtomicBool,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Cancel {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let hook = self.hook.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Registers the action to run on cancellation (immediately if already cancelled).
    pub(crate) fn on_cancel(&self, hook: impl FnOnce() + Send + 'static) {
        let mut slot = self.hook.lock().unwrap_or_else(|p| p.into_inner());
        if self.is_cancelled() {
            drop(slot);
            hook();
        } else {
            *slot = Some(Box::new(hook));
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
}

impl Reporter {
    fn publish(&self, new: PlayerState) {
        let mut state = self.status.lock();
        if self.cancel.is_cancelled() || *state == new {
            return;
        }
        *state = new.clone();
        (self.status.listener)(new);
    }

    pub(crate) fn connecting(&self) {
        self.publish(PlayerState::Connecting);
    }

    pub(crate) fn audible(&self) {
        let paused = self.controls.is_paused();
        self.publish(if paused { PlayerState::Paused } else { PlayerState::Playing });
    }

    pub(crate) fn failed(&self, message: String) {
        self.publish(PlayerState::Error(message));
    }

    pub(crate) fn station(&self, name: Option<String>) {
        self.update_now_playing(|np| np.station = name);
    }

    pub(crate) fn title(&self, title: String) {
        self.update_now_playing(|np| np.title = Some(title));
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
    Play { url: Url, reply: Sender<Result<(), String>> },
    Stop,
}

/// A playback in progress. `Drop` silences the sound (the cpal `Stream` is destroyed) and cancels the network.
struct Session {
    cancel: Arc<Cancel>,
    _output: output::Output,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct DesktopAudioEngine {
    commands: Sender<Command>,
    controls: Arc<Controls>,
    status: Arc<Status>,
}

impl DesktopAudioEngine {
    pub fn new(listener: StateListener, now_playing_listener: NowPlayingListener) -> Self {
        let controls = Arc::new(Controls::new());
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener,
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener,
        });
        let (commands, rx) = mpsc::channel();

        let (c, s) = (controls.clone(), status.clone());
        thread::Builder::new()
            .name("audio-engine".into())
            .spawn(move || engine_loop(rx, c, s))
            .expect("failed to start the audio-engine thread");

        Self { commands, controls, status }
    }

    pub fn state(&self) -> PlayerState {
        self.status.lock().clone()
    }

    pub fn now_playing(&self) -> NowPlaying {
        self.status.now_playing().clone()
    }
}

impl AudioEngine for DesktopAudioEngine {
    fn play(&mut self, url: &str) -> Result<(), String> {
        let url: Url = url.trim().parse().map_err(|e| format!("Invalid URL: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("Unsupported protocol: {}", url.scheme()));
        }

        let (reply, wait) = mpsc::channel();
        self.commands
            .send(Command::Play { url, reply })
            .map_err(|_| "The audio engine is stopped".to_string())?;
        wait.recv().map_err(|_| "The audio engine is stopped".to_string())?
    }

    fn pause(&mut self) -> Result<(), String> {
        if self.state() != PlayerState::Playing {
            return Ok(());
        }
        self.controls.paused.store(true, Ordering::Relaxed);
        self.status.set(PlayerState::Paused);
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        self.controls.paused.store(false, Ordering::Relaxed);
        if self.state() == PlayerState::Paused {
            self.status.set(PlayerState::Playing);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.commands.send(Command::Stop).map_err(|_| "The audio engine is stopped".to_string())
    }

    fn set_volume(&mut self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("Invalid volume".into());
        }
        self.controls.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        Ok(())
    }
}

fn engine_loop(rx: mpsc::Receiver<Command>, controls: Arc<Controls>, status: Arc<Status>) {
    // Dedicated runtime: it drives the HTTP download while the decoder thread
    // blocks on synchronous reads from the `stream-download` buffer.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("audio-net")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("tokio runtime unavailable: {e}");
            return;
        }
    };

    let mut session: Option<Session> = None;

    // The loop ends when the last `Sender` (the engine) is dropped.
    for command in rx {
        // In every case, the previous session is stopped first.
        session = None;

        match command {
            Command::Stop => {
                controls.paused.store(false, Ordering::Relaxed);
                status.clear_now_playing();
                status.set(PlayerState::Idle);
            }
            Command::Play { url, reply } => {
                controls.paused.store(false, Ordering::Relaxed);
                status.clear_now_playing();
                status.set(PlayerState::Connecting);
                match start_session(url, &controls, &status, runtime.handle()) {
                    Ok(new) => {
                        session = Some(new);
                        let _ = reply.send(Ok(()));
                    }
                    Err(message) => {
                        status.set(PlayerState::Error(message.clone()));
                        let _ = reply.send(Err(message));
                    }
                }
            }
        }
    }
    drop(session);
}

fn start_session(
    url: Url,
    controls: &Arc<Controls>,
    status: &Arc<Status>,
    runtime: &tokio::runtime::Handle,
) -> Result<Session, String> {
    let (output, producer) = output::open(controls.clone())?;
    let cancel = Arc::new(Cancel::default());
    let reporter =
        Reporter { status: status.clone(), controls: controls.clone(), cancel: cancel.clone() };

    decoder::spawn(decoder::Job {
        url,
        format: output.format,
        producer,
        reporter,
        cancel: cancel.clone(),
        runtime: runtime.clone(),
    })
    .map_err(|e| format!("Failed to start the decoder: {e}"))?;

    Ok(Session { cancel, _output: output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_runs_hook_once_even_if_registered_late() {
        let cancel = Cancel::default();
        let hits = Arc::new(AtomicU32::new(0));

        let h = hits.clone();
        cancel.on_cancel(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        cancel.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let h = hits.clone();
        cancel.on_cancel(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(hits.load(Ordering::SeqCst), 2);
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
        let reporter =
            Reporter { status, controls: Arc::new(Controls::new()), cancel: cancel.clone() };

        reporter.connecting();
        cancel.cancel();
        reporter.audible();
        assert_eq!(*seen.lock().unwrap(), vec![PlayerState::Connecting]);
    }

    #[test]
    fn state_serializes_for_the_frontend() {
        let json = |s: &PlayerState| serde_json::to_string(s).unwrap();
        assert_eq!(json(&PlayerState::Idle), r#"{"status":"idle"}"#);
        assert_eq!(
            json(&PlayerState::Error("boom".into())),
            r#"{"status":"error","message":"boom"}"#
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
        };

        reporter.station(Some("Radio".into()));
        reporter.title("A - B".into());
        reporter.title("A - B".into()); // duplicate (e.g. after a reconnect): ignored
        cancel.cancel();
        reporter.title("late".into()); // cancelled session: ignored

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1], NowPlaying { station: Some("Radio".into()), title: Some("A - B".into()) });
    }

    /// Against the real network: `cargo test -- --ignored live_`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "network"]
    async fn live_resolves_a_pls_then_probes_the_stream() {
        let pls: Url = "https://somafm.com/groovesalad.pls".parse().unwrap();
        let stream = resolve_stream_url(pls).await.unwrap();
        assert_ne!(stream.path(), "/groovesalad.pls");

        let handle = tokio::runtime::Handle::current();
        let info = tokio::task::spawn_blocking(move || probe_stream(&stream, &handle))
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
            let result = tokio::task::spawn_blocking(move || probe_stream(&url, &h)).await.unwrap();
            assert!(result.is_err(), "{result:?}");
        }
    }
}
