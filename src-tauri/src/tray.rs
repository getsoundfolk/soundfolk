// SPDX-License-Identifier: MPL-2.0
//! Notification area icon: the app keeps playing in the background once its window is closed,
//! so this is the way back to the window, a remote for the player, and the way to really quit.
//!
//! Two backends, because a click on the icon never reaches the app on Linux otherwise: Tauri's
//! icon goes through libappindicator, which declares the item a menu and nothing else
//! (`ItemIsMenu`), so every click opens the menu and no `TrayIconEvent` ever fires. On Linux,
//! [`ksni`] speaks StatusNotifierItem itself, as the desktops expect: a left click shows or hides
//! the window, a middle click plays or pauses, a right click opens the menu. Windows and macOS
//! keep Tauri's icon.
//!
//! "The window" is the one used last, main or mini player (`windows.rs`): only one is up at a
//! time, so Hide needs no name. Items that would do nothing are greyed out: Play before any
//! station was chosen, Previous and Next with fewer than two favorites.
//!
//! The menu's actions go through the same [`control`] as the desktop's media keys (MPRIS), so the
//! two can never behave differently.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tauri::AppHandle;

use crate::audio::PlayerState;
use crate::media_controls::{control, Command};

pub use imp::init;

/// Whether an icon actually exists for the user to click.
///
/// On Linux, a panel that only knows the older XEmbed tray (i3bar, dwm, xfce4-panel without its
/// StatusNotifier plugin) runs no `StatusNotifierWatcher`: no icon ever shows. Closing the window
/// would then leave the app playing with no way back, so the app quits instead (see `lib.rs`).
/// Optimistic: cleared once the Linux backend knows there is no watcher, or when `init` fails.
static AVAILABLE: AtomicBool = AtomicBool::new(true);

/// Whether the player is playing, as the menu shows it (Play or Pause).
static PLAYING: AtomicBool = AtomicBool::new(false);

/// Whether the sound is muted, as the menu shows it (Mute or Unmute).
static MUTED: AtomicBool = AtomicBool::new(false);

/// Whether a window (main or mini player) is shown, as the menu shows it (Hide or Show). One
/// opens at launch, unless the app starts hidden (`lib.rs`).
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(true);

/// Whether the player has a station, as the menu shows it (Play greyed out without one). Never
/// lost once there: the player keeps the last station.
static HAS_STATION: AtomicBool = AtomicBool::new(false);

/// How many favorites there are: Previous and Next need two to switch between.
static FAVORITES: AtomicUsize = AtomicUsize::new(0);

/// Can the window be brought back from the notification area? See [`AVAILABLE`].
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

pub fn set_available(available: bool) {
    AVAILABLE.store(available, Ordering::Relaxed);
}

/// No icon to come back from: after a launch hidden in the notification area, the window opens
/// after all, or the app would play on with no way back to it.
#[cfg(target_os = "linux")]
pub fn set_unavailable(app: &AppHandle) {
    use tauri::Manager;

    set_available(false);
    if app
        .get_webview_window(crate::windows::MAIN_WINDOW)
        .is_none()
    {
        crate::windows::show_main_window(app);
    }
}

/// Follows the player, so that the menu offers Pause while it plays and Play otherwise. Any
/// state but `Idle` means a station was chosen.
pub fn show_state(app: &AppHandle, state: &PlayerState) {
    let playing = matches!(
        state,
        PlayerState::Playing | PlayerState::Connecting | PlayerState::Reconnecting(_)
    );
    let station = !matches!(state, PlayerState::Idle);
    let changed = PLAYING.swap(playing, Ordering::Relaxed) != playing;
    let found = station && !HAS_STATION.swap(true, Ordering::Relaxed);
    if changed || found {
        imp::refresh(app);
    }
}

/// At launch: whether the player starts with a station (the last one played), and how many
/// favorites there are. Before [`init`].
pub fn show_launch_state(has_station: bool, favorites: usize) {
    HAS_STATION.store(has_station, Ordering::Relaxed);
    FAVORITES.store(favorites, Ordering::Relaxed);
}

