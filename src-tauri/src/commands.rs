// SPDX-License-Identifier: MPL-2.0
//! Tauri commands exposed to the frontend (`invoke("play_stream", { url })`, ...): thin adapters
//! over the player ([`crate::player`]), the recipes ([`crate::recipes`]), the database and the
//! catalog.
//!
//! They are `async` so they run off the main thread: opening the audio output can take a few
//! hundred ms and must not freeze the window. Their blocking work goes through
//! [`blocking`] and [`with_db`] (see [`crate::state`]).

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::audio::{self, AudioEngine, NowPlaying, PlayerState, RecipeTest, TitleStatus};
use crate::catalog::{surprise, SearchPage, SearchQuery, Station};
use crate::covers::{self, Cover, CoverSource};
use crate::db::{
    Database, Favorite, HistoryEntry, HistorySummary, LikedTrack, PersonalStation, Stats,
};
use crate::error::AppResult;
use crate::export;
use crate::files::{open_path, save_download, ExportedFile};
use crate::musicbrainz::{IdentifyWaker, SETTING_IDENTIFY};
use crate::player::{self, catalog_ref, AudioState, PlayOutcome, ResolvedStation};
use crate::recipes;
use crate::state::{
    blocking, with_db, CatalogState, CoversState, WebTitlesState, SETTING_WEB_TITLES,
};
use crate::track_services::{self, CustomService};

// ── Audio ────────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn play_stream(
    app: AppHandle,
    url: String,
    station_id: Option<String>,
    station_name: Option<String>,
) -> AppResult<PlayOutcome> {
    blocking(move || player::start_station(&app, url, station_id, station_name)).await
}

#[tauri::command]
#[specta::specta]
pub async fn stop_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().stop()?)
}

#[tauri::command]
#[specta::specta]
pub async fn pause_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().pause()?)
}

#[tauri::command]
#[specta::specta]
pub async fn resume_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().resume()?)
}

#[tauri::command]
#[specta::specta]
pub async fn get_player_state(audio: State<'_, AudioState>) -> AppResult<PlayerState> {
    Ok(audio.engine().state())
}

/// Last station played (in this run or the previous one), for the player to show it ready to play.
#[tauri::command]
#[specta::specta]
pub async fn get_current_station(
    audio: State<'_, AudioState>,
) -> AppResult<Option<ResolvedStation>> {
    Ok(audio.station().map(ResolvedStation::from))
}

