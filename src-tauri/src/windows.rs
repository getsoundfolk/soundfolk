// SPDX-License-Identifier: MPL-2.0
//! The main window: opened, shown, toggled and closed from anywhere (tray, media keys, a second
//! launch, the mini player), and opened again where it was when it was closed.
//!
//! Closing the window really closes it: its web view (WebKit, most of the app's memory) is freed
//! while the app keeps playing from the notification area (`tray.rs`).

use std::sync::Mutex;

use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent};

use crate::{mini, notifications, tray};

/// Label of the main window, as declared in `tauri.conf.json` (`create: false`: opened in `setup`).
pub const MAIN_WINDOW: &str = "main";

/// Size and position of the main window when it was last closed, to open it again in the same place.
#[derive(Default)]
pub struct WindowGeometry(Mutex<Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>>);

/// Whether the main window is open, visible and not minimized.
pub fn main_window_shown(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(MAIN_WINDOW).is_some_and(|window| {
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
    })
}

/// Shows the main window if it is closed or minimized, closes it otherwise.
pub fn toggle_main_window(app: &tauri::AppHandle) {
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) if main_window_shown(app) => {
            if let Err(e) = window.close() {
                tracing::warn!("failed to close the main window: {e}");
            }
        }
        _ => show_main_window(app),
    }
}

/// Brings the main window to the front, even from the notification area: once closed it no longer
/// exists, so it is created again (from any thread: windows are built on the main thread).
pub fn show_main_window(app: &tauri::AppHandle) {
    let handle = app.clone();
    let shown = app.run_on_main_thread(move || {
        let window = match handle.get_webview_window(MAIN_WINDOW) {
            Some(window) => window,
            None => match open_main_window(&handle) {
                Ok(window) => window,
                Err(e) => return tracing::warn!("failed to open the main window: {e}"),
            },
        };
        if let Err(e) = window
            .show()
            .and_then(|()| window.unminimize())
            .and_then(|()| window.set_focus())
        {
            tracing::warn!("failed to show the main window: {e}");
        }
        // Only once the main window is there: the mini player is the app's last window otherwise.
        mini::close(&handle);
    });
    if let Err(e) = shown {
        tracing::warn!("failed to show the main window: {e}");
    }
}

/// Creates the main window as `tauri.conf.json` declares it, where it was when it was closed.
pub fn open_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let windows = &app.config().app.windows;
    let config = windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW)
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;
    let window = WebviewWindowBuilder::from_config(app, &config)?.build()?;
    let geometry = *app
        .state::<WindowGeometry>()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some((position, size)) = geometry
        && let Err(e) = window
            .set_size(size)
            .and_then(|()| window.set_position(position))
    {
        tracing::debug!("previous window geometry not restored: {e}");
    }
    Ok(window)
}

/// The app's window events: the main window's geometry kept when it closes, the tray and the track
/// notifications told whether it is shown, the mini player's close turned into a return.
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    // Nothing in the mini player closes it, but a window manager shortcut can: that is a
    // return to the main window, or the app would play on with no window to come back to.
    if window.label() == mini::LABEL {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            show_main_window(window.app_handle());
        }
        return;
    }
    if let WindowEvent::CloseRequested { .. } = event
        && let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size())
    {
        let geometry = window.app_handle().state::<WindowGeometry>();
        *geometry.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((position, size));
    }
    // The tray menu offers to show the window or to hide it, whichever it would do. Being
    // minimized or restored comes as a resize (or a focus change), closing as `Destroyed`.
    // Track notifications are only for when the window does not have the focus.
    if window.label() == MAIN_WINDOW {
        match event {
            WindowEvent::Focused(focused) => notifications::set_foreground(*focused),
            WindowEvent::Destroyed => notifications::set_foreground(false),
            _ => {}
        }
        match event {
            WindowEvent::Destroyed => tray::show_window_state(window.app_handle(), false),
            WindowEvent::Resized(_) | WindowEvent::Focused(_) => {
                tray::show_window_state(window.app_handle(), main_window_shown(window.app_handle()))
            }
            _ => {}
        }
    }
}
