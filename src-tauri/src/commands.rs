// SPDX-License-Identifier: MPL-2.0
//! Tauri commands exposed to the frontend (`invoke("play_stream", { url })`, ...).
//!
//! They are `async` so they run off the main thread: opening the audio output
//! can take a few hundred ms and must not freeze the window.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::audio::{
    self, AudioEngine, DesktopAudioEngine, NowPlaying, PlayerState, RecipeTest, StationRef, TitleStatus, WebTitles,
};
use crate::recipes::{self, Origin};
use crate::catalog::{CatalogSource, SearchPage, SearchQuery};
use crate::db::{Database, Favorite, HistoryEntry, HistorySummary, LikedTrack, PersonalStation, ReportType, Stats};
use crate::export;
use crate::error::AppResult;
use crate::covers::{self, Cover, CoverSource, Covers};
use crate::musicbrainz::{IdentifyWaker, SETTING_IDENTIFY};
use crate::track_services::{self, CustomService};

/// Station being played, as the history records it.
#[derive(Debug, Clone)]
pub struct OnAir {
    pub id: String,
    pub name: Option<String>,
    /// Stream URL, to play the station again from the system media controls.
    pub url: String,
}

/// The trait takes `&mut self`: the engine lives behind a `Mutex` in the Tauri state.
pub struct AudioState {
    engine: Mutex<DesktopAudioEngine>,
    /// Station currently playing, shared with the listener that logs titles.
    station: Arc<Mutex<Option<OnAir>>>,
    /// Incremented on every station start: a delayed click count checks that it is unchanged.
    plays: AtomicU64,
    /// Catalog station to report, waiting for its first sound (`(play, station_id)`).
    pending_click: Mutex<Option<(u64, String)>>,
}

impl AudioState {
    pub fn new(engine: DesktopAudioEngine, station: Arc<Mutex<Option<OnAir>>>) -> Self {
        Self { engine: Mutex::new(engine), station, plays: AtomicU64::new(0), pending_click: Mutex::new(None) }
    }

