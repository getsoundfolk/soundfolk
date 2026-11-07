// SPDX-License-Identifier: MPL-2.0
mod audio;
mod catalog;
mod commands;
mod covers;
mod curated_lists;
mod daily_list;
mod db;
mod donations;
mod error;
mod export;
mod favorite_repair;
mod media_controls;
mod musicbrainz;
mod notifications;
mod recipe_list;
mod recipes;
mod track_services;
mod tracks;
mod tray;
mod volume;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent};
use tracing_subscriber::EnvFilter;

use audio::{DesktopAudioEngine, NowPlaying, PlayerState, WebEvent, WebTitles};
use catalog::radio_browser::RadioBrowser;
use commands::{AudioState, CatalogState, OnAir};
use db::Database;
use media_controls::SystemMedia;

/// Event emitted on every player state change (payload: `PlayerState`).
const PLAYER_STATE_EVENT: &str = "player:state";
/// Event emitted when the on-air station or title changes (payload: `NowPlaying`).
const NOW_PLAYING_EVENT: &str = "player:now-playing";
/// Event emitted when the source of the title changes (payload: `TitleStatus`).
const TITLE_STATUS_EVENT: &str = "player:title-status";
/// Event emitted after a local recipe file was saved (payload: `RecipeNotice`).
const RECIPE_NOTICE_EVENT: &str = "recipes:notice";
/// Event emitted when a new list of donation links was downloaded (no payload).
const DONATIONS_EVENT: &str = "donations:updated";
/// Event emitted when new curated lists were downloaded (no payload).
const LISTS_EVENT: &str = "lists:updated";
/// How often an open listening session records that it is still playing.
const SESSION_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);

/// Adds the title to `listening_history`, without consecutive duplicates: while listening goes on,
/// the same track on the same station several times in a row is one entry (checked here, then by
/// the database, which also catches another spelling of the title). After a stop (`last_logged`
/// cleared, see `track_session`), the same track is heard again and gets its own entry. Some radios
/// send their own name as the title between tracks: that is not a track, so it is skipped.
/// Returns whether a new entry was added: a new track on air.
fn journalize_title(
    app: &tauri::AppHandle,
    station: &Mutex<Option<OnAir>>,
    last_logged: &Mutex<Option<Logged>>,
    now_playing: &NowPlaying,
) -> bool {
    let Some(title) = now_playing
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    let Some(station) = station.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
        return false;
    };
    let is_station_name =
        |name: Option<&str>| name.is_some_and(|n| n.trim().eq_ignore_ascii_case(title));
    if is_station_name(station.name.as_deref()) || is_station_name(now_playing.station.as_deref()) {
        return false;
    }

    let mut last = last_logged.lock().unwrap_or_else(|p| p.into_inner());
    let entry = (station.id, title.to_owned());
    let db = app.state::<Database>();
    if let Some(logged) = last
        .as_mut()
        .filter(|l| l.station_id == entry.0 && l.title == entry.1)
    {
        // The same title, now with artist and title apart (a server API confirming the ICY title).
        if let (Some(track), false) = (&now_playing.track, logged.structured) {
            let parsed = tracks::ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            };
            match db.set_track_parts(&entry.0, &entry.1, &parsed) {
                Ok(()) => logged.structured = true,
                Err(e) => tracing::warn!("listening history not updated: {e}"),
            }
        }
        return false;
    }
    let structured = now_playing.track.is_some();
    let continuing = last.is_some();
    let logged = match &now_playing.track {
        // Artist and title given apart by the source: stored as given, with the real start.
        Some(track) => {
            let parsed = tracks::ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            };
            let played_at = track.started_at.as_deref().and_then(history_time);
            db.log_structured_track(
                &entry.0,
                station.name.as_deref(),
                &entry.1,
                &parsed,
                played_at.as_deref(),
                continuing,
            )
        }
        None => db.log_track(&entry.0, station.name.as_deref(), &entry.1, continuing),
    };
    match logged {
        Ok(_) => {
            *last = Some(Logged {
                station_id: entry.0,
                title: entry.1,
                structured,
            });
            true
        }
        Err(e) => {
            tracing::warn!("listening history not saved: {e}");
            false
        }
    }
}

