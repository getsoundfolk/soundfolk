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

use crate::audio::{self, AudioEngine, DesktopAudioEngine, NowPlaying, PlayerState, StationRef, WebTitles};
use crate::catalog::{CatalogSource, SearchPage, SearchQuery};
use crate::db::{Database, Favorite, HistoryEntry, LikedTrack, PersonalStation, ReportType, Stats};
use crate::error::AppResult;
use crate::musicbrainz::{IdentifyWaker, SETTING_IDENTIFY};

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
    let play = audio.plays.fetch_add(1, Ordering::SeqCst) + 1;

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
fn is_catalog_id(id: &str) -> bool {
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

#[tauri::command]
pub async fn get_player_state(audio: State<'_, AudioState>) -> AppResult<PlayerState> {
    Ok(audio.engine().state())
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
    name: Option<String>,
) -> AppResult<PersonalStation> {
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
    let info = tokio::task::spawn_blocking(move || audio::probe_stream(&probed, &handle))
        .await
        .map_err(|e| format!("Stream test interrupted: {e}"))??;

    let name = name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or(info.station)
        .or_else(|| stream.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Personal station".to_owned());
    Ok(db.add_personal_station(&name, stream.as_str(), None)?)
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
pub async fn clear_listening_history(db: State<'_, Database>) -> AppResult<usize> {
    Ok(db.clear_history()?)
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