    pub(crate) fn engine(&self) -> std::sync::MutexGuard<'_, DesktopAudioEngine> {
        self.engine.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Last station played (kept after a stop).
    pub(crate) fn station(&self) -> Option<OnAir> {
        self.station.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

// ── Audio ────────────────────────────────────────────────────────────────────

/// Station catalog (today Radio-Browser, behind `CatalogSource`).
pub struct CatalogState(pub Arc<dyn CatalogSource>);

/// Setting: report to the catalog that a station is being played (enabled by default).
const SETTING_CLICK_COUNTING: &str = "click_counting";
/// Read the title of streams that do not announce it from the web (station page, server status).
pub const SETTING_WEB_TITLES: &str = "web_titles";

/// Services listed under "Find this track on", by id. Unset: the frontend's defaults.
const SETTING_TRACK_SERVICES: &str = "track_services";
/// Sites the user added to that menu (JSON list of `CustomService`).
const SETTING_CUSTOM_TRACK_SERVICES: &str = "custom_track_services";
/// Volume the user last chose (`0.0..=1.0`), restored at the next launch.
const SETTING_VOLUME: &str = "volume";

/// Web title sources, to turn them on or off from the settings.
pub struct WebTitlesState(pub Arc<WebTitles>);
/// True once the click-counting notice has been shown to the user.
const SETTING_CLICK_NOTICE_SEEN: &str = "click_notice_seen";
/// A station is only reported once it has been audible this long: zapping through
/// stations (with the media keys, for instance) does not count.
const CLICK_LISTEN_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
pub struct PlayOutcome {
    /// The frontend must show the click-counting notice (once only).
    pub show_click_notice: bool,
}

#[tauri::command]
pub async fn play_stream(
    app: AppHandle,
    url: String,
    station_id: Option<String>,
    station_name: Option<String>,
) -> AppResult<PlayOutcome> {
    start_station(&app, url, station_id, station_name)
}

/// Plays a station, from the window or from the system media controls.
pub(crate) fn start_station(
    app: &AppHandle,
    url: String,
    station_id: Option<String>,
    station_name: Option<String>,
) -> AppResult<PlayOutcome> {
    let audio = app.state::<AudioState>();
    let db = app.state::<Database>();
    // Without a catalog ID, the URL identifies the station in the history.
    let id = station_id.clone().unwrap_or_else(|| url.clone());
    let name = station_name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    // Lets a recipe recognise the station by its Radio-Browser UUID.
    let station = StationRef { id: station_id.clone().filter(|id| is_catalog_id(id)) };
    *audio.station.lock().unwrap_or_else(|p| p.into_inner()) = Some(OnAir { id, name, url: url.clone() });
    app.state::<CoversState>().0.set_station(station.id.clone());
    let play = audio.plays.fetch_add(1, Ordering::SeqCst) + 1;
    crate::favorite_repair::watch(app, station_id.as_deref(), &url);

    // Decided before playing: the first sound may come before `play` returns.
    let mut show_click_notice = false;
    let mut pending_click = None;
    if let Some(id) = station_id.filter(|id| is_catalog_id(id)) {
        if db.get_bool(SETTING_CLICK_COUNTING, true)? {
            if db.get_bool(SETTING_CLICK_NOTICE_SEEN, false)? {
                pending_click = Some((play, id));
            } else {
                // Nothing is sent until the user has seen the notice: this first
                // click is therefore not counted.
                db.set_bool(SETTING_CLICK_NOTICE_SEEN, true)?;
                show_click_notice = true;
            }
        }
    }
    *audio.pending_click.lock().unwrap_or_else(|p| p.into_inner()) = pending_click;
    // A recipe saved since the last check applies from this playback.
    app.state::<WebTitlesState>().0.reload_recipes();
    audio.engine().play(&url, station)?;
    Ok(PlayOutcome { show_click_notice })
}

/// Called on every player state change: the listening delay before a click is reported starts
/// with the first sound, not when the station is chosen (connecting can take several seconds).
/// Resuming after a pause does not start it again.
pub(crate) fn schedule_click(app: &AppHandle, state: &PlayerState) {
    if *state != PlayerState::Playing {
        return;
    }
    let Some(audio) = app.try_state::<AudioState>() else { return };
    let Some((play, station_id)) = audio.pending_click.lock().unwrap_or_else(|p| p.into_inner()).take() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLICK_LISTEN_DELAY).await;
        count_click_if_still_on_air(&app, play, &station_id).await;
    });
}

/// Reports the station if `play` is still the current playback and it is audible (or paused by
/// the user), and if counting has not been turned off in the meantime.
async fn count_click_if_still_on_air(app: &AppHandle, play: u64, station_id: &str) {
    let audio = app.state::<AudioState>();
    let listening = matches!(audio.engine().state(), PlayerState::Playing | PlayerState::Paused);
    if audio.plays.load(Ordering::SeqCst) != play || !listening {
        return;
    }
    if !app.state::<Database>().get_bool(SETTING_CLICK_COUNTING, true).unwrap_or(false) {
        return;
    }
    let catalog = app.state::<CatalogState>().0.clone();
    if let Err(e) = catalog.count_click(station_id).await {
        tracing::debug!("failed to count click: {e}");
    }
}

/// Catalog stations have a UUID; personal stations (`user:…`) are never reported.
pub(crate) fn is_catalog_id(id: &str) -> bool {
    !id.starts_with("user:") && id.len() == 36
}

#[tauri::command]
pub async fn stop_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().stop()?)
}

#[tauri::command]
pub async fn pause_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().pause()?)
}

#[tauri::command]
pub async fn resume_stream(audio: State<'_, AudioState>) -> AppResult<()> {
    Ok(audio.engine().resume()?)
}

#[tauri::command]
pub async fn set_volume(audio: State<'_, AudioState>, volume: f32) -> AppResult<()> {
    Ok(audio.engine().set_volume(volume)?)
}

