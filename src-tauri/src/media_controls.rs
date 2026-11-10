// SPDX-License-Identifier: MPL-2.0
//! System media controls (MPRIS on Linux, Now Playing on macOS, the System Media Transport
//! Controls on Windows): the desktop shows the station
//! and the title on air, and its play / pause / stop buttons and the keyboard media keys drive the
//! player. Next and
//! previous switch between favorite stations.
//!
//! In a radio app the station comes first (next / previous change it), so the desktop's first
//! line, the MPRIS title, is the station and its second line, the MPRIS artist, is the track.
//! The station is also the album, for desktops that show it, and its logo the picture.
//!
//! On macOS, MediaPlayer is driven from the main thread, on Windows from a thread of their own
//! (see `native::SystemMedia::apply`). Windows ties the controls to a window: a hidden one of
//! their own, since the main window comes and goes. Elsewhere [`SystemMedia`] does nothing. The
//! tray menu drives the player through [`control`] too, so that it can never behave differently
//! from the media keys.
//!
//! Next and previous skip a favorite that is proven not to broadcast before its first sound (the
//! engine only gives up at once when the station is at fault, see `audio/failure.rs`): someone
//! zapping from the tray or the media keys is not looking at the window, and silence would explain
//! nothing. Without proof (no network, no Internet) the player waits as usual; a favorite gone
//! from the catalog stops the zapping, so that the window can offer to remove it; after a full
//! turn without sound, the last error is shown. The window lists what was skipped.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::audio::{AudioEngine, NowPlaying, PlayerState};
use crate::db::{Database, Favorite};
use crate::error::AppResult;
use crate::events;
use crate::player::{start_station, AudioState};

#[derive(Debug, Clone, Serialize, specta::Type)]
pub(crate) struct StationChange {
    pub id: String,
    pub name: String,
    pub url: String,
    pub genre: Option<String>,
    /// The window must show the click-counting notice (once only).
    pub show_click_notice: bool,
    /// Favorites skipped on the way, because they did not broadcast (none when the backend
    /// retries the same station).
    pub skipped: Vec<Skipped>,
}

/// A favorite skipped by Next / Previous.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub(crate) struct Skipped {
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

