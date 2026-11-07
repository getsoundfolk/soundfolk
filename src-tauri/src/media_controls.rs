// SPDX-License-Identifier: MPL-2.0
//! System media controls (MPRIS on Linux): the desktop shows the station and the title on air,
//! and its play / pause / stop buttons and the keyboard media keys drive the player. Next and
//! previous switch between favorite stations.
//!
//! In a radio app the station comes first (next / previous change it), so the desktop's first
//! line, the MPRIS title, is the station and its second line, the MPRIS artist, is the track.
//! The station is also the album, for desktops that show it, and its logo the picture.
//!
//! Linux only for now: Windows needs the window's HWND and macOS the main thread's run loop.
//! Elsewhere [`SystemMedia`] does nothing. The tray menu drives the player through [`control`] too,
//! so that it can never behave differently from the media keys.
//!
//! Next and previous skip a favorite that is proven not to broadcast before its first sound (the
//! engine only gives up at once when the station is at fault, see `audio/failure.rs`): someone
//! zapping from the tray or the media keys is not looking at the window, and silence would explain
//! nothing. Without proof (no network, no Internet) the player waits as usual; a favorite gone
//! from the catalog stops the zapping, so that the window can offer to remove it; after a full
//! turn without sound, the last error is shown. The window lists what was skipped.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::audio::{AudioEngine, NowPlaying, PlayerState};
use crate::commands::{start_station, AudioState};
use crate::db::{Database, Favorite};
use crate::error::AppResult;

/// Event emitted when the media controls switch station (payload: `StationChange`), so that the
/// window shows the station on air.
const STATION_EVENT: &str = "player:station";

#[derive(Debug, Clone, Serialize)]
struct StationChange {
    id: String,
    name: String,
    url: String,
    genre: Option<String>,
    /// The window must show the click-counting notice (once only).
    show_click_notice: bool,
    /// Favorites skipped on the way, because they did not broadcast.
    skipped: Vec<Skipped>,
}

/// A favorite skipped by Next / Previous.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Skipped {
    name: String,
    /// Why, as the player said it.
    reason: String,
}

/// The Next / Previous in progress, until its station is heard (Tauri state).
#[derive(Default)]
pub struct Zap(Mutex<Option<Zapping>>);

impl Zap {
    fn lock(&self) -> MutexGuard<'_, Option<Zapping>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Zapping {
    forward: bool,
    /// The favorite being tried.
    station_id: String,
    /// Favorites tried so far, this one included.
    tried: usize,
    skipped: Vec<Skipped>,
}

/// What a failed favorite leads to.
#[derive(Debug, PartialEq)]
enum Skip {
    /// The error is shown.
    Stop,
    /// The next favorite is played.
    To(Favorite, Vec<Skipped>),
}

impl Zapping {
    /// The favorite tried failed with `message`: the next one, unless all have been tried.
    fn skip(&mut self, favorites: &[Favorite], message: &str) -> Skip {
        if self.tried >= favorites.len() {
            return Skip::Stop;
        }
        let Some(next) = neighbour(favorites, Some(&self.station_id), self.forward).cloned() else {
            return Skip::Stop;
        };
        let name = favorites
            .iter()
            .find(|f| f.station_id == self.station_id)
            .map_or_else(|| self.station_id.clone(), |f| f.name.clone());
        let reason = message
            .strip_prefix("The station is not broadcasting: ")
            .unwrap_or(message)
            .to_owned();
        self.skipped.push(Skipped { name, reason });
        self.station_id = next.station_id.clone();
        self.tried += 1;
        Skip::To(next, self.skipped.clone())
    }
}

