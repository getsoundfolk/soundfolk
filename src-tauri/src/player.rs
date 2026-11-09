// SPDX-License-Identifier: MPL-2.0
//! The player: the station on air, starting a station (from the window, the media controls or a
//! favorite repair), the click reported to the catalog once a station was heard, and finding how
//! to play a station known only by its id.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::audio::{AudioEngine, DesktopAudioEngine, PlayerState, StationRef};
use crate::catalog::CatalogSource;
use crate::db::Database;
use crate::error::AppResult;
use crate::state::{with_db, CatalogState, CoversState, WebTitlesState};

/// Station being played, as the history records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnAir {
    pub id: String,
    pub name: Option<String>,
    /// Stream URL, to play the station again from the system media controls.
    pub url: String,
}

/// The trait takes `&mut self`: the engine lives behind a `Mutex` in the Tauri state. The lock is
/// held only to send a command, never while the engine opens a sound output: it orders the
/// commands (see `crate::favorite_repair`), it does not wait for them.
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
        Self {
            engine: Mutex::new(engine),
            station,
            plays: AtomicU64::new(0),
            pending_click: Mutex::new(None),
        }
    }

    pub(crate) fn engine(&self) -> std::sync::MutexGuard<'_, DesktopAudioEngine> {
        self.engine.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Last station played (kept after a stop).
    pub(crate) fn station(&self) -> Option<OnAir> {
        self.station
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Setting: report to the catalog that a station is being played (enabled by default).
const SETTING_CLICK_COUNTING: &str = "click_counting";
/// True once the click-counting notice has been shown to the user.
const SETTING_CLICK_NOTICE_SEEN: &str = "click_notice_seen";
/// Last station played (JSON `OnAir`): the app opens with it in the player, ready to play.
const SETTING_LAST_STATION: &str = "last_station";
/// A station is only reported once it has been audible this long: zapping through
/// stations (with the media keys, for instance) does not count.
const CLICK_LISTEN_DELAY: Duration = Duration::from_secs(30);

/// Last station played in a previous run, loaded in the player at launch.
pub(crate) fn saved_station(db: &Database) -> Option<OnAir> {
    let json = db
        .get_str(SETTING_LAST_STATION)
        .inspect_err(|e| tracing::warn!("last station not read: {e}"))
        .ok()??;
    serde_json::from_str(&json)
        .inspect_err(|e| tracing::warn!("last station not restored: {e}"))
        .ok()
}

fn save_station(db: &Database, station: &OnAir) {
    let saved = serde_json::to_string(station)
        .map_err(|e| e.to_string())
        .and_then(|json| {
            db.set_str(SETTING_LAST_STATION, &json)
                .map_err(|e| e.to_string())
        });
    if let Err(e) = saved {
        tracing::warn!("last station not saved: {e}");
    }
}

/// Whether stations played are reported to the catalog.
pub(crate) fn click_counting(db: &Database) -> AppResult<bool> {
    Ok(db.get_bool(SETTING_CLICK_COUNTING, true)?)
}

/// If the user sets counting themselves, the notice no longer needs to be shown.
pub(crate) fn set_click_counting(db: &Database, enabled: bool) -> AppResult<()> {
    let counting = if enabled { "true" } else { "false" };
    Ok(db.set_strs(&[
        (SETTING_CLICK_NOTICE_SEEN, "true"),
        (SETTING_CLICK_COUNTING, counting),
    ])?)
}

#[derive(Debug, Serialize)]
pub struct PlayOutcome {
    /// The frontend must show the click-counting notice (once only).
    pub show_click_notice: bool,
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
    let name = station_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    // Lets a recipe recognise the station by its Radio-Browser UUID.
    let catalog_id = station_id.clone().filter(|id| is_catalog_id(id));
    // Catalog stations and stations received in a link stay off the local network (16 §6.2).
    let public_only = catalog_id.is_some()
        || station_id.as_deref().is_some_and(|id| {
            // Unknown origin: the station keeps the rule rather than lose it on a read error.
            db.personal_station_from_link(id).unwrap_or_else(|e| {
                tracing::warn!("station origin not read, kept off the local network: {e}");
                true
            })
        });
    let station = StationRef {
        id: catalog_id,
        public_only,
    };
    let on_air = OnAir {
        id,
        name,
        url: url.clone(),
    };
    save_station(&db, &on_air);
    *audio.station.lock().unwrap_or_else(|p| p.into_inner()) = Some(on_air);
    app.state::<CoversState>().0.set_station(station.id.clone());
    let play = audio.plays.fetch_add(1, Ordering::SeqCst) + 1;
    crate::favorite_repair::watch(app, station_id.as_deref(), &url);

    // Decided before playing: the first sound may come before `play` returns.
    let mut show_click_notice = false;
    let mut pending_click = None;
    if let Some(id) = station_id.filter(|id| is_catalog_id(id))
        && click_counting(&db)?
    {
        if db.get_bool(SETTING_CLICK_NOTICE_SEEN, false)? {
            pending_click = Some((play, id));
        } else {
            // Nothing is sent until the user has seen the notice: this first
            // click is therefore not counted.
            db.set_bool(SETTING_CLICK_NOTICE_SEEN, true)?;
            show_click_notice = true;
        }
    }
    *audio
        .pending_click
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = pending_click;
    // A recipe saved since the last check applies from this playback.
    app.state::<WebTitlesState>().0.reload_recipes();
    // The engine lock orders the request against the other commands; the answer is awaited
    // without it, so the volume or the player state never wait for a sound output to open.
    let reply = audio.engine().play(&url, station)?;
    reply.wait()?;
    Ok(PlayOutcome { show_click_notice })
}

/// Called on every player state change: the listening delay before a click is reported starts
/// with the first sound, not when the station is chosen (connecting can take several seconds).
/// Resuming after a pause does not start it again.
pub(crate) fn schedule_click(app: &AppHandle, state: &PlayerState) {
    if *state != PlayerState::Playing {
        return;
    }
    let Some(audio) = app.try_state::<AudioState>() else {
        return;
    };
    let Some((play, station_id)) = audio
        .pending_click
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    else {
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
    let listening = matches!(
        audio.engine().state(),
        PlayerState::Playing | PlayerState::Paused
    );
    if audio.plays.load(Ordering::SeqCst) != play || !listening {
        return;
    }
    let db = app.state::<Database>();
    let counting = with_db(&db, click_counting).await;
    if !counting.unwrap_or(false) {
        return;
    }
    let catalog = app.state::<CatalogState>().0.clone();
    if let Err(e) = catalog.count_click(station_id).await {
        tracing::debug!("failed to count click: {e}");
    }
}

/// The station for a recipe tool: its catalog id if it has one, nothing else.
pub(crate) fn catalog_ref(station_id: Option<String>) -> StationRef {
    StationRef {
        id: station_id.filter(|id| is_catalog_id(id)),
        ..Default::default()
    }
}

/// Catalog stations have a UUID; personal stations (`user:…`) are never reported.
pub(crate) fn is_catalog_id(id: &str) -> bool {
    !id.starts_with("user:") && id.len() == 36
}

/// What the player needs to play a station, whatever its origin.
#[derive(Debug, Serialize)]
pub struct ResolvedStation {
    pub id: String,
    pub name: String,
    pub url: String,
    pub genre: Option<String>,
}

impl From<OnAir> for ResolvedStation {
    fn from(s: OnAir) -> Self {
        Self {
            name: s.name.unwrap_or_else(|| s.url.clone()),
            id: s.id,
            url: s.url,
            genre: None,
        }
    }
}

/// Finds how to play a station known only by the ID stored in the history: personal stations
/// from the database, catalog stations from the catalog (one request), and stations played
/// without an ID are identified by their URL already.
pub(crate) async fn resolve(
    db: &Database,
    catalog: &dyn CatalogSource,
    station_id: String,
    name: Option<String>,
) -> AppResult<ResolvedStation> {
    let id = station_id.clone();
    let (personal, favorite) = with_db(db, move |db| {
        Ok((db.personal_station(&id)?, db.favorite(&id)?))
    })
    .await?;
    if let Some(p) = personal {
        return Ok(ResolvedStation {
            id: p.station_id,
            name: p.name,
            url: p.stream_url,
            genre: p.genre,
        });
    }
    if let Some(f) = favorite {
        return Ok(ResolvedStation {
            id: f.station_id,
            name: f.name,
            url: f.stream_url,
            genre: f.genre,
        });
    }
    if is_catalog_id(&station_id) {
        let station = catalog
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
        return Ok(ResolvedStation {
            id: station_id.clone(),
            name,
            url: station_id,
            genre: None,
        });
    }
    Err(format!("Unknown station: {station_id}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_station_survives_a_restart() {
        let db = Database::open_in_memory().unwrap();
        assert!(saved_station(&db).is_none());
        let fip = OnAir {
            id: "uuid-fip".into(),
            name: Some("FIP".into()),
            url: "https://example.org/fip.mp3".into(),
        };
        save_station(&db, &fip);
        let back = saved_station(&db).unwrap();
        assert_eq!((back.id, back.name, back.url), (fip.id, fip.name, fip.url));
    }

    #[test]
    fn a_damaged_last_station_is_ignored() {
        let db = Database::open_in_memory().unwrap();
        db.set_str(SETTING_LAST_STATION, "{not json").unwrap();
        assert!(saved_station(&db).is_none());
    }

    #[test]
    fn setting_click_counting_skips_the_notice() {
        let db = Database::open_in_memory().unwrap();
        set_click_counting(&db, false).unwrap();
        assert!(!click_counting(&db).unwrap());
        assert!(db.get_bool(SETTING_CLICK_NOTICE_SEEN, false).unwrap());
    }
}