/// Volume saved by `save_volume`, or `None` if the user never moved the slider.
#[tauri::command]
pub async fn get_volume(db: State<'_, Database>) -> AppResult<Option<f32>> {
    let saved = db.get_str(SETTING_VOLUME)?.and_then(|v| v.parse::<f32>().ok());
    Ok(saved.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)))
}

/// Remembers the volume for the next launch. Called once the slider is released, not on every
/// step of a drag: concurrent commands could otherwise land out of order and keep a stale value.
#[tauri::command]
pub async fn save_volume(db: State<'_, Database>, volume: f32) -> AppResult<()> {
    if !volume.is_finite() {
        return Err("Invalid volume".to_owned().into());
    }
    Ok(db.set_str(SETTING_VOLUME, &volume.clamp(0.0, 1.0).to_string())?)
}

#[tauri::command]
pub async fn get_player_state(audio: State<'_, AudioState>) -> AppResult<PlayerState> {
    Ok(audio.engine().state())
}

/// Last station played, for a window opened again while the app kept playing in the background.
#[tauri::command]
pub async fn get_current_station(audio: State<'_, AudioState>) -> AppResult<Option<ResolvedStation>> {
    Ok(audio.station().map(|s| ResolvedStation {
        name: s.name.unwrap_or_else(|| s.url.clone()),
        id: s.id,
        url: s.url,
        genre: None,
    }))
}

#[tauri::command]
pub async fn get_now_playing(audio: State<'_, AudioState>) -> AppResult<NowPlaying> {
    Ok(audio.engine().now_playing())
}

#[tauri::command]
pub async fn get_click_counting(db: State<'_, Database>) -> AppResult<bool> {
    Ok(db.get_bool(SETTING_CLICK_COUNTING, true)?)
}

#[tauri::command]
pub async fn set_click_counting(db: State<'_, Database>, enabled: bool) -> AppResult<()> {
    // If the user sets counting themselves, the notice no longer needs to be shown.
    db.set_bool(SETTING_CLICK_NOTICE_SEEN, true)?;
    Ok(db.set_bool(SETTING_CLICK_COUNTING, enabled)?)
}

#[tauri::command]
pub async fn get_web_titles(db: State<'_, Database>) -> AppResult<bool> {
    Ok(db.get_bool(SETTING_WEB_TITLES, true)?)
}

/// Applies from the next station played.
#[tauri::command]
pub async fn set_web_titles(db: State<'_, Database>, web: State<'_, WebTitlesState>, enabled: bool) -> AppResult<()> {
    db.set_bool(SETTING_WEB_TITLES, enabled)?;
    web.0.set_enabled(enabled);
    Ok(())
}

// ── Recipes ──────────────────────────────────────────────────────────────────

/// Where the title of the current playback comes from.
#[tauri::command]
pub async fn get_title_status(web: State<'_, WebTitlesState>) -> AppResult<TitleStatus> {
    Ok(web.0.status())
}

/// Picks up recipe files saved since the last check (called when the window gets the focus
/// back, typically from the editor): no polling of the folder.
#[tauri::command]
pub async fn reload_recipes(web: State<'_, WebTitlesState>) -> AppResult<()> {
    web.0.reload_recipes();
    Ok(())
}

/// Reads the station's recipe page now and says what it finds.
#[tauri::command]
pub async fn test_recipe(web: State<'_, WebTitlesState>, url: String, station_id: Option<String>) -> AppResult<RecipeTest> {
    web.0.reload_recipes();
    let station = StationRef { id: station_id.filter(|id| is_catalog_id(id)) };
    Ok(web.0.test(&station, &url).await)
}

