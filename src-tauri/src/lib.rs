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
mod listening;
mod media_controls;
mod mini;
mod musicbrainz;
mod notifications;
mod recipe_list;
mod recipes;
mod share;
mod track_services;
mod tracks;
mod tray;
#[cfg(desktop)]
mod updates;
mod volume;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent};
use tracing_subscriber::EnvFilter;

use audio::{DesktopAudioEngine, NowPlaying, WebEvent, WebTitles};
use catalog::radio_browser::RadioBrowser;
use commands::{AudioState, CatalogState};
use db::Database;
use media_controls::SystemMedia;

/// Event emitted on every player state change (payload: `PlayerState`).
const PLAYER_STATE_EVENT: &str = "player:state";
/// Event emitted when the source of the title changes (payload: `TitleStatus`).
const TITLE_STATUS_EVENT: &str = "player:title-status";
/// Event emitted after a local recipe file was saved (payload: `RecipeNotice`).
const RECIPE_NOTICE_EVENT: &str = "recipes:notice";
/// Event emitted when a new list of donation links was downloaded (no payload).
const DONATIONS_EVENT: &str = "donations:updated";
/// Event emitted when new curated lists were downloaded (no payload).
const LISTS_EVENT: &str = "lists:updated";

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
        // Only once the main window is there: the mini player is the app's last window otherwise.
        mini::close(&handle);
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
    if let Some((position, size)) = geometry
        && let Err(e) = window
            .set_size(size)
            .and_then(|()| window.set_position(position))
    {
        tracing::debug!("previous window geometry not restored: {e}");
    }
    Ok(window)
}

/// `soundfolk://` links (docs/16-sharing.md §3.1): the one Soundfolk was started with, and the
/// ones opened while it runs (a second launch hands them over through single-instance). The
/// scheme is registered again at every launch on Linux and Windows, which covers the AppImage and
/// development builds; the `.deb` declares it too.
#[cfg(desktop)]
fn watch_share_links(app: &tauri::App) {
    use tauri_plugin_deep_link::DeepLinkExt;
    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        if let Some(url) = event.urls().last()
            && share::opened(&handle, url.as_str())
        {
            show_main_window(&handle);
        }
    });
    if let Ok(Some(urls)) = app.deep_link().get_current()
        && let Some(url) = urls.last()
    {
        share::opened(app.handle(), url.as_str());
    }
    #[cfg(any(target_os = "linux", windows))]
    {
        // Runs `update-desktop-database` and `xdg-mime` on Linux: not on the startup path.
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            if let Err(e) = handle.deep_link().register_all() {
                tracing::warn!("soundfolk:// links not registered: {e}");
            }
        });
    }
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
        .plugin(tauri_plugin_deep_link::init())
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
            app.manage(share::OpenedLink::default());
            #[cfg(desktop)]
            watch_share_links(app);

            // The last station is in the player from launch: Play (window, tray, media keys) starts it.
            let on_air = Arc::new(Mutex::new(commands::saved_station(
                &app.state::<Database>(),
            )));

            let state_handle = app.handle().clone();
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
                // A setting that cannot be read takes its default, as Constant volume does: it
                // is no reason not to start.
                app.state::<Database>()
                    .get_bool(commands::SETTING_WEB_TITLES, true)
                    .unwrap_or_else(|e| {
                        tracing::warn!("web titles setting not read: {e}");
                        true
                    }),
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
            #[cfg(desktop)]
            {
                let updates = Arc::new(updates::Updates::new(app.handle()));
                app.manage(updates.clone());
                updates::start(
                    app.handle(),
                    updates,
                    app.path()
                        .app_cache_dir()
                        .ok()
                        .map(|dir| dir.join("updates")),
                );
            }
            let journal =
                listening::Journal::start(app.handle().clone(), covers.clone(), on_air.clone())?;
            let state_journal = journal.clone();
            let state_media = media.clone();
            let engine = DesktopAudioEngine::new(
                Arc::new(move |state| {
                    let state = favorite_repair::on_state(&state_handle, state);
                    let state = media_controls::on_state(&state_handle, state);
                    state_journal.state(&state);
                    commands::schedule_click(&state_handle, &state);
                    state_media.show_state(&state);
                    tray::show_state(&state_handle, &state);
                    if let Err(e) = state_handle.emit(PLAYER_STATE_EVENT, state) {
                        tracing::warn!("failed to emit player state: {e}");
                    }
                }),
                // Logged, then announced, on the `listening` thread: never waits here.
                Arc::new(move |now_playing: NowPlaying| journal.title(now_playing)),
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
            commands::surprise_candidates,
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
            curated_lists::refresh_curated_lists,
            share::read_share_link,
            share::make_share_link,
            share::get_share_name,
            share::set_share_name,
            share::take_opened_link,
            share::catalog_station_for_stream,
            mini::open_mini_player,
            mini::close_mini_player,
            mini::set_mini_compact,
            mini::switch_favorite,
            #[cfg(desktop)]
            updates::get_update_check,
            #[cfg(desktop)]
            updates::set_update_check,
            #[cfg(desktop)]
            updates::get_update,
            #[cfg(desktop)]
            updates::skip_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Soundfolk")
        .run(|app, event| {
            // The last window closing does not end the app (no exit code): it goes on in the tray,
            // when there is one to come back from
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = &event
                && tray::available()
            {
                api.prevent_exit();
            }
            if let tauri::RunEvent::Exit = event {
                mini::save_position(app);
                if let Err(e) = app.state::<Database>().end_session() {
                    tracing::warn!("listening session not closed: {e}");
                }
            }
        });
}