/// Follows the favorites, so that Previous and Next are greyed out with fewer than two.
pub fn show_favorites(app: &AppHandle, count: usize) {
    if FAVORITES.swap(count, Ordering::Relaxed) != count {
        imp::refresh(app);
    }
}

/// Follows the mute, so that the menu offers Unmute while the sound is cut, Mute otherwise.
pub fn show_muted(app: &AppHandle, muted: bool) {
    if MUTED.swap(muted, Ordering::Relaxed) != muted {
        imp::refresh(app);
    }
}

/// Follows the windows, so that the menu offers to hide the one shown, to show the last one used
/// otherwise (closed or minimized).
pub fn show_window_state(app: &AppHandle, shown: bool) {
    if WINDOW_SHOWN.swap(shown, Ordering::Relaxed) != shown {
        imp::refresh(app);
    }
}

/// What the menu shows, read from the atomics above.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MenuState {
    window_shown: bool,
    playing: bool,
    muted: bool,
    has_station: bool,
    favorites: usize,
}

impl MenuState {
    fn now() -> Self {
        Self {
            window_shown: WINDOW_SHOWN.load(Ordering::Relaxed),
            playing: PLAYING.load(Ordering::Relaxed),
            muted: MUTED.load(Ordering::Relaxed),
            has_station: HAS_STATION.load(Ordering::Relaxed),
            favorites: FAVORITES.load(Ordering::Relaxed),
        }
    }
}

/// An item of the menu. Its id is the contract between the backends and [`handle_menu`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    id: &'static str,
    label: &'static str,
    enabled: bool,
}

/// The menu, top to bottom, `None` for a separator. Always the same items, so that the backends
/// only change labels and greying.
fn entries(state: MenuState) -> [Option<Entry>; 8] {
    let entry = |id, label, enabled| Some(Entry { id, label, enabled });
    let can_switch = state.favorites >= 2;
    [
        entry("toggle", toggle_label(state.window_shown), true),
        None,
        entry(
            "play_pause",
            play_pause_label(state.playing),
            state.playing || state.has_station,
        ),
        entry("previous", "Previous favorite", can_switch),
        entry("next", "Next favorite", can_switch),
        entry("mute", mute_label(state.muted), true),
        None,
        entry("quit", "Quit", true),
    ]
}

fn play_pause_label(playing: bool) -> &'static str {
    if playing {
        "Pause"
    } else {
        "Play"
    }
}

fn mute_label(muted: bool) -> &'static str {
    if muted {
        "Unmute"
    } else {
        "Mute"
    }
}

fn toggle_label(window_shown: bool) -> &'static str {
    if window_shown {
        "Hide"
    } else {
        "Show"
    }
}

