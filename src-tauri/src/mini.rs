// SPDX-License-Identifier: MPL-2.0
//! Mini player: a small always-on-top window that stands in for the main one.
//!
//! It loads the same front end as the main window (`main.ts` mounts the mini player when the
//! window's label is [`LABEL`]), and every event is emitted to all windows, so it follows the
//! player with no state of its own.
//!
//! Opening it **closes** the main window, as closing it from its title bar does: one web view at a
//! time, the main window's memory freed (`windows.rs`). Coming back is always
//! [`crate::windows::show_main_window`] — the mini player's own button, a shared link — which
//! opens the main window first, then takes the mini player down, so the app never goes without a
//! window it can come back from. The tray's Hide takes it down alone: the tray is the way back.
//!
//! The app remembers which of the two was shown last ([`last`]): Show, a second launch and the
//! next launch open that one.
//!
//! Two sizes, full and compact; the front end picks its layout from the window's width, so the
//! size is the only copy of the mode. Both the size and the position are kept for the next time.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::db::{settings, Database};

pub const LABEL: &str = "mini";

/// Logical sizes, full and compact. Not resizable by the user: its own button switches them.
///
/// Held by size constraints (minimum = maximum) rather than by `resizable(false)`: GTK ignores a
/// resize of a window that is not resizable, so the compact size could never be reached. With no
/// title bar and no border there is nothing to drag a size from anyway.
const FULL: (f64, f64) = (560.0, 180.0);
const COMPACT: (f64, f64) = (320.0, 88.0);
/// Gap to the screen's edges the first time it opens.
const MARGIN: f64 = 24.0;

/// Whether the mini player is up. Read by the track notifications, from the audio thread: the
/// mini player already shows the title, on top of everything.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Whether the mini player was the last window shown, as [`settings::MINI_LAST`] keeps it. Read
/// from the database once at launch ([`read_last`]), then followed here: the main thread, which
/// toggles the windows, must not wait for the database.
static LAST: AtomicBool = AtomicBool::new(false);

pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Whether Show should bring back the mini player rather than the main window.
pub fn last() -> bool {
    LAST.load(Ordering::Relaxed)
}

/// Reads which window was shown last, at launch.
pub fn read_last(app: &AppHandle) -> bool {
    let last = app
        .state::<Database>()
        .flag(settings::MINI_LAST)
        .unwrap_or_else(|e| {
            tracing::warn!("last window not read: {e}");
            settings::MINI_LAST.default
        });
    LAST.store(last, Ordering::Relaxed);
    last
}

/// Keeps which window is shown, for Show and the next launch. From any thread: saved from a
/// blocking one, and only when it changes.
pub fn remember_last(app: &AppHandle, mini: bool) {
    if LAST.swap(mini, Ordering::Relaxed) == mini {
        return;
    }
    let app = app.clone();
    crate::state::spawn_blocking(move || {
        if let Err(e) = app.state::<Database>().set_flag(settings::MINI_LAST, mini) {
            tracing::warn!("last window not saved: {e}");
        }
    });
}

/// The size and the spot it was left at, read from the database before going to the main thread:
/// the database may be held for a while (an export), and the main thread must not wait for it.
pub struct Remembered {
    compact: bool,
    position: Option<PhysicalPosition<i32>>,
}

impl Remembered {
    pub fn read(app: &AppHandle) -> Self {
        let db = app.state::<Database>();
        Self {
            compact: db.flag(settings::MINI_COMPACT).unwrap_or_else(|e| {
                tracing::warn!("mini player size not read: {e}");
                false
            }),
            position: db
                .text(settings::MINI_POSITION)
                .unwrap_or_else(|e| {
                    tracing::warn!("mini player position not read: {e}");
                    None
                })
                .and_then(|v| parse_position(&v)),
        }
    }
}