/// Called for every player state, after [`crate::favorite_repair::on_state`]; returns the state
/// to show. A favorite started by Next / Previous that fails before its first sound gives way to
/// the next one.
pub(crate) fn on_state(app: &tauri::AppHandle, state: PlayerState) -> PlayerState {
    let Some(zap) = app.try_state::<Zap>() else {
        return state;
    };
    let mut zap = zap.lock();
    let message = match &state {
        PlayerState::Error(message) => message,
        // Still trying: the zapping goes on.
        PlayerState::Connecting | PlayerState::Reconnecting(_) => return state,
        // Heard, or stopped: it is over.
        PlayerState::Playing | PlayerState::Paused | PlayerState::Idle => {
            *zap = None;
            return state;
        }
    };
    let Some(zapping) = zap.as_mut() else {
        return state;
    };
    let on_air = app
        .try_state::<AudioState>()
        .and_then(|audio| audio.station())
        .map(|s| s.id);
    // Another station failed: one started from the window, or the previous one before the
    // favorite took its place.
    if on_air.as_deref() != Some(zapping.station_id.as_str()) {
        return state;
    }
    // Gone from the catalog: the window offers to remove it.
    if message == crate::favorite_repair::GONE {
        *zap = None;
        return state;
    }
    let db = app.state::<Database>();
    let favorites = match db.list_favorites() {
        Ok(favorites) => favorites,
        Err(e) => {
            tracing::warn!("cannot skip to the next favorite: {e}");
            *zap = None;
            return state;
        }
    };
    let failed = zapping.station_id.clone();
    let Skip::To(next, mut skipped) = zapping.skip(&favorites, message) else {
        *zap = None;
        return state;
    };
    // Named as the user knows it.
    if let Ok(Some(label)) = db.station_name(&failed) {
        for list in [&mut skipped, &mut zapping.skipped] {
            if let Some(last) = list.last_mut() {
                last.name = label.clone();
            }
        }
    }
    let shown = PlayerState::Reconnecting(format!("Skipping {}…", skipped[skipped.len() - 1].name));
    tracing::info!(station = %next.station_id, "favorite not broadcasting: skipping to the next one");
    // Starting a station waits for the sound output, and the engine may be locked by the caller.
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = play_favorite(&app, next, skipped) {
            tracing::warn!("cannot skip to the next favorite: {e}");
        }
    });
    shown
}

#[cfg(target_os = "linux")]
pub use linux::SystemMedia;

#[cfg(not(target_os = "linux"))]
pub struct SystemMedia;

#[cfg(not(target_os = "linux"))]
impl SystemMedia {
    pub fn new(_app: tauri::AppHandle) -> Self {
        Self
    }

    pub fn show_state(&self, _state: &PlayerState) {}

    pub fn show_now_playing(
        &self,
        _now_playing: &NowPlaying,
        _station: Option<&str>,
        _label: Option<&str>,
        _cover: Option<&str>,
    ) {
    }
}

/// What the desktop displays: the station as the title, the track on air as the artist.
// Only the Linux backend shows it; the tests run everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq)]
struct Display {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

/// `station` is the station's own name; the stream's `icy-name` is the fallback. `label` is the
/// name the user gave it, shown instead. A title that is only the station name (sent by some radios
/// between tracks) is not a track. The track is rebuilt as `Artist - Title`, without the station
/// signature some radios append (found with the station's own name).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn display(now_playing: &NowPlaying, station: Option<&str>, label: Option<&str>) -> Display {
    let station = station
        .or(now_playing.station.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let track = now_playing
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !station.is_some_and(|s| s.eq_ignore_ascii_case(t)))
        .map(|raw| now_playing.parsed(raw, station).display());
    let label = label.map(str::trim).filter(|l| !l.is_empty());
    match label.or(station) {
        Some(station) => Display {
            title: Some(station.to_owned()),
            artist: track,
            album: Some(station.to_owned()),
        },
        // Station unknown: the track takes the first line
        None => Display {
            title: track,
            artist: None,
            album: None,
        },
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{Mutex, MutexGuard};

    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig,
    };

    use super::{control, display, Command, NowPlaying, PlayerState};

    pub struct SystemMedia {
        /// `None` when the controls could not be created: the player works without them.
        controls: Option<Mutex<MediaControls>>,
    }