/// The last title added to the history.
struct Logged {
    station_id: String,
    title: String,
    /// Stored with artist and title as the source gave them.
    structured: bool,
}

/// An RFC 3339 start as stored in the history (`YYYY-MM-DD HH:MM:SS`, UTC), if plausible: not
/// in the future, and less than a day old.
fn history_time(started_at: &str) -> Option<String> {
    let started: jiff::Timestamp = started_at.parse().ok()?;
    let age = jiff::Timestamp::now().duration_since(started);
    (age >= jiff::SignedDuration::ZERO && age < jiff::SignedDuration::from_hours(24))
        .then(|| started.strftime("%Y-%m-%d %H:%M:%S").to_string())
}

/// Opens a listening session when a station starts playing and closes it when playback stops. A
/// stop, a pause or an error also ends the run of titles `journalize_title` merges; a
/// reconnection does not.
fn track_session(
    app: &tauri::AppHandle,
    station: &Mutex<Option<OnAir>>,
    last_logged: &Mutex<Option<Logged>>,
    state: &PlayerState,
) {
    if matches!(
        state,
        PlayerState::Idle | PlayerState::Paused | PlayerState::Error(_)
    ) {
        *last_logged.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
    let db = app.state::<Database>();
    let result = match state {
        PlayerState::Playing => match station.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(s) => db.start_session(&s.id, s.name.as_deref()),
            None => Ok(()),
        },
        // While reconnecting nothing is heard: the session ends when the sound stopped (a sleep
        // is not listening time), and a new one starts when it comes back.
        PlayerState::Idle
        | PlayerState::Paused
        | PlayerState::Reconnecting(_)
        | PlayerState::Error(_) => db.end_session(),
        PlayerState::Connecting => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!("listening session not saved: {e}");
    }
}

/// Label of the main window, as declared in `tauri.conf.json` (`create: false`: opened in `setup`).
const MAIN_WINDOW: &str = "main";

/// Argument of the launch at login (Settings): the app starts in the notification area, without
/// its window, and does not play.
const HIDDEN_ARG: &str = "--hidden";

/// Size and position of the main window when it was last closed, to open it again in the same place.
#[derive(Default)]
struct WindowGeometry(Mutex<Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>>);

/// Whether the main window is open, visible and not minimized.
fn main_window_shown(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(MAIN_WINDOW).is_some_and(|window| {
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
    })
}

/// Shows the main window if it is closed or minimized, closes it otherwise.
fn toggle_main_window(app: &tauri::AppHandle) {
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
fn show_main_window(app: &tauri::AppHandle) {
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
    });
    if let Err(e) = shown {
        tracing::warn!("failed to show the main window: {e}");
    }
}