/// Opens the station's recipe in the user's editor, as a local file: the local recipe in use,
/// a copy of the shipped one, the station's local file that does not load yet, or a new
/// recipe to fill in. Returns the file's path.
#[tauri::command]
pub async fn edit_recipe(
    app: AppHandle,
    web: State<'_, WebTitlesState>,
    url: String,
    station_id: Option<String>,
    name: Option<String>,
    homepage: Option<String>,
) -> AppResult<String> {
    let recipes = web.0.recipes().clone();
    let dir = recipes_folder(&recipes)?;
    let station = StationRef { id: station_id.filter(|id| is_catalog_id(id)) };
    let set = recipes.current();
    let existing = set.find(station.id.as_deref(), &url).cloned();
    let path = match existing {
        Some(entry) if entry.origin == Origin::Local => entry.path.clone().unwrap_or_default(),
        Some(entry) => {
            let path = dir.join(format!("{}.toml", entry.id));
            if !path.exists() {
                let text = recipes.local_copy_of(&entry).ok_or_else(|| "recipe not found".to_owned())?;
                std::fs::write(&path, text)?;
            }
            path
        }
        None => match web.0.local_error_for(&station, &url) {
            Some(broken) => broken.path.into(),
            None => {
                let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
                let host = audio::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_owned));
                let base = recipes::slug(name.as_deref().or(host.as_deref()).unwrap_or("station"));
                let path = (1..)
                    .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
                    .filter(|id| !recipes.is_known_id(id))
                    .map(|id| dir.join(format!("{id}.toml")))
                    .find(|path| !path.exists())
                    .expect("an unused name");
                let template = recipes::template(&recipes::Template {
                    name: name.as_deref().or(host.as_deref()).unwrap_or("Station"),
                    station_id: station.id.as_deref(),
                    stream: &url,
                    homepage: homepage.as_deref(),
                });
                std::fs::write(&path, template)?;
                path
            }
        },
    };
    // Written by the app, not by the user: no "saved" notice for it.
    recipes.reload_if_changed();
    open_path(&app, &path)?;
    Ok(path.display().to_string())
}

/// What *Share this recipe* did.
#[derive(Debug, Serialize)]
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
pub async fn share_recipe(
    app: AppHandle,
    web: State<'_, WebTitlesState>,
    url: String,
    station_id: Option<String>,
) -> AppResult<SharedRecipe> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    use tauri_plugin_opener::OpenerExt;

    web.0.reload_recipes();
    let recipes = web.0.recipes().clone();
    let station = StationRef { id: station_id.filter(|id| is_catalog_id(id)) };
    let entry = recipes.current().find(station.id.as_deref(), &url).cloned().ok_or_else(|| "No recipe matches this station.".to_owned())?;
    let path = match (&entry.origin, &entry.path) {
        (Origin::Local, Some(path)) => path.clone(),
        _ => return Err("Only your own recipes can be shared: edit this one locally first.".to_owned().into()),
    };
    let text = std::fs::read_to_string(&path)?;
    let page_url = entry.recipe.fetch.url.clone();
    let (page, date) = web.0.read_page(&page_url).await.map_err(|e| format!("Could not read the page: {e}."))?;
    let at = date.unwrap_or_else(soundfolk_nowplaying::Timestamp::now);
    let shared = soundfolk_nowplaying::fixture::prepare(&entry.id, &text, &entry.recipe, &page_url, &page, at)
        .map_err(|e| format!("Not ready to share: {e}."))?;

    // A copy laid out as in the repository, for a pull request by hand.
    let folder = recipes_folder(&recipes)?.join("to-share");
    std::fs::create_dir_all(folder.join("stations"))?;
    std::fs::create_dir_all(folder.join("fixtures"))?;
    std::fs::write(folder.join("stations").join(format!("{}.toml", entry.id)), &shared.recipe)?;
    std::fs::write(folder.join("fixtures").join(&shared.fixture_file), &shared.fixture)?;

    let name = &entry.recipe.name;
    let mut link = recipes::share_link(&entry.id, name, &shared.recipe, Some(&shared.fixture));
    let fixture_in_clipboard = link.len() > recipes::MAX_LINK;
    if fixture_in_clipboard {
        app.clipboard()
            .write_text(shared.fixture.clone())
            .map_err(|e| format!("could not copy the excerpt: {e}"))?;
        link = recipes::share_link(&entry.id, name, &shared.recipe, None);
    }
    app.opener().open_url(&link, None::<&str>).map_err(|e| format!("could not open the browser: {e}"))?;
    Ok(SharedRecipe { id: entry.id, folder: folder.display().to_string(), fixture_in_clipboard })
}

