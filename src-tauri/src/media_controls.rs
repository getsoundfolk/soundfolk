// SPDX-License-Identifier: MPL-2.0
//! System media controls (MPRIS on Linux): the desktop shows the station and the title on air,
//! and its play / pause / stop buttons and the keyboard media keys drive the player. Next and
//! previous switch between favorite stations.
//!
//! In a radio app the station comes first (next / previous change it), so the desktop's first
//! line, the MPRIS title, is the station and its second line, the MPRIS artist, is the track.
//! The station is also the album, for desktops that show it, and the track's cover art the picture.
//!
//! Linux only for now: Windows needs the window's HWND and macOS the main thread's run loop.
//! Elsewhere [`SystemMedia`] does nothing.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use serde::Serialize;

use crate::audio::{NowPlaying, PlayerState};
use crate::db::Favorite;

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

    pub fn show_now_playing(&self, _now_playing: &NowPlaying, _station: Option<&str>, _cover: Option<&str>) {}
}

/// What the desktop displays: the station as the title, the track on air as the artist.
#[derive(Debug, PartialEq)]
struct Display {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

/// `station` is the name the user knows the station by; the stream's `icy-name` is the fallback.
/// A title that is only the station name (sent by some radios between tracks) is not a track.
/// The track is rebuilt as `Artist - Title`, without the station signature some radios append.
fn display(now_playing: &NowPlaying, station: Option<&str>) -> Display {
    let station = station.or(now_playing.station.as_deref()).map(str::trim).filter(|s| !s.is_empty());
    let track = now_playing
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !station.is_some_and(|s| s.eq_ignore_ascii_case(t)))
        .map(|raw| {
            // Given apart by the source: shown as is; otherwise parsed from the ICY title.
            let parsed = match &now_playing.track {
                Some(track) => crate::tracks::ParsedTitle { artist: track.artist.clone(), title: track.title.clone() },
                None => crate::tracks::parse(raw, station),
            };
            match parsed.artist {
                Some(artist) => format!("{artist} - {}", parsed.title),
                None => parsed.title,
            }
        });
    match station {
        Some(station) => Display { title: Some(station.to_owned()), artist: track, album: Some(station.to_owned()) },
        // Station unknown: the track takes the first line
        None => Display { title: track, artist: None, album: None },
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{Mutex, MutexGuard};

    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
    use tauri::{Emitter, Manager};

    use super::{display, neighbour, NowPlaying, PlayerState, StationChange, STATION_EVENT};
    use crate::audio::AudioEngine;
    use crate::commands::{start_station, AudioState};
    use crate::db::Database;
    use crate::error::AppResult;

    pub struct SystemMedia {
        /// `None` when the controls could not be created: the player works without them.
        controls: Option<Mutex<MediaControls>>,
    }

    impl SystemMedia {
        pub fn new(app: tauri::AppHandle) -> Self {
            let config =
                PlatformConfig { display_name: "Soundfolk", dbus_name: "soundfolk", hwnd: None };
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
                Ok(controls) => Self { controls: Some(Mutex::new(controls)) },
                Err(e) => {
                    tracing::warn!("system media controls unavailable: {e:?}");
                    Self { controls: None }
                }
            }
        }

        fn controls(&self) -> Option<MutexGuard<'_, MediaControls>> {
            self.controls.as_ref().map(|c| c.lock().unwrap_or_else(|p| p.into_inner()))
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

        /// `cover`: address of the track's cover art, as resolved by `covers.rs`.
        pub fn show_now_playing(&self, now_playing: &NowPlaying, station: Option<&str>, cover: Option<&str>) {
            let shown = display(now_playing, station);
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

    /// Next / Previous switch between favorite stations. A live stream cannot seek: those events
    /// are ignored.
    fn handle(app: &tauri::AppHandle, event: MediaControlEvent) {
        let result = match event {
            MediaControlEvent::Raise => {
                crate::show_main_window(app);
                Ok(())
            }
            MediaControlEvent::Quit => {
                app.exit(0);
                Ok(())
            }
            MediaControlEvent::Next => switch_favorite(app, true),
            MediaControlEvent::Previous => switch_favorite(app, false),
            MediaControlEvent::Play | MediaControlEvent::Pause | MediaControlEvent::Toggle | MediaControlEvent::Stop => {
                drive(app, &event)
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            tracing::warn!("media control {event:?} failed: {e}");
        }
    }

    fn drive(app: &tauri::AppHandle, event: &MediaControlEvent) -> AppResult<()> {
        let Some(audio) = app.try_state::<AudioState>() else { return Ok(()) };
        let mut engine = audio.engine();
        match (event, engine.state()) {
            // Pausing while connecting is not possible yet: Toggle stops instead
            (MediaControlEvent::Stop, _)
            | (MediaControlEvent::Toggle, PlayerState::Connecting | PlayerState::Reconnecting(_)) => engine.stop()?,
            (MediaControlEvent::Pause | MediaControlEvent::Toggle, PlayerState::Playing) => engine.pause()?,
            (MediaControlEvent::Play | MediaControlEvent::Toggle, PlayerState::Paused) => engine.resume()?,
            // After a stop, plays the last station again
            (MediaControlEvent::Play | MediaControlEvent::Toggle, PlayerState::Idle | PlayerState::Error(_)) => {
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
        let current = app.try_state::<AudioState>().and_then(|audio| audio.station()).map(|s| s.id);
        let Some(next) = neighbour(&favorites, current.as_deref(), forward).cloned() else {
            return Ok(());
        };
        let outcome =
            start_station(app, next.stream_url.clone(), Some(next.station_id.clone()), Some(next.name.clone()))?;
        let change = StationChange {
            id: next.station_id,
            name: next.name,
            url: next.stream_url,
            genre: next.genre,
            show_click_notice: outcome.show_click_notice,
        };
        if let Err(e) = app.emit(STATION_EVENT, change) {
            tracing::warn!("failed to emit the new station: {e}");
        }
        Ok(())
    }
}

/// The favorite before or after `current` in the list, wrapping around. From a station that is not
/// a favorite, forward starts the first one and backward the last one.
fn neighbour<'a>(favorites: &'a [Favorite], current: Option<&str>, forward: bool) -> Option<&'a Favorite> {
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
        NowPlaying { station: station.map(str::to_owned), title: title.map(str::to_owned), ..Default::default() }
    }

    #[test]
    fn station_comes_first_and_the_track_below() {
        let shown = display(&on_air(Some("icy name"), Some("Daft Punk - One More Time - FIP")), Some("FIP"));
        assert_eq!(
            shown,
            Display {
                title: Some("FIP".into()),
                artist: Some("Daft Punk - One More Time".into()),
                album: Some("FIP".into()),
            }
        );
        // A title that cannot be split is shown as is; without a name, `icy-name` is the station
        let shown = display(&on_air(Some("icy name"), Some("Jingle")), None);
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
        let id = |current, forward| neighbour(&favorites, current, forward).map(|f| f.station_id.as_str());
        assert_eq!(id(Some("a"), true), Some("b"));
        assert_eq!(id(Some("c"), true), Some("a"));
        assert_eq!(id(Some("a"), false), Some("c"));
        assert_eq!(id(Some("other"), true), Some("a"));
        assert_eq!(id(None, false), Some("c"));
        assert_eq!(neighbour(&[], Some("a"), true).map(|f| &f.station_id), None);
    }

    #[test]
    fn station_is_shown_without_a_track() {
        let expected = Display { title: Some("FIP".into()), artist: None, album: Some("FIP".into()) };
        assert_eq!(display(&on_air(None, None), Some("FIP")), expected);
        assert_eq!(display(&on_air(Some("FIP"), Some(" fip ")), None), expected);
        assert_eq!(display(&on_air(None, Some("  ")), Some("FIP")), expected);
    }

    #[test]
    fn track_takes_the_first_line_without_a_station() {
        let shown = display(&on_air(None, Some("Daft Punk - One More Time")), None);
        assert_eq!(shown, Display { title: Some("Daft Punk - One More Time".into()), artist: None, album: None });
    }
}