/// Opens the mini player (or brings it forward), then closes the main window. Main thread only:
/// windows are built there.
pub fn open(app: &AppHandle, remembered: &Remembered) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.show()?;
        window.set_focus()?;
    } else {
        let (width, height) = if remembered.compact { COMPACT } else { FULL };
        let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
            // Its own title, untranslated, so that window manager rules can match it.
            .title("Soundfolk mini player")
            .inner_size(width, height)
            .min_inner_size(width, height)
            .max_inner_size(width, height)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            // A square shadow around a window with rounded corners looks broken.
            .shadow(false)
            // Placed before it shows, so it never appears first where the window manager guessed.
            .visible(false)
            .build()?;
        crate::windows::software_rendering(&window);
        // WebKitGTK's view asks for at least 200 px of height and GTK sizes the window to fit it:
        // without this the window would be 200 px high, and could not be made compact.
        #[cfg(target_os = "linux")]
        if let Err(e) = window.with_webview(|webview| {
            use gtk::prelude::WidgetExt;
            webview.inner().set_size_request(1, 1);
        }) {
            tracing::debug!("mini player: web view size request kept: {e}");
        }
        if let Some(position) = placement(app, &window, remembered.position, (width, height))
            && let Err(e) = window.set_position(position)
        {
            tracing::debug!("mini player: position not restored: {e}");
        }
        window.show()?;
        window.set_focus()?;
    }
    OPEN.store(true, Ordering::Relaxed);
    remember_last(app, true);
    crate::tray::show_window_state(app, true);
    // Last, once the mini player is up: if it failed, the main window stays.
    if let Some(main) = app.get_webview_window(crate::windows::MAIN_WINDOW) {
        main.close()?;
    }
    Ok(())
}

/// Takes the mini player down, keeping where it was: after the main window was shown, or for the
/// tray's Hide. Main thread only.
pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    // Called on the main thread: saved from a blocking thread.
    if let Some(value) = position_of(&window) {
        let app = app.clone();
        crate::state::spawn_blocking(move || save_position_value(&app, &value));
    }
    OPEN.store(false, Ordering::Relaxed);
    // Destroyed rather than closed: closing asks first, and that request is turned into a return
    // to the main window (`lib.rs`).
    if let Err(e) = window.destroy() {
        tracing::warn!("mini player not closed: {e}");
    }
    crate::tray::show_window_state(app, crate::windows::main_window_shown(app));
}

/// Switches between the full and the compact size, and keeps the choice.
pub fn set_compact(app: &AppHandle, compact: bool) -> tauri::Result<()> {
    let window = app
        .get_webview_window(LABEL)
        .ok_or(tauri::Error::WindowNotFound)?;
    let (width, height) = if compact { COMPACT } else { FULL };
    let scale = window.scale_factor()?;
    let new = PhysicalSize::new(
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    );
    // Read before the resize: the old size is gone after it.
    let moved = match (
        window.outer_position(),
        window.outer_size(),
        window.current_monitor(),
    ) {
        (Ok(position), Ok(old), Ok(Some(monitor))) => Some(anchored(
            position,
            old,
            new,
            *monitor.position(),
            *monitor.size(),
        )),
        _ => None,
    };
    // The constraints are lifted for the resize, then set to the new size.
    window.set_min_size(None::<tauri::Size>)?;
    window.set_max_size(None::<tauri::Size>)?;
    window.set_size(new)?;
    window.set_min_size(Some(new))?;
    window.set_max_size(Some(new))?;
    // Ignored on Wayland, where the compositor keeps the top-left corner.
    if let Some(position) = moved {
        let _ = window.set_position(position);
    }
    if let Err(e) = app
        .state::<Database>()
        .set_flag(settings::MINI_COMPACT, compact)
    {
        tracing::warn!("mini player size not saved: {e}");
    }
    Ok(())
}

/// Keeps where the mini player is, if it is up. Also called on exit: Quit never goes through
/// [`close`].
pub fn save_position(app: &AppHandle) {
    if let Some(value) = app.get_webview_window(LABEL).and_then(|w| position_of(&w)) {
        save_position_value(app, &value);
    }
}

/// `x,y` in physical pixels, as [`parse_position`] reads it.
fn position_of(window: &WebviewWindow) -> Option<String> {
    let p = window.outer_position().ok()?;
    Some(format!("{},{}", p.x, p.y))
}