#[derive(Debug, Serialize)]
pub struct LocalRecipes {
    pub folder: Option<String>,
    /// Files in the folder, valid or not.
    pub count: usize,
    pub errors: Vec<LocalRecipeError>,
}

#[derive(Debug, Serialize)]
pub struct LocalRecipeError {
    pub file: String,
    pub error: String,
}

/// The user's own recipes, for the settings.
#[tauri::command]
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
                file: e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                error: e.error.clone(),
            })
            .collect(),
    })
}

/// Opens the local recipes folder in the file manager (created if needed).
#[tauri::command]
pub async fn open_recipes_folder(app: AppHandle, web: State<'_, WebTitlesState>) -> AppResult<()> {
    let dir = recipes_folder(web.0.recipes())?;
    open_path(&app, &dir)
}

/// The local recipes folder, created with the JSON Schema next to the recipes (completion and
/// checks in editors that read the `#:schema` line).
fn recipes_folder(recipes: &recipes::Recipes) -> AppResult<std::path::PathBuf> {
    let dir = recipes.local_dir().ok_or_else(|| "no configuration folder on this system".to_owned())?.to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let schema = serde_json::to_string_pretty(&soundfolk_nowplaying::recipe::json_schema()).expect("schema serializes");
    let schema_path = dir.join(recipes::SCHEMA_FILE);
    if std::fs::read_to_string(&schema_path).ok().as_deref() != Some(schema.as_str()) {
        std::fs::write(schema_path, schema)?;
    }
    Ok(dir)
}

fn open_path(app: &AppHandle, path: &std::path::Path) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| crate::error::AppError::from(format!("could not open {}: {e}", path.display())))
}

/// Ids of the services shown in a track's menu, or `None` if the user never chose.
#[tauri::command]
pub async fn get_track_services(db: State<'_, Database>) -> AppResult<Option<Vec<String>>> {
    Ok(db.get_list(SETTING_TRACK_SERVICES)?)
}

#[tauri::command]
pub async fn set_track_services(db: State<'_, Database>, ids: Vec<String>) -> AppResult<()> {
    let ids: Vec<String> = ids.into_iter().filter(|id| !id.is_empty() && !id.contains(',')).collect();
    Ok(db.set_list(SETTING_TRACK_SERVICES, &ids)?)
}

/// Sites the user added. A stored list that no longer passes the checks is dropped, not trusted.
#[tauri::command]
pub async fn get_custom_track_services(db: State<'_, Database>) -> AppResult<Vec<CustomService>> {
    let Some(json) = db.get_str(SETTING_CUSTOM_TRACK_SERVICES)? else { return Ok(Vec::new()) };
    let stored: Vec<CustomService> = serde_json::from_str(&json).unwrap_or_default();
    Ok(stored.into_iter().filter_map(|s| track_services::validate(s).ok()).collect())
}

/// Replaces the whole list. Nothing is saved if one site fails the checks.
#[tauri::command]
pub async fn set_custom_track_services(db: State<'_, Database>, services: Vec<CustomService>) -> AppResult<()> {
    let services = track_services::validate_all(services)?;
    let json = serde_json::to_string(&services).map_err(|e| e.to_string())?;
    Ok(db.set_str(SETTING_CUSTOM_TRACK_SERVICES, &json)?)
}

// ── Catalog ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn search_stations(
    catalog: State<'_, CatalogState>,
    query: SearchQuery,
) -> AppResult<SearchPage> {
    Ok(catalog.0.search(&query).await?)
}

/// What the player needs to play a station, whatever its origin.
#[derive(Debug, Serialize)]
pub struct ResolvedStation {
    pub id: String,
    pub name: String,
    pub url: String,
    pub genre: Option<String>,
}