/// Read from the database when Next / Previous is pressed: the player's state listener, which
/// decides the skips, runs under the player's lock and never waits on the database.
#[derive(Debug, Clone, PartialEq)]
struct Zapping {
    forward: bool,
    /// The favorites, in their order, as they were when the zapping started.
    favorites: Vec<Favorite>,
    /// Names the user gave stations, by station ID.
    labels: HashMap<String, String>,
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
    fn skip(&mut self, message: &str) -> Skip {
        if self.tried >= self.favorites.len() {
            return Skip::Stop;
        }
        let Some(next) = neighbour(&self.favorites, Some(&self.station_id), self.forward).cloned()
        else {
            return Skip::Stop;
        };
        // Named as the user knows it.
        let name = self
            .labels
            .get(&self.station_id)
            .cloned()
            .unwrap_or_else(|| {
                self.favorites
                    .iter()
                    .find(|f| f.station_id == self.station_id)
                    .map_or_else(|| self.station_id.clone(), |f| f.name.clone())
            });
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
    let Skip::To(next, skipped) = zapping.skip(message) else {
        *zap = None;
        return state;
    };
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use native::SystemMedia;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub struct SystemMedia;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
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
// Only the Linux, macOS and Windows backends show it; the tests run everywhere.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
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
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod native {
    use std::sync::{Arc, Mutex};

    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig,
    };

    use super::{control, display, Command, NowPlaying, PlayerState};

    /// An update of the controls, run on their own thread (Windows).
    #[cfg(target_os = "windows")]
    type Job = Box<dyn FnOnce() + Send>;

    pub struct SystemMedia {
        /// `None` when the controls could not be created: the player works without them.
        controls: Option<Arc<Mutex<MediaControls>>>,
        /// To reach the main thread.
        #[cfg(target_os = "macos")]
        app: tauri::AppHandle,
        /// The thread the controls are updated from, in the order of the calls. `None` if it could
        /// not start: the caller's thread does it.
        #[cfg(target_os = "windows")]
        jobs: Option<std::sync::mpsc::Sender<Job>>,
    }

    impl SystemMedia {
        /// Called from the main thread (Tauri's `setup`). On macOS, MediaPlayer's remote commands
        /// are registered there, and their handlers run there; on Windows, the hidden window the
        /// controls are tied to is served by its event loop.
        pub fn new(app: tauri::AppHandle) -> Self {
            #[cfg(target_os = "windows")]
            let hwnd = windows::hidden_window();
            #[cfg(not(target_os = "windows"))]
            let hwnd = None;
            let config = PlatformConfig {
                display_name: "Soundfolk",
                dbus_name: "soundfolk",
                hwnd,
            };
            let handler_app = app.clone();
            // souvlaki panics on Windows without a window.
            let controls = if cfg!(target_os = "windows") && hwnd.is_none() {
                Err("no window to tie them to".to_owned())
            } else {
                MediaControls::new(config)
                    .and_then(|mut controls| {
                        // Called on the D-Bus thread (Linux), the main thread (macOS) or a thread
                        // of the system's (Windows): the engine may block for a while, so it is
                        // driven from a worker thread instead.
                        controls.attach(move |event| {
                            let app = handler_app.clone();
                            tauri::async_runtime::spawn_blocking(move || handle(&app, event));
                        })?;
                        Ok(controls)
                    })
                    .map_err(|e| format!("{e:?}"))
            };
            let controls = match controls {
                Ok(controls) => Some(Arc::new(Mutex::new(controls))),
                Err(e) => {
                    tracing::warn!("system media controls unavailable: {e}");
                    None
                }
            };
            #[cfg(target_os = "windows")]
            let jobs = controls.as_ref().and_then(|_| windows::updater());
            Self {
                controls,
                #[cfg(target_os = "macos")]
                app,
                #[cfg(target_os = "windows")]
                jobs,
            }
        }

        /// Runs `update` on the controls. On macOS it runs on the main thread, in the order of the
        /// calls: MediaPlayer expects it, and the main thread's run loop drains the Objective-C
        /// objects souvlaki leaves to the autorelease pool (a worker thread has none: they would
        /// leak with every title). On Windows it runs on the controls' own thread, in the order of
        /// the calls too: souvlaki waits there for a cover file to open, which the caller (the main
        /// thread, for a command from the window) must not.
        fn apply<F>(&self, what: &'static str, update: F)
        where
            F: FnOnce(&mut MediaControls) -> Result<(), souvlaki::Error> + Send + 'static,
        {
            let Some(controls) = self.controls.clone() else {
                return;
            };
            let run = move || {
                let mut controls = controls.lock().unwrap_or_else(|p| p.into_inner());
                if let Err(e) = update(&mut controls) {
                    tracing::debug!("media controls: {what} not updated: {e:?}");
                }
            };
            #[cfg(target_os = "macos")]
            if let Err(e) = self.app.run_on_main_thread(run) {
                tracing::debug!("media controls: {what} not updated: {e}");
            }
            #[cfg(target_os = "windows")]
            match &self.jobs {
                Some(jobs) => {
                    // The thread is gone (it never stops): done here.
                    if let Err(std::sync::mpsc::SendError(run)) = jobs.send(Box::new(run)) {
                        run();
                    }
                }
                None => run(),
            }
            #[cfg(target_os = "linux")]
            run();
        }

        pub fn show_state(&self, state: &PlayerState) {
            let playback = match state {
                PlayerState::Connecting | PlayerState::Reconnecting(_) | PlayerState::Playing => {
                    MediaPlayback::Playing { progress: None }
                }
                PlayerState::Paused => MediaPlayback::Paused { progress: None },
                PlayerState::Idle | PlayerState::Error(_) => MediaPlayback::Stopped,
            };
            self.apply("playback", move |c| c.set_playback(playback));
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
            #[cfg(target_os = "windows")]
            let cover = cover.map(windows::cover_address);
            #[cfg(not(target_os = "windows"))]
            let cover = cover.map(str::to_owned);
            self.apply("metadata", move |c| {
                let metadata = |cover_url| MediaMetadata {
                    title: field(&shown.title),
                    artist: field(&shown.artist),
                    album: field(&shown.album),
                    cover_url,
                    ..Default::default()
                };
                // A picture that cannot be read (Windows opens the file there and then) must not
                // keep the station and the track from showing.
                c.set_metadata(metadata(cover.as_deref())).or_else(|e| {
                    tracing::debug!("media controls: cover not shown: {e:?}");
                    c.set_metadata(metadata(None))
                })
            });
        }
    }

    /// A field of the controls. Windows keeps a field it is not given: an empty one replaces the
    /// previous track when the station sends none.
    fn field(value: &Option<String>) -> Option<&str> {
        if cfg!(target_os = "windows") {
            Some(value.as_deref().unwrap_or(""))
        } else {
            value.as_deref()
        }
    }

    /// A live stream cannot seek: those events are ignored. macOS sends no Raise, Quit or Stop,
    /// Windows no Raise or Quit.
    fn handle(app: &tauri::AppHandle, event: MediaControlEvent) {
        let command = match event {
            MediaControlEvent::Raise => return crate::windows::show_main_window(app),
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

    /// The System Media Transport Controls' needs: a window, a thread, and file paths as Windows
    /// writes them.
    #[cfg(target_os = "windows")]
    mod windows {
        use std::ffi::c_void;

        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, WNDCLASSW, WS_EX_TOOLWINDOW,
            WS_OVERLAPPED,
        };

        use super::Job;

        /// A window that is never shown, for the controls to be tied to: the main window is gone
        /// once closed, and not there at all after a launch hidden in the notification area. Made
        /// on the main thread, whose event loop serves it, for the life of the app.
        pub(super) fn hidden_window() -> Option<*mut c_void> {
            let class = wide("SoundfolkMediaControls");
            let title = wide("Soundfolk");
            // SAFETY: a null name asks for the running executable's own module, which needs no
            // freeing.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // SAFETY: `window_class` and the name it points to outlive the call, which copies
            // them. Registered once: `new` is called once.
            if unsafe { RegisterClassW(&window_class) } == 0 {
                tracing::warn!(
                    "media controls: window class not registered: {}",
                    std::io::Error::last_os_error()
                );
                return None;
            }
            // SAFETY: the class was registered above with this module; the names outlive the
            // call; no parent, menu or creation data. The window is never destroyed.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            if hwnd.is_null() {
                tracing::warn!(
                    "media controls: no window: {}",
                    std::io::Error::last_os_error()
                );
                return None;
            }
            Some(hwnd)
        }

        /// The thread that updates the controls, one job after the other.
        pub(super) fn updater() -> Option<std::sync::mpsc::Sender<Job>> {
            let (jobs, queue) = std::sync::mpsc::channel::<Job>();
            let started = std::thread::Builder::new()
                .name("media-controls".into())
                .spawn(move || {
                    // SAFETY: called once, first thing on this thread; the thread stays in the
                    // multithreaded apartment for its whole life, so it never uninitializes.
                    let joined =
                        unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
                    if joined < 0 {
                        tracing::debug!("media controls: COM not initialized ({joined:#x})");
                    }
                    for job in queue {
                        job();
                    }
                });
            match started {
                Ok(_) => Some(jobs),
                Err(e) => {
                    tracing::warn!("media controls: no thread of their own: {e}");
                    None
                }
            }
        }

        /// souvlaki opens a `file://` address as the path written after `file://`, as is:
        /// `file:///C:/Users/…` (the usual form) would be `/C:/Users/…`, which Windows refuses.
        pub(super) fn cover_address(cover: &str) -> String {
            url::Url::parse(cover)
                .ok()
                .filter(|address| address.scheme() == "file")
                .and_then(|address| address.to_file_path().ok())
                .map_or_else(
                    || cover.to_owned(),
                    |path| format!("file://{}", path.display()),
                )
        }

        /// A string as Windows takes it: UTF-16, ending with a zero.
        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(std::iter::once(0)).collect()
        }

        #[cfg(test)]
        mod tests {
            use super::cover_address;

            #[test]
            fn file_addresses_become_windows_paths() {
                assert_eq!(
                    cover_address("file:///C:/Users/Ana%20B/AppData/soundfolk.png"),
                    r"file://C:\Users\Ana B\AppData\soundfolk.png"
                );
                // A logo or a cover on the web is left as is
                assert_eq!(
                    cover_address("https://radio.test/logo.png"),
                    "https://radio.test/logo.png"
                );
            }
        }
    }
}

/// What the desktop's media controls and the tray menu ask the player to do.
// The tray only sends Toggle, Next and Previous: elsewhere than Linux, macOS and Windows nothing
// else is built.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
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
/// thread that delivers D-Bus, Now Playing or menu events.
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
    let db = app.state::<Database>();
    let favorites = db.list_favorites()?;
    let current = app
        .try_state::<AudioState>()
        .and_then(|audio| audio.station())
        .map(|s| s.id);
    let Some(next) = neighbour(&favorites, current.as_deref(), forward).cloned() else {
        return Ok(());
    };
    if let Some(zap) = app.try_state::<Zap>() {
        let labels = db.station_names().unwrap_or_else(|e| {
            tracing::warn!("station names not read: {e}");
            HashMap::new()
        });
        *zap.lock() = Some(Zapping {
            forward,
            favorites,
            labels,
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
    if let Err(e) = app.emit(events::STATION, change) {
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

    fn zapping(favorites: &[Favorite], id: &str, forward: bool) -> Zapping {
        Zapping {
            forward,
            favorites: favorites.to_vec(),
            labels: HashMap::new(),
            station_id: id.into(),
            tried: 1,
            skipped: Vec::new(),
        }
    }

    const NOT_ON_AIR: &str = "The station is not broadcasting: the stream is not on air (HTTP 404)";

    #[test]
    fn a_silent_favorite_gives_way_to_the_next_one() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let mut zap = zapping(&favorites, "a", true);
        let Skip::To(next, skipped) = zap.skip(NOT_ON_AIR) else {
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
        let mut zap = zapping(&favorites, "a", false);
        assert!(matches!(zap.skip(NOT_ON_AIR), Skip::To(f, _) if f.station_id == "c"));
    }

    #[test]
    fn zapping_stops_after_a_full_turn() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let mut zap = zapping(&favorites, "a", true);
        assert!(matches!(zap.skip(NOT_ON_AIR), Skip::To(..)));
        let Skip::To(_, skipped) = zap.skip("Cannot open the sound output") else {
            panic!("not skipped");
        };
        // A message without the usual start is kept whole
        assert_eq!(skipped[1].reason, "Cannot open the sound output");
        // a, b and c tried: c's error is shown
        assert_eq!(zap.skip(NOT_ON_AIR), Skip::Stop);
        // A single favorite is never skipped
        assert_eq!(
            zapping(&[favorite("a")], "a", true).skip(NOT_ON_AIR),
            Skip::Stop
        );
    }

    #[test]
    fn a_skipped_favorite_is_named_as_the_user_knows_it() {
        let favorites = [favorite("a"), favorite("b"), favorite("c")];
        let mut zap = zapping(&favorites, "a", true);
        zap.labels.insert("a".into(), "My A".into());
        let Skip::To(_, skipped) = zap.skip(NOT_ON_AIR) else {
            panic!("not skipped");
        };
        assert_eq!(skipped[0].name, "My A");
        // Without a name of the user's, the favorite's own.
        let Skip::To(_, skipped) = zap.skip(NOT_ON_AIR) else {
            panic!("not skipped");
        };
        assert_eq!(skipped[1].name, "B");
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
