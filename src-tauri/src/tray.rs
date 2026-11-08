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
//! The menu's actions go through the same [`control`] as the desktop's media keys (MPRIS), so the
//! two can never behave differently.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

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

/// Whether the main window is shown, as the menu shows it (Hide or Show). It opens at launch,
/// unless the app starts hidden (`lib.rs`).
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(true);

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
    set_available(false);
    if app.get_webview_window(crate::MAIN_WINDOW).is_none() {
        crate::show_main_window(app);
    }
}

/// Follows the player, so that the menu offers Pause while it plays and Play otherwise.
pub fn show_state(app: &AppHandle, state: &PlayerState) {
    let playing = matches!(
        state,
        PlayerState::Playing | PlayerState::Connecting | PlayerState::Reconnecting(_)
    );
    if PLAYING.swap(playing, Ordering::Relaxed) != playing {
        imp::set_playing(app, playing);
    }
}

/// Follows the mute, so that the menu offers Unmute while the sound is cut, Mute otherwise.
pub fn show_muted(app: &AppHandle, muted: bool) {
    if MUTED.swap(muted, Ordering::Relaxed) != muted {
        imp::set_muted(app, muted);
    }
}

/// Follows the main window, so that the menu offers to hide it while it is shown, to show it
/// otherwise (closed or minimized).
pub fn show_window_state(app: &AppHandle, shown: bool) {
    if WINDOW_SHOWN.swap(shown, Ordering::Relaxed) != shown {
        imp::set_window_shown(app, shown);
    }
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
        "Hide Soundfolk"
    } else {
        "Show Soundfolk"
    }
}