fn save_position_value(app: &AppHandle, value: &str) {
    if let Err(e) = app
        .state::<Database>()
        .set_text(settings::MINI_POSITION, value)
    {
        tracing::warn!("mini player position not saved: {e}");
    }
}

/// [`open`] from any thread, a failure logged (the tray's menu).
pub fn show(app: &AppHandle) {
    let app = app.clone();
    crate::state::spawn_blocking(move || {
        let remembered = Remembered::read(&app);
        let handle = app.clone();
        let opened = app.run_on_main_thread(move || {
            if let Err(e) = open(&handle, &remembered) {
                tracing::warn!("mini player not opened: {e}");
            }
        });
        if let Err(e) = opened {
            tracing::warn!("mini player not opened: {e}");
        }
    });
}

/// Hands over from the main window to the mini player.
#[tauri::command]
#[specta::specta]
pub async fn open_mini_player(app: AppHandle) -> Result<(), String> {
    // Windows are built on the main thread: the answer comes back from there, so that a failure
    // reaches the button rather than only the log.
    let reader = app.clone();
    let remembered = crate::state::spawn_blocking(move || Remembered::read(&reader))
        .await
        .map_err(|e| e.to_string())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(open(&handle, &remembered).map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "the mini player did not open".to_owned())?
}

/// Back to the main window, the same way as from the tray.
#[tauri::command]
#[specta::specta]
pub fn close_mini_player(app: AppHandle) {
    crate::windows::show_main_window(&app);
}

/// The mini player's shrink and expand buttons.
#[tauri::command]
#[specta::specta]
pub async fn set_mini_compact(app: AppHandle, compact: bool) -> Result<(), String> {
    // Off the async runtime: the setting is saved to the database.
    crate::state::blocking(move || set_compact(&app, compact).map_err(|e| e.to_string().into()))
        .await
        .map_err(|e| e.to_string())
}

/// Next or previous favorite, as the media keys and the tray do it. Unlike them, the mini player
/// has a window to show a failure in.
#[tauri::command]
#[specta::specta]
pub async fn switch_favorite(app: AppHandle, forward: bool) -> Result<(), String> {
    crate::state::blocking(move || crate::media_controls::switch_favorite(&app, forward))
        .await
        .map_err(|e| e.to_string())
}

/// Where it was left, if that spot is still on a display (one may have been unplugged since),
/// otherwise the bottom-right corner of the display.
fn placement(
    app: &AppHandle,
    window: &WebviewWindow,
    saved: Option<PhysicalPosition<i32>>,
    size: (f64, f64),
) -> Option<PhysicalPosition<i32>> {
    let outer = window.outer_size().unwrap_or_default();
    saved
        .filter(|&p| {
            let middle = middle(p, outer);
            window.available_monitors().is_ok_and(|monitors| {
                monitors
                    .iter()
                    .any(|m| contains(*m.position(), *m.size(), middle))
            })
        })
        .or_else(|| bottom_right(app, window, size))
}

/// `x,y` in physical pixels, as [`position_of`] wrote it.
fn parse_position(s: &str) -> Option<PhysicalPosition<i32>> {
    let (x, y) = s.split_once(',')?;
    Some(PhysicalPosition::new(
        x.trim().parse().ok()?,
        y.trim().parse().ok()?,
    ))
}

/// Bottom-right of the main window's display, inside its work area (clear of a panel or a dock).
fn bottom_right(
    app: &AppHandle,
    window: &WebviewWindow,
    (width, height): (f64, f64),
) -> Option<PhysicalPosition<i32>> {
    let anchor = app
        .get_webview_window(crate::windows::MAIN_WINDOW)
        .unwrap_or_else(|| window.clone());
    let monitor = anchor
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| anchor.primary_monitor().ok().flatten())?;
    let area = monitor.work_area();
    let px = |logical: f64| (logical * monitor.scale_factor()).round() as i32;
    Some(PhysicalPosition::new(
        area.position.x + area.size.width as i32 - px(width + MARGIN),
        area.position.y + area.size.height as i32 - px(height + MARGIN),
    ))
}

