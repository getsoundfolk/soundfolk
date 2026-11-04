// SPDX-License-Identifier: MPL-2.0
mod audio;
mod catalog;
mod commands;
mod db;
mod error;
mod media_controls;
mod musicbrainz;
mod tracks;

use std::sync::{Arc, Mutex};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};
use tracing_subscriber::EnvFilter;

use audio::{DesktopAudioEngine, NowPlaying, PlayerState};
use catalog::radio_browser::RadioBrowser;
use commands::{AudioState, CatalogState, OnAir};
use db::Database;
use media_controls::SystemMedia;

/// Event emitted on every player state change (payload: `PlayerState`).
const PLAYER_STATE_EVENT: &str = "player:state";
/// Event emitted when the on-air station or title changes (payload: `NowPlaying`).
const NOW_PLAYING_EVENT: &str = "player:now-playing";
/// How often an open listening session records that it is still playing.
const SESSION_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);

/// Adds the title to `listening_history`, without consecutive duplicates (a reconnection or a
/// server repeating the same ICY block must not create extra rows). Some radios send their own
/// name as the title between tracks: that is not a track, so it is skipped.
fn journalize_title(
    app: &tauri::AppHandle,
    station: &Mutex<Option<OnAir>>,
    last_logged: &Mutex<Option<(String, String)>>,
    now_playing: &NowPlaying,
) {
    let Some(title) = now_playing.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    let Some(station) = station.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
        return;
    };
    let is_station_name = |name: Option<&str>| name.is_some_and(|n| n.trim().eq_ignore_ascii_case(title));
    if is_station_name(station.name.as_deref()) || is_station_name(now_playing.station.as_deref()) {
        return;
    }

    let mut last = last_logged.lock().unwrap_or_else(|p| p.into_inner());
    let entry = (station.id, title.to_owned());
    if last.as_ref() == Some(&entry) {
        return;
    }
    match app.state::<Database>().log_track(&entry.0, station.name.as_deref(), &entry.1) {
        Ok(()) => *last = Some(entry),
        Err(e) => tracing::warn!("listening history not saved: {e}"),
    }
}

/// Opens a listening session when a station starts playing and closes it when playback stops.
fn track_session(app: &tauri::AppHandle, station: &Mutex<Option<OnAir>>, state: &PlayerState) {
    let db = app.state::<Database>();
    let result = match state {
        PlayerState::Playing => match station.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(s) => db.start_session(&s.id, s.name.as_deref()),
            None => Ok(()),
        },
        PlayerState::Idle | PlayerState::Paused | PlayerState::Error(_) => db.end_session(),
        PlayerState::Connecting => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!("listening session not saved: {e}");
    }
}

/// Shows the main window if it is hidden or minimized, hides it otherwise.
fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
    if !visible {
        show_main_window(app);
    } else if let Err(e) = window.hide() {
        tracing::warn!("failed to hide the main window: {e}");
    }
}

/// Brings the main window to the front, even from the notification area.
fn show_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.show().and_then(|()| window.unminimize()).and_then(|()| window.set_focus()) {
        tracing::warn!("failed to show the main window: {e}");
    }
}

/// Notification area icon: the app keeps playing in the background once its window is closed,
/// so this is the way back to the window and the way to really quit.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / hide Soundfolk", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Soundfolk")
        .menu(&Menu::with_items(app, &[&toggle, &quit])?)
        // Left click toggles the window (Windows and macOS; on Linux every click opens the menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => toggle_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("soundfolk_lib=info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(Database::open(data_dir.join("soundfolk.db"))?);

            let on_air = Arc::new(Mutex::new(None::<OnAir>));
            let last_logged = Mutex::new(None::<(String, String)>);

            let state_handle = app.handle().clone();
            let title_handle = app.handle().clone();
            let station = on_air.clone();
            let session_station = on_air.clone();
            let media = Arc::new(SystemMedia::new(app.handle().clone()));
            let state_media = media.clone();
            let engine = DesktopAudioEngine::new(
                Arc::new(move |state| {
                    track_session(&state_handle, &session_station, &state);
                    commands::schedule_click(&state_handle, &state);
                    state_media.show_state(&state);
                    if let Err(e) = state_handle.emit(PLAYER_STATE_EVENT, state) {
                        tracing::warn!("failed to emit player state: {e}");
                    }
                }),
                Arc::new(move |now_playing: NowPlaying| {
                    journalize_title(&title_handle, &station, &last_logged, &now_playing);
                    let name = station.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|s| s.name.clone());
                    media.show_now_playing(&now_playing, name.as_deref());
                    if let Err(e) = title_handle.emit(NOW_PLAYING_EVENT, now_playing) {
                        tracing::warn!("failed to emit now-playing title: {e}");
                    }
                }),
            );
            app.manage(AudioState::new(engine, on_air));
            app.manage(CatalogState(Arc::new(RadioBrowser::new()?)));

            let identify = Arc::new(tokio::sync::Notify::new());
            musicbrainz::spawn_worker(app.handle().clone(), identify.clone());
            app.manage(musicbrainz::IdentifyWaker(identify));

            let heartbeat = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(SESSION_HEARTBEAT).await;
                    if let Err(e) = heartbeat.state::<Database>().touch_session() {
                        tracing::warn!("listening session heartbeat failed: {e}");
                    }
                }
            });

            build_tray(app)?;
            Ok(())
        })
        // Closing the window sends it to the notification area; "Quit" in the tray menu exits
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(e) = window.hide() {
                    tracing::warn!("failed to hide the main window: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::play_stream,
            commands::stop_stream,
            commands::pause_stream,
            commands::resume_stream,
            commands::set_volume,
            commands::get_player_state,
            commands::get_now_playing,
            commands::add_favorite,
            commands::remove_favorite,
            commands::list_favorites,
            commands::get_listening_history,
            commands::clear_listening_history,
            commands::like_track,
            commands::unlike_track,
            commands::list_likes,
            commands::get_stats,
            commands::get_track_identification,
            commands::set_track_identification,
            commands::report_station,
            commands::search_stations,
            commands::resolve_station,
            commands::get_click_counting,
            commands::set_click_counting,
            commands::add_personal_station,
            commands::list_personal_stations,
            commands::remove_personal_station,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Soundfolk")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Err(e) = app.state::<Database>().end_session() {
                    tracing::warn!("listening session not closed: {e}");
                }
            }
        });
}