/// Finds how to play a station known only by the ID stored in the history: personal stations
/// from the database, catalog stations from the catalog (one request), and stations played
/// without an ID are identified by their URL already.
#[tauri::command]
pub async fn resolve_station(
    db: State<'_, Database>,
    catalog: State<'_, CatalogState>,
    station_id: String,
    name: Option<String>,
) -> AppResult<ResolvedStation> {
    if let Some(p) = db.personal_station(&station_id)? {
        return Ok(ResolvedStation { id: p.station_id, name: p.name, url: p.stream_url, genre: p.genre });
    }
    if let Some(f) = db.list_favorites()?.into_iter().find(|f| f.station_id == station_id) {
        return Ok(ResolvedStation { id: f.station_id, name: f.name, url: f.stream_url, genre: f.genre });
    }
    if is_catalog_id(&station_id) {
        let station = catalog
            .0
            .station(&station_id)
            .await?
            .ok_or_else(|| "This station is no longer in the catalog".to_owned())?;
        return Ok(ResolvedStation {
            genre: station.tags.first().cloned(),
            id: station.id,
            name: station.name,
            url: station.url,
        });
    }
    if station_id.starts_with("http://") || station_id.starts_with("https://") {
        let name = name.unwrap_or_else(|| station_id.clone());
        return Ok(ResolvedStation { id: station_id.clone(), name, url: station_id, genre: None });
    }
    Err(format!("Unknown station: {station_id}").into())
}

// ── Personal stations (private, never sent) ──────────────────────────────────

/// Adds a station from a URL (direct stream, `.m3u` or `.pls`). The stream is **tested**
/// (connection, detection, first decoded packet) before being saved.
#[tauri::command]
pub async fn add_personal_station(
    db: State<'_, Database>,
    url: String,
    name: String,
) -> AppResult<PersonalStation> {
    let name = name.trim();
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

    let stream = audio::resolve_stream_url(url).await?;
    let handle = tokio::runtime::Handle::current();
    let probed = stream.clone();
    tokio::task::spawn_blocking(move || audio::probe_stream(&probed, &handle))
        .await
        .map_err(|e| format!("Stream test interrupted: {e}"))??;

    Ok(db.add_personal_station(name, stream.as_str(), None)?)
}

#[tauri::command]
pub async fn list_personal_stations(db: State<'_, Database>) -> AppResult<Vec<PersonalStation>> {
    Ok(db.list_personal_stations()?)
}

#[tauri::command]
pub async fn remove_personal_station(
    db: State<'_, Database>,
    station_id: String,
) -> AppResult<bool> {
    Ok(db.remove_personal_station(&station_id)?)
}

// ── Favorites ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_favorite(db: State<'_, Database>, favorite: Favorite) -> AppResult<()> {
    Ok(db.add_favorite(&favorite)?)
}

#[tauri::command]
pub async fn remove_favorite(db: State<'_, Database>, station_id: String) -> AppResult<bool> {
    Ok(db.remove_favorite(&station_id)?)
}

#[tauri::command]
pub async fn list_favorites(db: State<'_, Database>) -> AppResult<Vec<Favorite>> {
    Ok(db.list_favorites()?)
}

// ── History & reports ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_listening_history(
    db: State<'_, Database>,
    limit: Option<u32>,
    query: Option<String>,
) -> AppResult<Vec<HistoryEntry>> {
    Ok(db.recent_history(limit.unwrap_or(50).min(500), query.as_deref())?)
}

#[tauri::command]
pub async fn get_history_summary(db: State<'_, Database>) -> AppResult<HistorySummary> {
    Ok(db.history_summary()?)
}

#[tauri::command]
pub async fn clear_listening_history(db: State<'_, Database>) -> AppResult<usize> {
    Ok(db.clear_history()?)
}

#[derive(Debug, Serialize)]
pub struct ExportedFile {
    pub path: String,
    /// File name alone, for a short notice.
    pub file_name: String,
    pub count: usize,
}