fn middle(p: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> PhysicalPosition<i32> {
    PhysicalPosition::new(p.x + size.width as i32 / 2, p.y + size.height as i32 / 2)
}

/// Point in a display, physical pixels. A display left of or above the primary one has a negative
/// origin.
fn contains(
    origin: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    p: PhysicalPosition<i32>,
) -> bool {
    (origin.x..origin.x + size.width as i32).contains(&p.x)
        && (origin.y..origin.y + size.height as i32).contains(&p.y)
}

/// Top-left corner for a window going from `old` to `new` size, so that it keeps to the screen
/// edges it is near: made compact in the bottom-right corner, it stays in that corner. Decided per
/// axis by the half of the display its middle is in, so that shrinking then growing back returns
/// it to the same spot.
fn anchored(
    position: PhysicalPosition<i32>,
    old: PhysicalSize<u32>,
    new: PhysicalSize<u32>,
    origin: PhysicalPosition<i32>,
    display: PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let axis = |p: i32, old: u32, new: u32, start: i32, length: u32| {
        if 2 * p + old as i32 > 2 * start + length as i32 {
            p + old as i32 - new as i32
        } else {
            p
        }
    };
    PhysicalPosition::new(
        axis(position.x, old.width, new.width, origin.x, display.width),
        axis(position.y, old.height, new.height, origin.y, display.height),
    )
}

#[cfg(test)]
mod tests {
    use super::{anchored, contains, middle, parse_position};
    use tauri::{PhysicalPosition, PhysicalSize};

    #[test]
    fn reads_the_saved_position() {
        assert_eq!(
            parse_position("120,64"),
            Some(PhysicalPosition::new(120, 64))
        );
        assert_eq!(
            parse_position("-1800,-200"),
            Some(PhysicalPosition::new(-1800, -200))
        );
        assert_eq!(parse_position("garbage"), None);
        assert_eq!(parse_position("12,"), None);
    }

    #[test]
    fn a_window_is_on_a_display_by_its_middle() {
        let primary = (
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1920u32, 1080),
        );
        let left = (
            PhysicalPosition::new(-1920, 0),
            PhysicalSize::new(1920u32, 1080),
        );
        assert!(contains(
            primary.0,
            primary.1,
            PhysicalPosition::new(1300, 800)
        ));
        assert!(!contains(
            primary.0,
            primary.1,
            PhysicalPosition::new(-500, 800)
        ));
        assert!(contains(left.0, left.1, PhysicalPosition::new(-500, 800)));
        // The right and bottom edges belong to the next display.
        assert!(!contains(
            primary.0,
            primary.1,
            PhysicalPosition::new(1920, 500)
        ));
        // A window hanging off the right edge: its corner is on the display, its middle is not.
        let off = middle(
            PhysicalPosition::new(1800, 100),
            PhysicalSize::new(560, 180),
        );
        assert!(!contains(primary.0, primary.1, off));
    }

    #[test]
    fn resizing_keeps_the_nearest_edges() {
        let full = PhysicalSize::new(560u32, 180);
        let small = PhysicalSize::new(320u32, 88);
        let at = PhysicalPosition::new;
        let fhd = PhysicalSize::new(1920u32, 1080);
        let shrink = |p, origin| anchored(p, full, small, origin, fhd);
        let grow = |p, origin| anchored(p, small, full, origin, fhd);

        // Bottom-right: the right and bottom edges stay.
        assert_eq!(shrink(at(1336, 876), at(0, 0)), at(1576, 968));
        // Top-left: nothing moves.
        assert_eq!(shrink(at(24, 24), at(0, 0)), at(24, 24));
        // Top-right: only the right edge stays.
        assert_eq!(shrink(at(1336, 24), at(0, 0)), at(1576, 24));
        // On a display left of the primary one.
        assert_eq!(shrink(at(-584, 876), at(-1920, 0)), at(-344, 968));
        // Shrinking then growing back is a round trip.
        for p in [at(1336, 876), at(24, 24), at(900, 300)] {
            assert_eq!(grow(shrink(p, at(0, 0)), at(0, 0)), p);
        }
    }
}
