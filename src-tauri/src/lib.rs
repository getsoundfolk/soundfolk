// SPDX-License-Identifier: MPL-2.0
// Tests may build their own HTTP clients and blocking tasks (local servers, no debug check needed):
// `clippy.toml` refuses them only in the app.
#![cfg_attr(test, allow(clippy::disallowed_methods))]
mod audio;
mod bindings;
mod catalog;
mod commands;
mod covers;
mod curated_lists;
mod daily_list;
mod db;
mod donations;
mod error;
mod events;
mod export;
mod favorite_repair;
mod files;
mod http;
mod listening;
mod media_controls;
mod mini;
mod musicbrainz;
mod notifications;
mod now_playing;
mod player;
mod recipe_list;
mod recipes;
mod share;
mod state;
mod track_services;
mod tracks;
mod tray;
#[cfg(desktop)]
mod updates;
mod volume;
mod windows;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

use audio::{DesktopAudioEngine, NowPlaying};
use catalog::radio_browser::RadioBrowser;
use covers::Covers;
use db::Database;
use media_controls::SystemMedia;
use musicbrainz::MusicBrainz;
use now_playing::{WebEvent, WebTitles};
use player::{AudioState, OnAir};
use state::CatalogState;

/// Argument of the launch at login (Settings): the app starts in the notification area, without
/// its window, and does not play.
const HIDDEN_ARG: &str = "--hidden";

/// A folder in the app's cache, if the platform gives one.
fn cache_dir(app: &tauri::App, name: &str) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|dir| dir.join(name))
}

/// Emits a payload-less event, logging a failure.
fn announce(app: &tauri::AppHandle, event: &str) {
    if let Err(e) = app.emit(event, ()) {
        tracing::warn!("failed to emit {event}: {e}");
    }
}

/// The web titles (station recipes): the bundled and local recipes, the recipe list kept up to
/// date in the background, and their events sent to the window.
fn start_web_titles(app: &tauri::App) -> Arc<WebTitles> {
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
            .flag(db::settings::WEB_TITLES)
            .unwrap_or_else(|e| {
                tracing::warn!("web titles setting not read: {e}");
                true
            }),
        Box::new(move |event| {
            let sent = match event {
                WebEvent::Status(status) => events.emit(crate::events::TITLE_STATUS, status),
                WebEvent::Notice(notice) => events.emit(crate::events::RECIPE_NOTICE, notice),
            };
            if let Err(e) = sent {
                tracing::warn!("failed to emit a recipe event: {e}");
            }
        }),
    ));
    app.manage(state::WebTitlesState(web_titles.clone()));
    recipe_list::start(web_titles.clone(), cache_dir(app, "recipes"));
    web_titles
}

/// What is downloaded in the background: donation links, curated lists and, on desktop, updates.
fn start_downloads(app: &tauri::App) {
    let donations = Arc::new(donations::Donations::default());
    app.manage(donations.clone());
    let handle = app.handle().clone();
    donations::start(donations, cache_dir(app, "stations"), move || {
        announce(&handle, events::DONATIONS_UPDATED)
    });

    let lists = Arc::new(curated_lists::CuratedLists::default());
    app.manage(lists.clone());
    let handle = app.handle().clone();
    curated_lists::start(lists, cache_dir(app, "stations"), move || {
        announce(&handle, events::LISTS_UPDATED)
    });

    #[cfg(desktop)]
    {
        let updates = Arc::new(updates::Updates::new(app.handle()));
        app.manage(updates.clone());
        updates::start(app.handle(), updates, cache_dir(app, "updates"));
    }
}