/// Writes the whole history or all liked tracks (narrowed by `query`, as in the History tab)
/// to a new file in the Downloads folder, then shows it in the file manager.
#[tauri::command]
pub async fn export_history(
    app: AppHandle,
    db: State<'_, Database>,
    list: export::List,
    format: export::Format,
    query: Option<String>,
) -> AppResult<ExportedFile> {
    let (content, count) = match list {
        export::List::History => {
            let entries = db.recent_history(u32::MAX, query.as_deref())?;
            (export::history(&entries, format), entries.len())
        }
        export::List::Likes => {
            let tracks = db.list_likes(u32::MAX, query.as_deref())?;
            (export::likes(&tracks, format), tracks.len())
        }
    };
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("no folder to save the export in: {e}"))?;
    std::fs::create_dir_all(&dir)?;
    let date = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
    let base = format!("{}-{date}", list.file_stem());
    let path = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .map(|name| dir.join(format!("{name}.{}", format.extension())))
        .find(|path| !path.exists())
        .expect("an unused name");
    std::fs::write(&path, content)?;
    {
        use tauri_plugin_opener::OpenerExt;
        // The file is saved either way; a file manager that cannot be opened is not an error.
        if let Err(e) = app.opener().reveal_item_in_dir(&path) {
            tracing::warn!("could not show {} in the file manager: {e}", path.display());
        }
    }
    Ok(ExportedFile {
        file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.display().to_string(),
        count,
    })
}

// ── Likes ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn like_track(
    db: State<'_, Database>,
    waker: State<'_, IdentifyWaker>,
    history_id: i64,
) -> AppResult<bool> {
    let liked = db.like_track(history_id)?;
    waker.0.notify_one();
    Ok(liked)
}

/// Stats over the last `days` days, or all time.
#[tauri::command]
pub async fn get_stats(db: State<'_, Database>, days: Option<u32>) -> AppResult<Stats> {
    Ok(db.stats(days)?)
}

/// Whether liked tracks are identified on MusicBrainz (off by default).
#[tauri::command]
pub async fn get_track_identification(db: State<'_, Database>) -> AppResult<bool> {
    Ok(db.get_bool(SETTING_IDENTIFY, false)?)
}

#[tauri::command]
pub async fn set_track_identification(
    db: State<'_, Database>,
    waker: State<'_, IdentifyWaker>,
    enabled: bool,
) -> AppResult<()> {
    db.set_bool(SETTING_IDENTIFY, enabled)?;
    waker.0.notify_one();
    Ok(())
}

/// Cover art of the track on air, chosen from the sources the user put in order.
pub struct CoversState(pub Arc<Covers>);

#[tauri::command]
pub async fn get_cover_sources(db: State<'_, Database>) -> AppResult<Vec<CoverSource>> {
    Ok(covers::load_sources(&db)?)
}

/// Saves the order and switches, and chooses the cover of the track on air again.
#[tauri::command]
pub async fn set_cover_sources(
    db: State<'_, Database>,
    covers: State<'_, CoversState>,
    sources: Vec<CoverSource>,
) -> AppResult<Vec<CoverSource>> {
    let saved = covers::save_sources(&db, sources)?;
    covers.0.refresh();
    Ok(saved)
}

/// The cover shown for the track on air (the window may open while a track is playing).
#[tauri::command]
pub async fn get_cover(covers: State<'_, CoversState>) -> AppResult<Cover> {
    Ok(covers.0.current())
}

#[tauri::command]
pub async fn unlike_track(db: State<'_, Database>, track_key: String) -> AppResult<bool> {
    Ok(db.unlike_track(&track_key)?)
}

#[tauri::command]
pub async fn list_likes(
    db: State<'_, Database>,
    limit: Option<u32>,
    query: Option<String>,
) -> AppResult<Vec<LikedTrack>> {
    Ok(db.list_likes(limit.unwrap_or(200).min(1000), query.as_deref())?)
}

#[tauri::command]
pub async fn report_station(
    db: State<'_, Database>,
    station_id: String,
    report_type: ReportType,
    comment: Option<String>,
) -> AppResult<i64> {
    Ok(db.add_report(&station_id, report_type, comment.as_deref())?)
}