/// Creates the main window as `tauri.conf.json` declares it, where it was when it was closed.
fn open_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
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
    if let Some((position, size)) = geometry {
        if let Err(e) = window
            .set_size(size)
            .and_then(|()| window.set_position(position))
        {
            tracing::debug!("previous window geometry not restored: {e}");
        }
    }
    Ok(window)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("soundfolk_lib=info")),
        )
        .init();

    let builder = tauri::Builder::default();
    // Registered first, as the plugin asks: a second launch hands over to the running copy
    // (which keeps playing and shows its window) and exits before opening the database or the audio
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ));

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(Database::open(data_dir.join("soundfolk.db"))?);
            app.manage(WindowGeometry::default());

            // The last station is in the player from launch: Play (window, tray, media keys) starts it.
            let on_air = Arc::new(Mutex::new(commands::saved_station(
                &app.state::<Database>(),
            )));
            let last_logged = Arc::new(Mutex::new(None::<Logged>));
            let session_logged = last_logged.clone();

            let state_handle = app.handle().clone();
            let title_handle = app.handle().clone();
            let station = on_air.clone();
            let session_station = on_air.clone();
            let media = Arc::new(SystemMedia::new(app.handle().clone()));
            let musicbrainz = musicbrainz::MusicBrainz::new()
                .map(Arc::new)
                .inspect_err(|e| tracing::warn!("MusicBrainz lookups disabled: {e}"))
                .ok();
            let covers =
                covers::Covers::new(app.handle().clone(), musicbrainz.clone(), media.clone());
            app.manage(commands::CoversState(covers.clone()));
            app.manage(notifications::TrackNotifications::new(
                app.handle(),
                &app.state::<Database>(),
                covers::app_icon_file(app.handle()),
            ));
            let local_recipes = app
                .path()
                .app_config_dir()
                .ok()
                .map(|dir| dir.join("recipes"));
            let events = app.handle().clone();
            let web_titles = Arc::new(WebTitles::new(
                Arc::new(recipes::Recipes::new(local_recipes)),
                app.state::<Database>()
                    .get_bool(commands::SETTING_WEB_TITLES, true)?,
                Box::new(move |event| {
                    let sent = match event {
                        WebEvent::Status(status) => events.emit(TITLE_STATUS_EVENT, status),
                        WebEvent::Notice(notice) => events.emit(RECIPE_NOTICE_EVENT, notice),
                    };
                    if let Err(e) = sent {
                        tracing::warn!("failed to emit a recipe event: {e}");
                    }
                }),
            ));
            app.manage(commands::WebTitlesState(web_titles.clone()));
            recipe_list::start(
                web_titles.clone(),
                app.path()
                    .app_cache_dir()
                    .ok()
                    .map(|dir| dir.join("recipes")),
            );
            let donations = Arc::new(donations::Donations::default());
            app.manage(donations.clone());
            let donation_events = app.handle().clone();
            donations::start(
                donations,
                app.path()
                    .app_cache_dir()
                    .ok()
                    .map(|dir| dir.join("stations")),
                move || {
                    if let Err(e) = donation_events.emit(DONATIONS_EVENT, ()) {
                        tracing::warn!("failed to emit {DONATIONS_EVENT}: {e}");
                    }
                },
            );
            let lists = Arc::new(curated_lists::CuratedLists::default());
            app.manage(lists.clone());
            let list_events = app.handle().clone();
            curated_lists::start(
                lists,
                app.path()
                    .app_cache_dir()
                    .ok()
                    .map(|dir| dir.join("stations")),
                move || {
                    if let Err(e) = list_events.emit(LISTS_EVENT, ()) {
                        tracing::warn!("failed to emit {LISTS_EVENT}: {e}");
                    }
                },
            );
            let state_media = media.clone();
            let engine = DesktopAudioEngine::new(
                Arc::new(move |state| {
                    let state = favorite_repair::on_state(&state_handle, state);
                    let state = media_controls::on_state(&state_handle, state);
                    track_session(&state_handle, &session_station, &session_logged, &state);
                    commands::schedule_click(&state_handle, &state);
                    state_media.show_state(&state);
                    tray::show_state(&state_handle, &state);
                    if let Err(e) = state_handle.emit(PLAYER_STATE_EVENT, state) {
                        tracing::warn!("failed to emit player state: {e}");
                    }
                }),
                Arc::new(move |now_playing: NowPlaying| {
                    let new_track =
                        journalize_title(&title_handle, &station, &last_logged, &now_playing);
                    let on_air = station.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    let name = on_air.as_ref().and_then(|s| s.name.clone());
                    let label = on_air.and_then(|s| {
                        let db = title_handle.state::<Database>();
                        db.station_name(&s.id).unwrap_or_else(|e| {
                            tracing::warn!("station name not read: {e}");
                            None
                        })
                    });
                    if new_track {
                        let notifications =
                            title_handle.state::<notifications::TrackNotifications>();
                        notifications.track(&now_playing, name.as_deref(), label.as_deref());
                    }
                    covers.update(now_playing.clone(), name, label);
                    if let Err(e) = title_handle.emit(NOW_PLAYING_EVENT, now_playing) {
                        tracing::warn!("failed to emit now-playing title: {e}");
                    }
                }),
                web_titles,
            );
            app.manage(favorite_repair::FavoriteRepair::default());
            app.manage(media_controls::Zap::default());
            engine.set_normalize(volume::constant_volume(&app.state::<Database>()));
            engine.set_level_memory(Arc::new(volume::DatabaseLevels(app.handle().clone())));
            app.manage(AudioState::new(engine, on_air));
            app.manage(volume::Sound::default());
            app.manage(CatalogState(Arc::new(RadioBrowser::default())));

            let identify = Arc::new(tokio::sync::Notify::new());
            if let Some(mb) = musicbrainz {
                musicbrainz::spawn_worker(app.handle().clone(), mb, identify.clone());
            }
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

            // Launched at login: no window, so no web view, until the icon is clicked. If the icon turns
            // out to show nowhere, the window opens after all (`tray::set_unavailable`).
            let hidden = std::env::args().skip(1).any(|arg| arg == HIDDEN_ARG);
            if hidden {
                tray::show_window_state(app.handle(), false);
            }
            // Without an icon, closing the window quits: there would be no way back to it
            if let Err(e) = tray::init(app) {
                tracing::warn!("no notification area icon: {e}");
                tray::set_available(false);
            }
            if !hidden || !tray::available() {
                open_main_window(app.handle())?;
            }
            Ok(())
        })
        // Closing the window really closes it: its web view (WebKit, most of the app's memory) is
        // freed while the app keeps playing from the notification area (`tray.rs`). "Quit" in its menu exits.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                if let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) {
                    let geometry = window.app_handle().state::<WindowGeometry>();
                    *geometry.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((position, size));
                }
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
                    WindowEvent::Resized(_) | WindowEvent::Focused(_) => tray::show_window_state(
                        window.app_handle(),
                        main_window_shown(window.app_handle()),
                    ),
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::play_stream,
            commands::stop_stream,
            commands::pause_stream,
            commands::resume_stream,
            commands::set_volume,
            commands::get_volume,
            commands::save_volume,
            commands::get_player_state,
            commands::get_now_playing,
            commands::get_current_station,
            commands::add_favorite,
            commands::remove_favorite,
            commands::list_favorites,
            commands::reorder_favorites,
            commands::get_listening_history,
            commands::get_history_summary,
            commands::clear_listening_history,
            commands::export_history,
            commands::like_track,
            commands::unlike_track,
            commands::list_likes,
            commands::get_stats,
            commands::get_track_identification,
            commands::get_cover_sources,
            commands::set_cover_sources,
            commands::get_cover,
            commands::set_track_identification,
            commands::search_stations,
            commands::get_station,
            commands::resolve_station,
            commands::get_click_counting,
            commands::set_click_counting,
            commands::get_web_titles,
            commands::set_web_titles,
            commands::get_autostart,
            commands::set_autostart,
            notifications::get_track_notifications,
            notifications::set_track_notifications,
            volume::get_constant_volume,
            volume::set_constant_volume,
            volume::get_level_adjustment,
            volume::get_muted,
            volume::set_muted,
            commands::get_title_status,
            commands::reload_recipes,
            commands::test_recipe,
            commands::edit_recipe,
            commands::share_recipe,
            commands::get_local_recipes,
            commands::open_recipes_folder,
            commands::get_track_services,
            commands::set_track_services,
            commands::get_custom_track_services,
            commands::set_custom_track_services,
            commands::add_personal_station,
            commands::list_personal_stations,
            commands::remove_personal_station,
            commands::get_station_names,
            commands::set_station_name,
            donations::get_donation_link,
            curated_lists::get_curated_lists,
            curated_lists::get_lists_for_station,
            curated_lists::get_curated_list_stations,
            curated_lists::export_curated_list,
            curated_lists::get_favorite_lists,
            curated_lists::set_favorite_list,
            curated_lists::export_my_list,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Soundfolk")
        .run(|app, event| {
            // The last window closing does not end the app (no exit code): it goes on in the tray,
            // when there is one to come back from
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = &event
            {
                if tray::available() {
                    api.prevent_exit();
                }
            }
            if let tauri::RunEvent::Exit = event {
                if let Err(e) = app.state::<Database>().end_session() {
                    tracing::warn!("listening session not closed: {e}");
                }
            }
        });
}