    impl SystemMedia {
        pub fn new(app: tauri::AppHandle) -> Self {
            let config = PlatformConfig {
                display_name: "Soundfolk",
                dbus_name: "soundfolk",
                hwnd: None,
            };
            let controls = MediaControls::new(config).and_then(|mut controls| {
                // Called on the D-Bus thread: the engine may block for a while, so it is
                // driven from a worker thread instead.
                controls.attach(move |event| {
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || handle(&app, event));
                })?;
                Ok(controls)
            });
            match controls {
                Ok(controls) => Self {
                    controls: Some(Mutex::new(controls)),
                },
                Err(e) => {
                    tracing::warn!("system media controls unavailable: {e:?}");
                    Self { controls: None }
                }
            }
        }

        fn controls(&self) -> Option<MutexGuard<'_, MediaControls>> {
            self.controls
                .as_ref()
                .map(|c| c.lock().unwrap_or_else(|p| p.into_inner()))
        }

        pub fn show_state(&self, state: &PlayerState) {
            let playback = match state {
                PlayerState::Connecting | PlayerState::Reconnecting(_) | PlayerState::Playing => {
                    MediaPlayback::Playing { progress: None }
                }
                PlayerState::Paused => MediaPlayback::Paused { progress: None },
                PlayerState::Idle | PlayerState::Error(_) => MediaPlayback::Stopped,
            };
            if let Some(Err(e)) = self.controls().map(|mut c| c.set_playback(playback)) {
                tracing::debug!("media controls: playback not updated: {e:?}");
            }
        }

        /// `cover`: address of the picture (the station's logo, or the app icon), from `covers.rs`.
        pub fn show_now_playing(
            &self,
            now_playing: &NowPlaying,
            station: Option<&str>,
            label: Option<&str>,
            cover: Option<&str>,
        ) {
            let shown = display(now_playing, station, label);
            let metadata = MediaMetadata {
                title: shown.title.as_deref(),
                artist: shown.artist.as_deref(),
                album: shown.album.as_deref(),
                cover_url: cover,
                ..Default::default()
            };
            if let Some(Err(e)) = self.controls().map(|mut c| c.set_metadata(metadata)) {
                tracing::debug!("media controls: metadata not updated: {e:?}");
            }
        }
    }

    /// A live stream cannot seek: those events are ignored.
    fn handle(app: &tauri::AppHandle, event: MediaControlEvent) {
        let command = match event {
            MediaControlEvent::Raise => return crate::show_main_window(app),
            MediaControlEvent::Quit => return app.exit(0),
            MediaControlEvent::Play => Command::Play,
            MediaControlEvent::Pause => Command::Pause,
            MediaControlEvent::Toggle => Command::Toggle,
            MediaControlEvent::Stop => Command::Stop,
            MediaControlEvent::Next => Command::Next,
            MediaControlEvent::Previous => Command::Previous,
            _ => return,
        };
        control(app, command);
    }
}

/// What the desktop's media controls and the tray menu ask the player to do.
// The tray only sends Toggle, Next and Previous: elsewhere than Linux nothing else is built.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub enum Command {
    Play,
    Pause,
    Toggle,
    Stop,
    /// Next / Previous switch between favorite stations.
    Next,
    Previous,
}

/// Runs `command`. The engine may block for a while: call it from a worker thread, not from the
/// thread that delivers D-Bus or menu events.
pub fn control(app: &tauri::AppHandle, command: Command) {
    let result = match command {
        Command::Next => switch_favorite(app, true),
        Command::Previous => switch_favorite(app, false),
        _ => drive(app, command),
    };
    if let Err(e) = result {
        tracing::warn!("media control {command:?} failed: {e}");
    }
}

