// SPDX-License-Identifier: MPL-2.0
mod audio;
mod catalog;
mod commands;
mod db;
mod error;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

use audio::{DesktopAudioEngine, NowPlaying};
use catalog::radio_browser::RadioBrowser;
use commands::{AudioState, CatalogState, OnAir};
use db::Database;

/// Event emitted on every player state change (payload: `PlayerState`).
const PLAYER_STATE_EVENT: &str = "player:state";
/// Event emitted when the on-air station or title changes (payload: `NowPlaying`).
const NOW_PLAYING_EVENT: &str = "player:now-playing";

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
            let engine = DesktopAudioEngine::new(
                Arc::new(move |state| {
                    if let Err(e) = state_handle.emit(PLAYER_STATE_EVENT, state) {
                        tracing::warn!("failed to emit player state: {e}");
                    }
                }),
                Arc::new(move |now_playing: NowPlaying| {
                    journalize_title(&title_handle, &station, &last_logged, &now_playing);
                    if let Err(e) = title_handle.emit(NOW_PLAYING_EVENT, now_playing) {
                        tracing::warn!("failed to emit now-playing title: {e}");
                    }
                }),
            );
            app.manage(AudioState::new(engine, on_air));
            app.manage(CatalogState(Arc::new(RadioBrowser::new()?)));
            Ok(())
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
            commands::report_station,
            commands::search_stations,
            commands::get_click_counting,
            commands::set_click_counting,
            commands::add_personal_station,
            commands::list_personal_stations,
            commands::remove_personal_station,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Soundfolk");
}