/// Shared by both backends: the menu ids are the contract between them.
fn handle_menu(app: &AppHandle, id: &str) {
    let command = match id {
        "toggle" => return crate::toggle_main_window(app),
        "mini" => return crate::mini::show(app),
        "quit" => return app.exit(0),
        "mute" => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || crate::volume::toggle_mute(&app));
            return;
        }
        "play_pause" => Command::Toggle,
        "next" => Command::Next,
        "previous" => Command::Previous,
        _ => return,
    };
    // The engine may block for a while: not on the thread that delivers the menu events
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || control(&app, command));
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;

    use ksni::menu::{MenuItem, StandardItem};
    use ksni::{Handle, Icon, OfflineReason, ToolTip, Tray, TrayMethods};
    use tauri::AppHandle;

    use super::{
        handle_menu, mute_label, play_pause_label, set_available, set_unavailable, toggle_label,
        MUTED, WINDOW_SHOWN,
    };

    /// Set once the icon is registered; `Handle` is not `Clone`, so it lives here.
    static HANDLE: OnceLock<Handle<SoundfolkTray>> = OnceLock::new();

    struct SoundfolkTray {
        app: AppHandle,
        playing: bool,
        muted: bool,
        window_shown: bool,
        icon: Vec<Icon>,
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

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon.clone()
        }

        /// Left click.
        fn activate(&mut self, _x: i32, _y: i32) {
            crate::toggle_main_window(&self.app);
        }

        /// Middle click.
        fn secondary_activate(&mut self, _x: i32, _y: i32) {
            handle_menu(&self.app, "play_pause");
        }

        /// A window event may have been missed (a minimize the window manager did not report):
        /// the label is checked again as the menu opens. Some panels show the menu before the
        /// update arrives, hence the window events too.
        fn menu_about_to_show(&mut self) {
            self.window_shown = crate::main_window_shown(&self.app);
            WINDOW_SHOWN.store(self.window_shown, Ordering::Relaxed);
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
            let item = |label: &str, id: &'static str| {
                MenuItem::from(StandardItem {
                    label: label.into(),
                    activate: Box::new(move |tray: &mut Self| handle_menu(&tray.app, id)),
                    ..Default::default()
                })
            };
            vec![
                item(toggle_label(self.window_shown), "toggle"),
                item("Mini player", "mini"),
                MenuItem::Separator,
                item(play_pause_label(self.playing), "play_pause"),
                item(mute_label(self.muted), "mute"),
                item("Next favorite", "next"),
                item("Previous favorite", "previous"),
                MenuItem::Separator,
                item("Quit", "quit"),
            ]
        }
    }

    /// Tauri gives RGBA; StatusNotifierItem wants ARGB32 in network byte order.
    fn pixmap(image: &tauri::image::Image<'_>) -> Vec<Icon> {
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
        let window_shown = WINDOW_SHOWN.load(Ordering::Relaxed);
        let tray = SoundfolkTray {
            app: app.handle().clone(),
            playing: false,
            muted: MUTED.load(Ordering::Relaxed),
            window_shown,
            icon,
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

    pub fn set_playing(_app: &AppHandle, playing: bool) {
        let Some(handle) = HANDLE.get() else { return };
        tauri::async_runtime::spawn(async move {
            handle.update(|tray| tray.playing = playing).await;
        });
    }

    pub fn set_muted(_app: &AppHandle, muted: bool) {
        let Some(handle) = HANDLE.get() else { return };
        tauri::async_runtime::spawn(async move {
            handle.update(|tray| tray.muted = muted).await;
        });
    }

    pub fn set_window_shown(_app: &AppHandle, shown: bool) {
        let Some(handle) = HANDLE.get() else { return };
        tauri::async_runtime::spawn(async move {
            handle.update(|tray| tray.window_shown = shown).await;
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Manager, Wry};

    use super::{handle_menu, mute_label, play_pause_label, toggle_label, MUTED, WINDOW_SHOWN};

    /// The items whose label follows the window and the player.
    struct Labels {
        toggle: MenuItem<Wry>,
        play_pause: MenuItem<Wry>,
        mute: MenuItem<Wry>,
    }

    pub fn init(app: &tauri::App) -> tauri::Result<()> {
        let shown = WINDOW_SHOWN.load(std::sync::atomic::Ordering::Relaxed);
        let toggle = MenuItem::with_id(app, "toggle", toggle_label(shown), true, None::<&str>)?;
        let mini = MenuItem::with_id(app, "mini", "Mini player", true, None::<&str>)?;
        let play_pause = MenuItem::with_id(
            app,
            "play_pause",
            play_pause_label(false),
            true,
            None::<&str>,
        )?;
        let muted = MUTED.load(std::sync::atomic::Ordering::Relaxed);
        let mute = MenuItem::with_id(app, "mute", mute_label(muted), true, None::<&str>)?;
        let next = MenuItem::with_id(app, "next", "Next favorite", true, None::<&str>)?;
        let previous = MenuItem::with_id(app, "previous", "Previous favorite", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &toggle,
                &mini,
                &PredefinedMenuItem::separator(app)?,
                &play_pause,
                &mute,
                &next,
                &previous,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
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
                    crate::toggle_main_window(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app)?;
        app.manage(Labels {
            toggle,
            play_pause,
            mute,
        });
        Ok(())
    }

    pub fn set_playing(app: &AppHandle, playing: bool) {
        if let Some(labels) = app.try_state::<Labels>()
            && let Err(e) = labels.play_pause.set_text(play_pause_label(playing))
        {
            tracing::debug!("tray: Play / Pause label not updated: {e}");
        }
    }

    pub fn set_muted(app: &AppHandle, muted: bool) {
        if let Some(labels) = app.try_state::<Labels>()
            && let Err(e) = labels.mute.set_text(mute_label(muted))
        {
            tracing::debug!("tray: Mute / Unmute label not updated: {e}");
        }
    }

    pub fn set_window_shown(app: &AppHandle, shown: bool) {
        if let Some(labels) = app.try_state::<Labels>()
            && let Err(e) = labels.toggle.set_text(toggle_label(shown))
        {
            tracing::debug!("tray: Show / Hide label not updated: {e}");
        }
    }
}