/// Shared by both backends.
fn handle_menu(app: &AppHandle, id: &str) {
    let command = match id {
        "toggle" => return crate::windows::toggle_window(app),
        "quit" => return app.exit(0),
        "mute" => {
            let app = app.clone();
            crate::state::spawn_blocking(move || crate::volume::toggle_mute(&app));
            return;
        }
        "play_pause" => Command::Toggle,
        "next" => Command::Next,
        "previous" => Command::Previous,
        _ => return,
    };
    // The engine may block for a while: not on the thread that delivers the menu events
    let app = app.clone();
    crate::state::spawn_blocking(move || control(&app, command));
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;

    use ksni::menu::{MenuItem, StandardItem};
    use ksni::{Handle, Icon, OfflineReason, ToolTip, Tray, TrayMethods};
    use tauri::AppHandle;

    use super::{entries, handle_menu, set_available, set_unavailable, MenuState, WINDOW_SHOWN};

    /// Set once the icon is registered; `Handle` is not `Clone`, so it lives here.
    static HANDLE: OnceLock<Handle<SoundfolkTray>> = OnceLock::new();

    /// The menu is built from [`MenuState::now`] each time it is asked for: the tray holds no
    /// state of its own.
    struct SoundfolkTray {
        app: AppHandle,
        icon: Vec<Icon>,
        /// The icon as a PNG file ([`write_icon`]), empty if it could not be written.
        icon_file: String,
    }

    impl Tray for SoundfolkTray {
        fn id(&self) -> String {
            "soundfolk".into()
        }

        fn title(&self) -> String {
            "Soundfolk".into()
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip {
                title: "Soundfolk".into(),
                ..Default::default()
            }
        }

        /// The icon as a file, by its full path, as libappindicator gives it. Some hosts read
        /// nothing else: ayatana-indicator-application (the Xfce, MATE and Budgie "Indicator"
        /// panels) ignores the pixmap and shows a missing-icon sign for an item without a name.
        /// Not a name looked up in an `IconThemePath`: that panel then draws the PNG at its
        /// own 256 px and shows only its middle, an orange square, where a path is scaled.
        fn icon_name(&self) -> String {
            self.icon_file.clone()
        }

        /// The icon in pixels, for the hosts that read no file (or when it could not be written).
        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon.clone()
        }

        /// Left click.
        fn activate(&mut self, _x: i32, _y: i32) {
            crate::windows::toggle_window(&self.app);
        }

        /// Middle click.
        fn secondary_activate(&mut self, _x: i32, _y: i32) {
            handle_menu(&self.app, "play_pause");
        }

        /// A window event may have been missed (a minimize the window manager did not report):
        /// the label is checked again as the menu opens. Some panels show the menu before the
        /// update arrives, hence the window events too.
        fn menu_about_to_show(&mut self) {
            WINDOW_SHOWN.store(crate::windows::window_shown(&self.app), Ordering::Relaxed);
        }

        fn watcher_online(&self) {
            set_available(true);
        }

        /// No watcher on the bus: the icon shows nowhere. Returning `true` keeps the service
        /// running, so that a watcher that starts later (a restarted panel, `snixembed`) still gets
        /// the icon, and `watcher_online` makes it available again.
        fn watcher_offline(&self, reason: OfflineReason) -> bool {
            tracing::warn!(
                "tray: no StatusNotifierWatcher ({reason:?}), closing the window will quit"
            );
            set_unavailable(&self.app);
            true
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            entries(MenuState::now())
                .into_iter()
                .map(|entry| match entry {
                    Some(entry) => MenuItem::from(StandardItem {
                        label: entry.label.into(),
                        enabled: entry.enabled,
                        activate: Box::new(move |tray: &mut Self| handle_menu(&tray.app, entry.id)),
                        ..Default::default()
                    }),
                    None => MenuItem::Separator,
                })
                .collect()
        }
    }
    /// Writes the app icon to a file (once) and returns its path, empty if it could not be
    /// written.
    fn write_icon(app: &AppHandle) -> String {
        use tauri::Manager;

        let Ok(dir) = app.path().app_cache_dir() else {
            return String::new();
        };
        let path = dir.join("tray-icon.png");
        let icon = crate::covers::APP_ICON;
        if !std::fs::read(&path).is_ok_and(|bytes| bytes == icon) {
            let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, icon));
            if let Err(e) = written {
                tracing::warn!("tray: icon not written, shown by its pixels only: {e}");
                return String::new();
            }
        }
        path.to_str().map(str::to_owned).unwrap_or_default()
    }

    /// Tauri gives RGBA; StatusNotifierItem wants ARGB32 in network byte order.
    pub(super) fn pixmap(image: &tauri::image::Image<'_>) -> Vec<Icon> {
        let mut data = image.rgba().to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel.rotate_right(1);
        }
        vec![Icon {
            width: image.width() as i32,
            height: image.height() as i32,
            data,
        }]
    }

    pub fn init(app: &tauri::App) -> tauri::Result<()> {
        let icon = app.default_window_icon().map(pixmap).unwrap_or_default();
        let icon_file = write_icon(app.handle());
        let tray = SoundfolkTray {
            app: app.handle().clone(),
            icon,
            icon_file,
        };
        let handle = app.handle().clone();
        // Registering is asynchronous: a failure costs the icon, not the app, so it is logged.
        // `assume_sni_available`: no watcher yet becomes a `watcher_offline` call rather than an
        // error, which keeps the service waiting for one.
        tauri::async_runtime::spawn(async move {
            match tray.assume_sni_available(true).spawn().await {
                Ok(handle) => {
                    let _ = HANDLE.set(handle);
                }
                Err(e) => {
                    tracing::warn!("tray: StatusNotifierItem not registered: {e}");
                    set_unavailable(&handle);
                }
            }
        });
        Ok(())
    }

    /// The menu is read again from the atomics: an empty update is enough for ksni to compare it
    /// with the one shown and send the change.
    pub fn refresh(_app: &AppHandle) {
        let Some(handle) = HANDLE.get() else { return };
        tauri::async_runtime::spawn(async move {
            handle.update(|_| {}).await;
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Manager, Wry};

    use super::{entries, handle_menu, MenuState};

    /// The menu's items, by id, to change their label and greying.
    struct Items(Vec<(&'static str, MenuItem<Wry>)>);

    pub fn init(app: &tauri::App) -> tauri::Result<()> {
        let menu = Menu::new(app)?;
        let mut items = Vec::new();
        for entry in entries(MenuState::now()) {
            match entry {
                Some(entry) => {
                    let item =
                        MenuItem::with_id(app, entry.id, entry.label, entry.enabled, None::<&str>)?;
                    menu.append(&item)?;
                    items.push((entry.id, item));
                }
                None => menu.append(&PredefinedMenuItem::separator(app)?)?,
            }
        }
        let mut tray = TrayIconBuilder::with_id("main")
            .tooltip("Soundfolk")
            .menu(&menu)
            // The menu on a right click only: a left click shows or hides the window
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    crate::windows::toggle_window(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app)?;
        app.manage(Items(items));
        Ok(())
    }

    /// Sets every label and greying from [`MenuState::now`].
    pub fn refresh(app: &AppHandle) {
        let Some(items) = app.try_state::<Items>() else {
            return;
        };
        for entry in entries(MenuState::now()).into_iter().flatten() {
            let Some((_, item)) = items.0.iter().find(|(id, _)| *id == entry.id) else {
                continue;
            };
            if let Err(e) = item
                .set_text(entry.label)
                .and_then(|()| item.set_enabled(entry.enabled))
            {
                tracing::debug!("tray: {} not updated: {e}", entry.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(state: MenuState) -> Vec<Option<(&'static str, bool)>> {
        entries(state)
            .into_iter()
            .map(|e| e.map(|e| (e.label, e.enabled)))
            .collect()
    }

    #[test]
    fn menu_says_what_a_click_does() {
        let state = MenuState {
            window_shown: true,
            playing: true,
            muted: true,
            has_station: true,
            favorites: 3,
        };
        assert_eq!(
            labels(state),
            [
                Some(("Hide", true)),
                None,
                Some(("Pause", true)),
                Some(("Previous favorite", true)),
                Some(("Next favorite", true)),
                Some(("Unmute", true)),
                None,
                Some(("Quit", true)),
            ]
        );
        let hidden = labels(MenuState {
            window_shown: false,
            playing: false,
            muted: false,
            ..state
        });
        assert_eq!(hidden[0], Some(("Show", true)));
        assert_eq!(hidden[2], Some(("Play", true)));
        assert_eq!(hidden[5], Some(("Mute", true)));
    }

    #[test]
    fn items_that_would_do_nothing_are_greyed_out() {
        // First launch: no station yet, no favorites.
        let empty = labels(MenuState::default());
        assert_eq!(empty[2], Some(("Play", false)));
        assert_eq!(empty[3], Some(("Previous favorite", false)));
        assert_eq!(empty[4], Some(("Next favorite", false)));
        // One favorite: nothing to switch to.
        let one = labels(MenuState {
            has_station: true,
            favorites: 1,
            ..MenuState::default()
        });
        assert_eq!(one[2], Some(("Play", true)));
        assert_eq!(one[4], Some(("Next favorite", false)));
        assert_eq!(
            labels(MenuState {
                favorites: 2,
                ..MenuState::default()
            })[4],
            Some(("Next favorite", true))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn icon_pixels_go_from_rgba_to_argb() {
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
        let icons = imp::pixmap(&tauri::image::Image::new(&rgba, 2, 1));
        assert_eq!((icons[0].width, icons[0].height), (2, 1));
        assert_eq!(icons[0].data, [4, 1, 2, 3, 8, 5, 6, 7]);
    }
}