fn drive(app: &tauri::AppHandle, command: Command) -> AppResult<()> {
    let Some(audio) = app.try_state::<AudioState>() else {
        return Ok(());
    };
    let mut engine = audio.engine();
    match (command, engine.state()) {
        // Pausing while connecting is not possible yet: Toggle stops instead
        (Command::Stop, _)
        | (Command::Toggle, PlayerState::Connecting | PlayerState::Reconnecting(_)) => {
            engine.stop()?
        }
        (Command::Pause | Command::Toggle, PlayerState::Playing) => engine.pause()?,
        (Command::Play | Command::Toggle, PlayerState::Paused) => engine.resume()?,
        // After a stop, plays the last station again
        (Command::Play | Command::Toggle, PlayerState::Idle | PlayerState::Error(_)) => {
            drop(engine);
            if let Some(station) = audio.station() {
                start_station(app, station.url, Some(station.id), station.name)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Plays the next (or previous) favorite, and tells the window which station is now on air.
fn switch_favorite(app: &tauri::AppHandle, forward: bool) -> AppResult<()> {
    let favorites = app.state::<Database>().list_favorites()?;
    let current = app
        .try_state::<AudioState>()
        .and_then(|audio| audio.station())
        .map(|s| s.id);
    let Some(next) = neighbour(&favorites, current.as_deref(), forward).cloned() else {
        return Ok(());
    };
    if let Some(zap) = app.try_state::<Zap>() {
        *zap.lock() = Some(Zapping {
            forward,
            station_id: next.station_id.clone(),
            tried: 1,
            skipped: Vec::new(),
        });
    }
    play_favorite(app, next, Vec::new())
}

/// Plays `next`, and tells the window which station is now on air: before the sound output opens
/// (a few hundred ms), so the new name shows at once, with *Connecting…*, while the engine clears
/// the previous title.
fn play_favorite(app: &tauri::AppHandle, next: Favorite, skipped: Vec<Skipped>) -> AppResult<()> {
    let mut change = StationChange {
        id: next.station_id.clone(),
        name: next.name.clone(),
        url: next.stream_url.clone(),
        genre: next.genre,
        show_click_notice: false,
        skipped,
    };
    emit_station(app, &change);
    let outcome = start_station(app, next.stream_url, Some(next.station_id), Some(next.name))?;
    // Known only once playing: the window shows the click-counting notice on the same station.
    if outcome.show_click_notice {
        change.show_click_notice = true;
        emit_station(app, &change);
    }
    Ok(())
}

fn emit_station(app: &tauri::AppHandle, change: &StationChange) {
    if let Err(e) = app.emit(STATION_EVENT, change) {
        tracing::warn!("failed to emit the new station: {e}");
    }
}

/// The favorite before or after `current` in the list, wrapping around. From a station that is not
/// a favorite, forward starts the first one and backward the last one.
fn neighbour<'a>(
    favorites: &'a [Favorite],
    current: Option<&str>,
    forward: bool,
) -> Option<&'a Favorite> {
    let len = favorites.len();
    if len == 0 {
        return None;
    }
    let index = match current.and_then(|id| favorites.iter().position(|f| f.station_id == id)) {
        Some(i) if forward => (i + 1) % len,
        Some(i) => (i + len - 1) % len,
        None if forward => 0,
        None => len - 1,
    };
    favorites.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_air(station: Option<&str>, title: Option<&str>) -> NowPlaying {
        NowPlaying {
            station: station.map(str::to_owned),
            title: title.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn station_comes_first_and_the_track_below() {
        let shown = display(
            &on_air(Some("icy name"), Some("Daft Punk - One More Time - FIP")),
            Some("FIP"),
            None,
        );
        assert_eq!(
            shown,
            Display {
                title: Some("FIP".into()),
                artist: Some("Daft Punk - One More Time".into()),
                album: Some("FIP".into()),
            }
        );
        // A title that cannot be split is shown as is; without a name, `icy-name` is the station
        let shown = display(&on_air(Some("icy name"), Some("Jingle")), None, None);
        assert_eq!(shown.title.as_deref(), Some("icy name"));
        assert_eq!(shown.artist.as_deref(), Some("Jingle"));
    }

    fn favorite(id: &str) -> Favorite {
        Favorite {
            station_id: id.into(),
            name: id.to_uppercase(),
            stream_url: format!("https://radio.test/{id}"),
            genre: None,
            added_at: None,
        }
    }

    #[test]
    fn favorites_wrap_around() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let id = |current, forward| {
            neighbour(&favorites, current, forward).map(|f| f.station_id.as_str())
        };
        assert_eq!(id(Some("a"), true), Some("b"));
        assert_eq!(id(Some("c"), true), Some("a"));
        assert_eq!(id(Some("a"), false), Some("c"));
        assert_eq!(id(Some("other"), true), Some("a"));
        assert_eq!(id(None, false), Some("c"));
        assert_eq!(neighbour(&[], Some("a"), true).map(|f| &f.station_id), None);
    }

    fn zapping(id: &str, forward: bool) -> Zapping {
        Zapping {
            forward,
            station_id: id.into(),
            tried: 1,
            skipped: Vec::new(),
        }
    }

    const NOT_ON_AIR: &str = "The station is not broadcasting: the stream is not on air (HTTP 404)";

    #[test]
    fn a_silent_favorite_gives_way_to_the_next_one() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let mut zap = zapping("a", true);
        let Skip::To(next, skipped) = zap.skip(&favorites, NOT_ON_AIR) else {
            panic!("not skipped");
        };
        assert_eq!(next.station_id, "b");
        assert_eq!(
            skipped,
            [Skipped {
                name: "A".into(),
                reason: "the stream is not on air (HTTP 404)".into(),
            }]
        );
        assert_eq!(zap.station_id, "b");
        // Previous goes backward
        let mut zap = zapping("a", false);
        assert!(matches!(zap.skip(&favorites, NOT_ON_AIR), Skip::To(f, _) if f.station_id == "c"));
    }

    #[test]
    fn zapping_stops_after_a_full_turn() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let mut zap = zapping("a", true);
        assert!(matches!(zap.skip(&favorites, NOT_ON_AIR), Skip::To(..)));
        let Skip::To(_, skipped) = zap.skip(&favorites, "Cannot open the sound output") else {
            panic!("not skipped");
        };
        // A message without the usual start is kept whole
        assert_eq!(skipped[1].reason, "Cannot open the sound output");
        // a, b and c tried: c's error is shown
        assert_eq!(zap.skip(&favorites, NOT_ON_AIR), Skip::Stop);
        // A single favorite is never skipped
        assert_eq!(
            zapping("a", true).skip(&[favorite("a")], NOT_ON_AIR),
            Skip::Stop
        );
    }

    #[test]
    fn the_name_given_to_the_station_is_shown() {
        // The signature is still found with the station's own name.
        let shown = display(
            &on_air(None, Some("Daft Punk - One More Time - FIP")),
            Some("FIP"),
            Some("France Inter Paris"),
        );
        assert_eq!(
            shown,
            Display {
                title: Some("France Inter Paris".into()),
                artist: Some("Daft Punk - One More Time".into()),
                album: Some("France Inter Paris".into()),
            }
        );
    }

    #[test]
    fn station_is_shown_without_a_track() {
        let expected = Display {
            title: Some("FIP".into()),
            artist: None,
            album: Some("FIP".into()),
        };
        assert_eq!(display(&on_air(None, None), Some("FIP"), None), expected);
        assert_eq!(
            display(&on_air(Some("FIP"), Some(" fip ")), None, None),
            expected
        );
        assert_eq!(
            display(&on_air(None, Some("  ")), Some("FIP"), None),
            expected
        );
    }

    #[test]
    fn track_takes_the_first_line_without_a_station() {
        let shown = display(&on_air(None, Some("Daft Punk - One More Time")), None, None);
        assert_eq!(
            shown,
            Display {
                title: Some("Daft Punk - One More Time".into()),
                artist: None,
                album: None
            }
        );
    }
}