/// The audio engine, and everything that follows its state: favorite repair, media controls,
/// listening journal, click counting, tray and window.
fn start_player(
    app: &tauri::App,
    on_air: Arc<Mutex<Option<OnAir>>>,
    media: Arc<SystemMedia>,
    covers: Arc<Covers>,
    web_titles: Arc<WebTitles>,
) -> tauri::Result<()> {
    let journal = listening::Journal::start(app.handle().clone(), covers, on_air.clone())?;
    let state_journal = journal.clone();
    let state_handle = app.handle().clone();
    let engine = DesktopAudioEngine::new(
        Arc::new(move |state| {
            let state = favorite_repair::on_state(&state_handle, state);
            let state = media_controls::on_state(&state_handle, state);
            state_journal.state(&state);
            player::schedule_click(&state_handle, &state);
            media.show_state(&state);
            tray::show_state(&state_handle, &state);
            if let Err(e) = state_handle.emit(events::PLAYER_STATE, state) {
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
    Ok(())
}

/// The tray icon, and the window used last unless the app was launched at login. Without an icon,
/// the main window: the mini player has no title bar to quit from.
fn open_windows(app: &tauri::App) -> tauri::Result<()> {
    // Launched at login: no window, so no web view, until the icon is clicked. If the icon turns
    // out to show nowhere, the window opens after all (`tray::set_unavailable`).
    let hidden = std::env::args().skip(1).any(|arg| arg == HIDDEN_ARG);
    if hidden {
        tray::show_window_state(app.handle(), false);
    }
    // What the menu greys out: Play without a station, Previous and Next without two favorites
    let has_station = app
        .try_state::<AudioState>()
        .is_some_and(|audio| audio.station().is_some());
    let favorites = app
        .state::<Database>()
        .list_favorites()
        .map(|favorites| favorites.len())
        .unwrap_or_else(|e| {
            tracing::warn!("favorites not counted for the tray: {e}");
            0
        });
    tray::show_launch_state(has_station, favorites);
    // Without an icon, closing the window quits: there would be no way back to it
    if let Err(e) = tray::init(app) {
        tracing::warn!("no notification area icon: {e}");
        tray::set_available(false);
    }
    if !tray::available() {
        windows::open_main_window(app.handle())?;
    } else if hidden {
        // Show (tray) still needs to know which window to bring back.
        mini::read_last(app.handle());
    } else {
        windows::open_last_window(app.handle())?;
    }
    Ok(())
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db = Database::open(data_dir.join("soundfolk.db"))?;
    db.reparse_history_in_background();
    app.manage(db);
    app.manage(windows::WindowGeometry::default());
    app.manage(share::OpenedLink::default());
    #[cfg(desktop)]
    share::watch_links(app);

    // The last station is in the player from launch: Play (window, tray, media keys) starts it.
    let on_air = Arc::new(Mutex::new(player::saved_station(&app.state::<Database>())));

    let media = Arc::new(SystemMedia::new(app.handle().clone()));
    let musicbrainz = MusicBrainz::new()
        .map(Arc::new)
        .inspect_err(|e| tracing::warn!("MusicBrainz lookups disabled: {e}"))
        .ok();
    let covers = Covers::new(app.handle().clone(), musicbrainz.clone(), media.clone());
    app.manage(state::CoversState(covers.clone()));
    app.manage(notifications::TrackNotifications::new(
        app.handle(),
        &app.state::<Database>(),
        covers::app_icon_file(app.handle()),
    ));
    let web_titles = start_web_titles(app);
    start_downloads(app);
    start_player(app, on_air, media, covers, web_titles)?;
    app.manage(CatalogState(Arc::new(RadioBrowser::default())));

    let identify = Arc::new(tokio::sync::Notify::new());
    if let Some(mb) = musicbrainz {
        musicbrainz::spawn_worker(app.handle().clone(), mb, identify.clone());
    }
    app.manage(musicbrainz::IdentifyWaker(identify));

    open_windows(app)?;
    Ok(())
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
    // (which keeps playing and shows the window used last) and exits before opening the database or the audio
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_last_window(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ));

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(setup)
        .on_window_event(windows::on_window_event)
        .invoke_handler(bindings::builder().invoke_handler())
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
