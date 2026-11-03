// SPDX-License-Identifier: MPL-2.0
//! Tauri commands exposed to the frontend (`invoke("play_stream", { url })`, ...).
//!
//! They are `async` so they run off the main thread: opening the audio output
//! can take a few hundred ms and must not freeze the window.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

use crate::audio::{self, AudioEngine, DesktopAudioEngine, NowPlaying, PlayerState};
use crate::catalog::{CatalogSource, SearchPage, SearchQuery};
use crate::db::{Database, Favorite, HistoryEntry, LikedTrack, PersonalStation, ReportType};
use crate::error::AppResult;

/// Station being played, as the history records it.
#[derive(Debug, Clone)]
pub struct OnAir {
    pub id: String,
    pub name: Option<String>,
}

/// The trait takes `&mut self`: the engine lives behind a `Mutex` in the Tauri state.
pub struct AudioState {
    engine: Mutex<DesktopAudioEngine>,
    /// Station currently playing, shared with the listener that logs titles.
    station: Arc<Mutex<Option<OnAir>>>,
}

impl AudioState {
    pub fn new(engine: DesktopAudioEngine, station: Arc<Mutex<Option<OnAir>>>) -> Self {
        Self { engine: Mutex::new(engine), station }
    }

    fn engine(&self) -> std::sync::MutexGuard<'_, DesktopAudioEngine> {
        self.engine.lock().unwrap_or_else(|p| p.into_inner())
    }
}

// ── Audio ────────────────────────────────────────────────────────────────────

/// Station catalog (today Radio-Browser, behind `CatalogSource`).
pub struct CatalogState(pub Arc<dyn CatalogSource>);

/// Setting: report to the catalog that a station is being played (enabled by default).
const SETTING_CLICK_COUNTING: &str = "click_counting";
/// True once the click-counting notice has been shown to the user.
const SETTING_CLICK_NOTICE_SEEN: &str = "click_notice_seen";

#[derive(Debug, Serialize)]
pub struct PlayOutcome {
    /// The frontend must show the click-counting notice (once only).
    pub show_click_notice: bool,
}

#[tauri::command]
pub async fn play_stream(
    audio: State<'_, AudioState>,
    db: State<'_, Database>,
    catalog: State<'_, CatalogState>,
    url: String,
    station_id: Option<String>,
    station_name: Option<String>,
) -> AppResult<PlayOutcome> {
    // Without a catalog ID, the URL identifies the station in the history.
    let id = station_id.clone().unwrap_or_else(|| url.clone());
    let name = station_name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    *audio.station.lock().unwrap_or_else(|p| p.into_inner()) = Some(OnAir { id, name });
    audio.engine().play(&url)?;

    // Counting is never on the critical path: started after playback has begun.
    let mut show_click_notice = false;
    if let Some(id) = station_id.filter(|id| is_catalog_id(id)) {
        if db.get_bool(SETTING_CLICK_COUNTING, true)? {
            if db.get_bool(SETTING_CLICK_NOTICE_SEEN, false)? {
                let catalog = catalog.0.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = catalog.count_click(&id).await {
                        tracing::debug!("failed to count click: {e}");
                    }
                });
            } else {
                // Nothing is sent until the user has seen the notice: this first
                // click is therefore not counted.
                db.set_bool(SETTING_CLICK_NOTICE_SEEN, true)?;
                show_click_notice = true;
            }
        }
    }
    Ok(PlayOutcome { show_click_notice })
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
pub async fn like_track(db: State<'_, Database>, history_id: i64) -> AppResult<bool> {
    Ok(db.like_track(history_id)?)
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