#[tauri::command]
#[specta::specta]
pub async fn get_now_playing(audio: State<'_, AudioState>) -> AppResult<NowPlaying> {
    Ok(audio.engine().now_playing())
}
#[tauri::command]
#[specta::specta]
pub async fn get_click_counting(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, player::click_counting).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_click_counting(db: State<'_, Database>, enabled: bool) -> AppResult<()> {
    with_db(&db, move |db| player::set_click_counting(db, enabled)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_web_titles(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| Ok(db.get_bool(SETTING_WEB_TITLES, true)?)).await
}

/// Applies from the next station played.
#[tauri::command]
#[specta::specta]
pub async fn set_web_titles(
    db: State<'_, Database>,
    web: State<'_, WebTitlesState>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.set_bool(SETTING_WEB_TITLES, enabled)?)).await?;
    web.0.set_enabled(enabled);
    Ok(())
}

/// Launch at login, hidden in the notification area. The system keeps the setting (on Linux a file
/// in `~/.config/autostart`), so it is read from there, not from the database.
#[tauri::command]
#[specta::specta]
pub async fn get_autostart(app: AppHandle) -> AppResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    blocking(move || {
        Ok(app
            .autolaunch()
            .is_enabled()
            .map_err(|e| format!("Launch at login unknown: {e}"))?)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> AppResult<()> {
    use tauri_plugin_autostart::ManagerExt;
    blocking(move || {
        let launch = app.autolaunch();
        let changed = if enabled {
            launch.enable()
        } else {
            launch.disable()
        };
        Ok(changed.map_err(|e| format!("Launch at login not changed: {e}"))?)
    })
    .await
}

// ── Recipes ──────────────────────────────────────────────────────────────────

/// Where the title of the current playback comes from.
#[tauri::command]
#[specta::specta]
pub async fn get_title_status(web: State<'_, WebTitlesState>) -> AppResult<TitleStatus> {
    Ok(web.0.status())
}

/// Picks up recipe files saved since the last check (called when the window gets the focus
/// back, typically from the editor): no polling of the folder.
#[tauri::command]
#[specta::specta]
pub async fn reload_recipes(web: State<'_, WebTitlesState>) -> AppResult<()> {
    let web = web.0.clone();
    blocking(move || {
        web.reload_recipes();
        Ok(())
    })
    .await
}

/// Reads the station's recipe page now and says what it finds.
#[tauri::command]
#[specta::specta]
pub async fn test_recipe(
    web: State<'_, WebTitlesState>,
    url: String,
    station_id: Option<String>,
) -> AppResult<RecipeTest> {
    let titles = web.0.clone();
    blocking(move || {
        titles.reload_recipes();
        Ok(())
    })
    .await?;
    let station = catalog_ref(station_id);
    Ok(web.0.test(&station, &url).await)
}

/// Opens the station's recipe in the user's editor, as a local file (see
/// [`recipes::file_to_edit`]). Returns the file's path.
#[tauri::command]
#[specta::specta]
pub async fn edit_recipe(
    app: AppHandle,
    web: State<'_, WebTitlesState>,
    url: String,
    station_id: Option<String>,
    name: Option<String>,
    homepage: Option<String>,
) -> AppResult<String> {
    let web = web.0.clone();
    blocking(move || {
        let station = catalog_ref(station_id);
        let broken = web.local_error_for(&station, &url).map(|e| e.path.into());
        let path = recipes::file_to_edit(
            web.recipes(),
            recipes::EditedStation {
                id: station.id.as_deref(),
                stream: &url,
                name: name.as_deref(),
                homepage: homepage.as_deref(),
                broken,
            },
        )?;
        open_path(&app, &path)?;
        Ok(path.display().to_string())
    })
    .await
}

/// What *Share this recipe* did.
#[derive(Debug, Serialize, specta::Type)]
pub struct SharedRecipe {
    pub id: String,
    /// Where the recipe and fixture were saved, as in the repository (`stations/`, `fixtures/`).
    pub folder: String,
    /// The fixture was too long for the link: it is in the clipboard, to paste in the form.
    pub fixture_in_clipboard: bool,
}

/// Prepares the station's local recipe for the shared list: reads the page now, keeps the
/// excerpt the recipe reads as its fixture, writes the `[[test]]`, and opens the "Share a recipe"
/// issue form filled in. A bot turns the issue into a pull request.
#[tauri::command]
#[specta::specta]
pub async fn share_recipe(
    app: AppHandle,
    web: State<'_, WebTitlesState>,
    url: String,
    station_id: Option<String>,
) -> AppResult<SharedRecipe> {
    let titles = web.0.clone();
    let (entry, text) = blocking(move || {
        titles.reload_recipes();
        let station = catalog_ref(station_id);
        recipes::local_recipe(titles.recipes(), station.id.as_deref(), &url)
    })
    .await?;
    let page_url = entry.recipe.fetch.url.clone();
    let (page, date) = web
        .0
        .read_page(&page_url)
        .await
        .map_err(|e| format!("Could not read the page: {e}."))?;
    let at = date.unwrap_or_else(soundfolk_nowplaying::Timestamp::now);
    let recipes = web.0.recipes().clone();
    blocking(move || {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        use tauri_plugin_opener::OpenerExt;

        let prepared = recipes::prepare_share(&recipes, &entry, &text, &page_url, &page, at)?;
        let fixture_in_clipboard = prepared.fixture_to_paste.is_some();
        if let Some(fixture) = prepared.fixture_to_paste {
            app.clipboard()
                .write_text(fixture)
                .map_err(|e| format!("could not copy the excerpt: {e}"))?;
        }
        app.opener()
            .open_url(&prepared.link, None::<&str>)
            .map_err(|e| format!("could not open the browser: {e}"))?;
        Ok(SharedRecipe {
            id: entry.id,
            folder: prepared.folder.display().to_string(),
            fixture_in_clipboard,
        })
    })
    .await
}

#[derive(Debug, Serialize, specta::Type)]
pub struct LocalRecipes {
    pub folder: Option<String>,
    /// Files in the folder, valid or not.
    pub count: usize,
    pub errors: Vec<LocalRecipeError>,
}

#[derive(Debug, Serialize, specta::Type)]
pub struct LocalRecipeError {
    pub file: String,
    pub error: String,
}

/// The user's own recipes, for the settings.
#[tauri::command]
#[specta::specta]
pub async fn get_local_recipes(web: State<'_, WebTitlesState>) -> AppResult<LocalRecipes> {
    let recipes = web.0.recipes();
    let set = recipes.current();
    Ok(LocalRecipes {
        folder: recipes.local_dir().map(|d| d.display().to_string()),
        count: set.local_count(),
        errors: set
            .errors()
            .iter()
            .map(|e| LocalRecipeError {
                file: e
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                error: e.error.clone(),
            })
            .collect(),
    })
}

/// Opens the local recipes folder in the file manager (created if needed).
#[tauri::command]
#[specta::specta]
pub async fn open_recipes_folder(app: AppHandle, web: State<'_, WebTitlesState>) -> AppResult<()> {
    let web = web.0.clone();
    blocking(move || {
        let dir = recipes::folder(web.recipes())?;
        open_path(&app, &dir)
    })
    .await
}

/// Ids of the services shown in a track's menu, or `None` if the user never chose.
#[tauri::command]
#[specta::specta]
pub async fn get_track_services(db: State<'_, Database>) -> AppResult<Option<Vec<String>>> {
    with_db(&db, track_services::enabled_ids).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_track_services(db: State<'_, Database>, ids: Vec<String>) -> AppResult<()> {
    with_db(&db, move |db| track_services::save_enabled_ids(db, ids)).await
}

/// Sites the user added.
#[tauri::command]
#[specta::specta]
pub async fn get_custom_track_services(db: State<'_, Database>) -> AppResult<Vec<CustomService>> {
    with_db(&db, track_services::load_custom).await
}

/// Replaces the whole list. Nothing is saved if one site fails the checks.
#[tauri::command]
#[specta::specta]
pub async fn set_custom_track_services(
    db: State<'_, Database>,
    services: Vec<CustomService>,
) -> AppResult<()> {
    let services = track_services::validate_all(services)?;
    with_db(&db, move |db| track_services::save_custom(db, &services)).await
}

// ── Catalog ──────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn search_stations(
    catalog: State<'_, CatalogState>,
    db: State<'_, Database>,
    mut query: SearchQuery,
) -> AppResult<SearchPage> {
    query.favorites = with_db(&db, |db| Ok(db.list_favorites()?))
        .await?
        .into_iter()
        .map(|f| f.station_id)
        .collect();
    Ok(catalog.0.search(&query).await?)
}

/// A catalog station by its id, with what the station sheet shows (genres, country, quality…).
/// `None` when the catalog no longer has it.
#[tauri::command]
#[specta::specta]
pub async fn get_station(
    catalog: State<'_, CatalogState>,
    station_id: String,
) -> AppResult<Option<Station>> {
    Ok(catalog.0.station(&station_id).await?)
}

/// Stations drawn in the catalog for Surprise me: up for music, in the order to try them
/// (see [`surprise`]). Can be empty; the frontend then draws again.
#[tauri::command]
#[specta::specta]
pub async fn surprise_candidates(
    catalog: State<'_, CatalogState>,
    exclude: Option<String>,
) -> AppResult<Vec<Station>> {
    let draw = catalog.0.random(SURPRISE_DRAW).await?;
    Ok(surprise::candidates(draw, exclude.as_deref()))
}

/// How many stations one Surprise me request draws: about half pass the filters.
const SURPRISE_DRAW: u32 = 60;
/// Finds how to play a station known only by the ID stored in the history (see
/// [`player::resolve`]).
#[tauri::command]
#[specta::specta]
pub async fn resolve_station(
    db: State<'_, Database>,
    catalog: State<'_, CatalogState>,
    station_id: String,
    name: Option<String>,
) -> AppResult<ResolvedStation> {
    player::resolve(&db, catalog.0.as_ref(), station_id, name).await
}

// ── Personal stations (private, never sent) ──────────────────────────────────

/// Adds a station from a URL (direct stream, `.m3u` or `.pls`). The stream is **tested**
/// (connection, detection, first decoded packet) before being saved. A station received in a
/// share link (`from_link`) may only reach public addresses, then and later (16 §6.2).
#[tauri::command]
#[specta::specta]
pub async fn add_personal_station(
    db: State<'_, Database>,
    url: String,
    name: String,
    from_link: Option<bool>,
) -> AppResult<PersonalStation> {
    let from_link = from_link.unwrap_or(false);
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("A station needs a name.".to_owned().into());
    }
    let url: audio::Url = url
        .trim()
        .parse()
        .map_err(|e| format!("Invalid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("Unsupported protocol: {}", url.scheme()).into());
    }

    let stream = audio::resolve_stream_url(url, from_link).await?;
    let handle = tokio::runtime::Handle::current();
    let probed = stream.clone();
    tokio::task::spawn_blocking(move || audio::probe_stream(&probed, &handle, from_link))
        .await
        .map_err(|e| format!("Stream test interrupted: {e}"))??;

    with_db(&db, move |db| {
        Ok(if from_link {
            db.add_personal_station_from_link(&name, stream.as_str())?
        } else {
            db.add_personal_station(&name, stream.as_str(), None)?
        })
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn list_personal_stations(db: State<'_, Database>) -> AppResult<Vec<PersonalStation>> {
    with_db(&db, |db| Ok(db.list_personal_stations()?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_personal_station(
    db: State<'_, Database>,
    station_id: String,
) -> AppResult<bool> {
    with_db(&db, move |db| Ok(db.remove_personal_station(&station_id)?)).await
}

// ── Station names ──────────────────────────────────────────────────────────────

/// Names the user gave stations, by station ID: shown instead of the catalog's.
#[tauri::command]
#[specta::specta]
pub async fn get_station_names(
    db: State<'_, Database>,
) -> AppResult<std::collections::HashMap<String, String>> {
    with_db(&db, |db| Ok(db.station_names()?)).await
}

/// Names a station; a blank or missing name gives it back its own. Returns the name kept. The
/// media controls follow at once when the station is on air.
#[tauri::command]
#[specta::specta]
pub async fn set_station_name(
    app: AppHandle,
    station_id: String,
    name: Option<String>,
) -> AppResult<Option<String>> {
    blocking(move || {
        let db = app.state::<Database>();
        let kept = db.set_station_name(&station_id, name.as_deref())?;
        let on_air = app.state::<AudioState>().station();
        if on_air.is_some_and(|s| s.id == station_id) {
            app.state::<CoversState>().0.rename(kept.clone());
        }
        Ok(kept)
    })
    .await
}

// ── Favorites ──────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn add_favorite(db: State<'_, Database>, favorite: Favorite) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.add_favorite(&favorite)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_favorite(db: State<'_, Database>, station_id: String) -> AppResult<bool> {
    with_db(&db, move |db| Ok(db.remove_favorite(&station_id)?)).await
}

/// Saves the order the listener gave the favorites (station IDs, top first).
#[tauri::command]
#[specta::specta]
pub async fn reorder_favorites(db: State<'_, Database>, station_ids: Vec<String>) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.reorder_favorites(&station_ids)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_favorites(db: State<'_, Database>) -> AppResult<Vec<Favorite>> {
    with_db(&db, |db| Ok(db.list_favorites()?)).await
}

// ── History & reports ────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn get_listening_history(
    db: State<'_, Database>,
    limit: Option<u32>,
    query: Option<String>,
) -> AppResult<Vec<HistoryEntry>> {
    let limit = limit.unwrap_or(50).min(500);
    with_db(&db, move |db| {
        Ok(db.recent_history(limit, query.as_deref())?)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_history_summary(db: State<'_, Database>) -> AppResult<HistorySummary> {
    with_db(&db, |db| Ok(db.history_summary()?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_listening_history(db: State<'_, Database>) -> AppResult<usize> {
    with_db(&db, |db| Ok(db.clear_history()?)).await
}
/// Writes the whole history or all liked tracks (narrowed by `query`, as in the History tab)
/// to a new file in the Downloads folder, then shows it in the file manager.
#[tauri::command]
#[specta::specta]
pub async fn export_history(
    app: AppHandle,
    db: State<'_, Database>,
    list: export::List,
    format: export::Format,
    query: Option<String>,
) -> AppResult<ExportedFile> {
    let db = db.inner().clone();
    blocking(move || {
        let (content, count) = export::render(&db, list, format, query.as_deref())?;
        save_download(&app, list.file_stem(), format.extension(), &content, count)
    })
    .await
}

// ── Likes ────────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn like_track(
    db: State<'_, Database>,
    waker: State<'_, IdentifyWaker>,
    history_id: i64,
) -> AppResult<bool> {
    let liked = with_db(&db, move |db| Ok(db.like_track(history_id)?)).await?;
    waker.0.notify_one();
    Ok(liked)
}

/// Stats over the last `days` days, or all time.
#[tauri::command]
#[specta::specta]
pub async fn get_stats(db: State<'_, Database>, days: Option<u32>) -> AppResult<Stats> {
    with_db(&db, move |db| Ok(db.stats(days)?)).await
}

/// Whether liked tracks are identified on MusicBrainz (off by default).
#[tauri::command]
#[specta::specta]
pub async fn get_track_identification(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| Ok(db.get_bool(SETTING_IDENTIFY, false)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_track_identification(
    db: State<'_, Database>,
    waker: State<'_, IdentifyWaker>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.set_bool(SETTING_IDENTIFY, enabled)?)).await?;
    waker.0.notify_one();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_cover_sources(db: State<'_, Database>) -> AppResult<Vec<CoverSource>> {
    with_db(&db, |db| Ok(covers::load_sources(db)?)).await
}

/// Saves the order and switches, and chooses the cover of the track on air again.
#[tauri::command]
#[specta::specta]
pub async fn set_cover_sources(
    db: State<'_, Database>,
    covers: State<'_, CoversState>,
    sources: Vec<CoverSource>,
) -> AppResult<Vec<CoverSource>> {
    let saved = with_db(&db, move |db| Ok(covers::save_sources(db, sources)?)).await?;
    covers.0.set_sources(saved.clone());
    Ok(saved)
}

/// The cover shown for the track on air (the window may open while a track is playing).
#[tauri::command]
#[specta::specta]
pub async fn get_cover(covers: State<'_, CoversState>) -> AppResult<Cover> {
    Ok(covers.0.current())
}

#[tauri::command]
#[specta::specta]
pub async fn unlike_track(db: State<'_, Database>, track_key: String) -> AppResult<bool> {
    with_db(&db, move |db| Ok(db.unlike_track(&track_key)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_likes(
    db: State<'_, Database>,
    limit: Option<u32>,
    query: Option<String>,
) -> AppResult<Vec<LikedTrack>> {
    let limit = limit.unwrap_or(200).min(1000);
    with_db(&db, move |db| Ok(db.list_likes(limit, query.as_deref())?)).await
}
