// SPDX-License-Identifier: MPL-2.0
//! A favorite whose stream no longer starts gets its new address from the catalog, without asking.
//!
//! Stations move their streams (new server, new mount point) while the catalog follows them under
//! the same ID. When a catalog favorite fails before its first sound, the station is looked up again
//! by ID, and only that one:
//! * the catalog has another address: it is played at once, and saved in the favorite only once it
//!   actually plays (if it fails too, the favorite stays as it was);
//! * the catalog no longer has the station: the error says so, and the window offers to remove
//!   the favorite (Yes / No under the error, never a dialog);
//! * anything else (same address, catalog unreachable): the original error.
//!
//! One lookup per playback, never a loop. While it runs, the player shows it is still trying
//! instead of the error. Personal stations are never sent to the catalog.

use std::sync::{Mutex, MutexGuard};

use tauri::{AppHandle, Emitter, Manager};

use crate::audio::PlayerState;
use crate::db::{Database, Favorite};
use crate::events;
use crate::media_controls::StationChange;
use crate::player::{self, AudioState};
use crate::state::CatalogState;

const LOOKING_UP: &str = "Looking for the station's new address…";
pub(crate) const GONE: &str = "This station is no longer in the catalog.";

/// The favorite being started, if the current playback is one (Tauri state).
#[derive(Default)]
pub struct FavoriteRepair(Mutex<Repair>);

#[derive(Default)]
struct Repair {
    /// Counts playbacks: a lookup that ends after another station started is dropped.
    plays: u64,
    watched: Option<Watched>,
    /// Address about to be played as the retry, so that [`watch`] recognises it.
    retry_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Watched {
    play: u64,
    favorite: Favorite,
    /// Address being played.
    url: String,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Not heard yet.
    Starting,
    /// Failed before its first sound: the catalog is being asked.
    LookingUp,
    /// Playing the catalog's new address, not heard yet.
    Retrying,
}

impl FavoriteRepair {
    fn lock(&self) -> MutexGuard<'_, Repair> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// What a player state asks for.
#[derive(Debug, PartialEq)]
enum Action {
    Nothing,
    /// The favorite failed before its first sound: hold the error back and ask the catalog.
    LookUp(Watched),
    /// The new address played: save it in the favorite.
    Save(Watched),
}

impl Repair {
    /// A playback starts; `favorite` is the catalog favorite it plays, if any.
    fn start(&mut self, favorite: Option<Favorite>, url: &str) {
        self.plays += 1;
        let retrying = self.retry_url.take().is_some_and(|retry| retry == url);
        let play = self.plays;
        self.watched = favorite.map(|favorite| Watched {
            play,
            favorite,
            url: url.to_owned(),
            stage: if retrying {
                Stage::Retrying
            } else {
                Stage::Starting
            },
        });
    }

    fn on_state(&mut self, state: &PlayerState) -> Action {
        match state {
            // Heard once: whatever happens next, it did start.
            PlayerState::Playing | PlayerState::Paused => match self.watched.take() {
                Some(watched) if watched.stage == Stage::Retrying => Action::Save(watched),
                _ => Action::Nothing,
            },
            PlayerState::Idle => {
                self.watched = None;
                Action::Nothing
            }
            PlayerState::Error(_) => match self.watched.as_mut() {
                Some(watched) if watched.stage == Stage::Starting => {
                    watched.stage = Stage::LookingUp;
                    Action::LookUp(watched.clone())
                }
                // Already looked up, or the new address failed too: the error is shown.
                _ => {
                    self.watched = None;
                    Action::Nothing
                }
            },
            PlayerState::Connecting | PlayerState::Reconnecting(_) => Action::Nothing,
        }
    }

    /// Whether playback `play` still waits for its lookup (not stopped, no other station since).
    fn looking_up(&self, play: u64) -> bool {
        self.watched
            .as_ref()
            .is_some_and(|w| w.play == play && w.stage == Stage::LookingUp)
    }
}

/// Called for every playback, before it starts: watches it if it is a catalog favorite.
pub(crate) fn watch(app: &AppHandle, station_id: Option<&str>, url: &str) {
    let Some(state) = app.try_state::<FavoriteRepair>() else {
        return;
    };
    let favorite = station_id
        .filter(|id| player::is_catalog_id(id))
        .and_then(|id| {
            app.state::<Database>().favorite(id).unwrap_or_else(|e| {
                tracing::warn!("favorite not read: {e}");
                None
            })
        });
    state.lock().start(favorite, url);
}

/// Called for every player state, before anyone else sees it; returns the state to show. Holds
/// back the error of a favorite that never started, while its new address is looked up.
pub(crate) fn on_state(app: &AppHandle, state: PlayerState) -> PlayerState {
    let Some(repair) = app.try_state::<FavoriteRepair>() else {
        return state;
    };
    let action = repair.lock().on_state(&state);
    match (action, &state) {
        (Action::LookUp(watched), PlayerState::Error(message)) => {
            look_up(app.clone(), watched, message.clone());
            PlayerState::Reconnecting(LOOKING_UP.to_owned())
        }
        (Action::Save(watched), _) => {
            // This runs under the player's lock, often on the decoder thread: the database, which
            // an export can hold for a while, is written from elsewhere.
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || save_new_address(&app, &watched));
            state
        }
        _ => state,
    }
}

/// Asks the catalog for the station again, then plays its new address or shows the error.
fn look_up(app: AppHandle, watched: Watched, error: String) {
    tauri::async_runtime::spawn(async move {
        let catalog = app.state::<CatalogState>().0.clone();
        let found = catalog.station(&watched.favorite.station_id).await;
        let gone = matches!(found, Ok(None));
        let error = match found {
            Ok(Some(station)) if station.url != watched.url && station.is_supported() => {
                // Starting a station waits for the sound output: off the async runtime.
                let (retry_app, retry_watched) = (app.clone(), watched.clone());
                let retried = tauri::async_runtime::spawn_blocking(move || {
                    retry(&retry_app, &retry_watched, station.url)
                })
                .await
                .unwrap_or_else(|e| Err(format!("retry interrupted: {e}")));
                match retried {
                    Ok(()) => return,
                    Err(e) => {
                        tracing::warn!("cannot play the favorite's new address: {e}");
                        error
                    }
                }
            }
            Ok(Some(_)) => error,
            Ok(None) => {
                tracing::info!(station = %watched.favorite.station_id, "favorite no longer in the catalog");
                GONE.to_owned()
            }
            Err(e) => {
                tracing::debug!("cannot look the favorite up again: {e}");
                error
            }
        };
        let audio = app.state::<AudioState>();
        // Held while checking: a station started meanwhile waits, and its state comes after.
        let engine = audio.engine();
        if take_if_looking_up(&app, watched.play) {
            engine.show_error(error);
            // The window offers to remove it, under the error.
            if gone && let Err(e) = app.emit(events::FAVORITE_GONE, &watched.favorite.station_id) {
                tracing::warn!("failed to emit the vanished favorite: {e}");
            }
        }
    });
}

/// Plays the catalog's new address in place of the favorite's.
fn retry(app: &AppHandle, watched: &Watched, url: String) -> Result<(), String> {
    let favorite = &watched.favorite;
    {
        let repair = app.state::<FavoriteRepair>();
        let mut repair = repair.lock();
        // Stopped, or another station started meanwhile.
        if !repair.looking_up(watched.play) {
            return Ok(());
        }
        repair.retry_url = Some(url.clone());
    }
    tracing::info!(station = %favorite.station_id, "favorite moved: trying its new address");
    let outcome = player::start_station(
        app,
        url.clone(),
        Some(favorite.station_id.clone()),
        Some(favorite.name.clone()),
    )
    .map_err(|e| e.to_string())?;
    let change = StationChange {
        id: favorite.station_id.clone(),
        name: favorite.name.clone(),
        url,
        genre: favorite.genre.clone(),
        show_click_notice: outcome.show_click_notice,
        skipped: Vec::new(),
    };
    if let Err(e) = app.emit(events::STATION, change) {
        tracing::warn!("failed to emit the new station: {e}");
    }
    Ok(())
}

/// Whether playback `play` is still waiting for its lookup (not stopped, no other station since).
fn take_if_looking_up(app: &AppHandle, play: u64) -> bool {
    let repair = app.state::<FavoriteRepair>();
    let mut repair = repair.lock();
    let current = repair.looking_up(play);
    if current {
        repair.watched = None;
    }
    current
}

/// The new address played: it replaces the old one, unless the favorite was removed meanwhile.
fn save_new_address(app: &AppHandle, watched: &Watched) {
    match replace_address(&app.state::<Database>(), watched) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return tracing::warn!("favorite not updated: {e}"),
    }
    tracing::info!(station = %watched.favorite.station_id, "favorite's new address saved");
    if let Err(e) = app.emit(events::FAVORITES_CHANGED, ()) {
        tracing::warn!("failed to emit the favorites change: {e}");
    }
}

/// Saves the address played in the favorite; `false` if it is no longer a favorite.
fn replace_address(db: &Database, watched: &Watched) -> rusqlite::Result<bool> {
    let Some(favorite) = db.favorite(&watched.favorite.station_id)? else {
        return Ok(false);
    };
    db.add_favorite(&Favorite {
        stream_url: watched.url.clone(),
        ..favorite
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "http://old.example/live";
    const NEW: &str = "https://new.example/live";

    fn favorite() -> Option<Favorite> {
        Some(Favorite {
            station_id: "ab30ca84-c228-4f57-8cac-816c69ef651d".into(),
            name: "RDWA".into(),
            stream_url: OLD.into(),
            genre: None,
            added_at: None,
        })
    }

    fn error() -> PlayerState {
        PlayerState::Error("The station is not broadcasting: its server refuses connections".into())
    }

    fn is_look_up(action: &Action) -> bool {
        matches!(action, Action::LookUp(w) if w.url == OLD && w.stage == Stage::LookingUp)
    }

    #[test]
    fn a_favorite_failing_before_its_first_sound_is_looked_up_once() {
        let mut repair = Repair::default();
        repair.start(favorite(), OLD);
        assert_eq!(repair.on_state(&PlayerState::Connecting), Action::Nothing);
        let action = repair.on_state(&error());
        assert!(is_look_up(&action), "{action:?}");
        assert!(repair.looking_up(1));
        // The error shown after the lookup goes through.
        assert_eq!(repair.on_state(&error()), Action::Nothing);
        assert!(!repair.looking_up(1));
    }

    #[test]
    fn the_new_address_is_saved_only_once_it_plays() {
        let mut repair = Repair::default();
        repair.start(favorite(), OLD);
        assert!(is_look_up(&repair.on_state(&error())));
        repair.retry_url = Some(NEW.into());
        repair.start(favorite(), NEW);
        assert_eq!(repair.on_state(&PlayerState::Connecting), Action::Nothing);
        match repair.on_state(&PlayerState::Playing) {
            Action::Save(w) => assert_eq!(w.url, NEW),
            other => panic!("{other:?}"),
        }
        // Saved once.
        assert_eq!(repair.on_state(&PlayerState::Playing), Action::Nothing);
    }

    #[test]
    fn a_new_address_that_fails_too_is_not_looked_up_again() {
        let mut repair = Repair::default();
        repair.start(favorite(), OLD);
        assert!(is_look_up(&repair.on_state(&error())));
        repair.retry_url = Some(NEW.into());
        repair.start(favorite(), NEW);
        assert_eq!(repair.on_state(&error()), Action::Nothing);
        assert_eq!(repair.watched, None);
    }

    #[test]
    fn a_favorite_that_played_once_is_left_alone() {
        let mut repair = Repair::default();
        repair.start(favorite(), OLD);
        assert_eq!(repair.on_state(&PlayerState::Playing), Action::Nothing);
        // Given up after playing (the station stopped broadcasting): not a moved stream.
        assert_eq!(repair.on_state(&error()), Action::Nothing);
    }

    #[test]
    fn other_stations_are_never_looked_up() {
        let mut repair = Repair::default();
        repair.start(None, OLD);
        assert_eq!(repair.on_state(&error()), Action::Nothing);
    }

    #[test]
    fn a_stop_or_another_station_cancels_the_lookup() {
        let mut repair = Repair::default();
        repair.start(favorite(), OLD);
        assert!(is_look_up(&repair.on_state(&error())));
        assert_eq!(repair.on_state(&PlayerState::Idle), Action::Nothing);
        assert!(!repair.looking_up(1));

        repair.start(favorite(), OLD);
        assert!(is_look_up(&repair.on_state(&error())));
        repair.start(None, "http://other.example/");
        assert!(!repair.looking_up(2));
    }

    #[test]
    fn a_user_play_of_another_address_is_not_taken_for_the_retry() {
        let mut repair = Repair {
            retry_url: Some(NEW.into()),
            ..Default::default()
        };
        repair.start(favorite(), OLD);
        assert_eq!(
            repair.watched.as_ref().map(|w| w.stage),
            Some(Stage::Starting)
        );
        assert_eq!(repair.retry_url, None);
    }

    #[test]
    fn the_new_address_replaces_the_old_one_only_in_a_favorite_still_there() {
        let db = Database::open_in_memory().unwrap();
        let mut favorite = favorite().unwrap();
        favorite.name = "Renamed meanwhile".into();
        db.add_favorite(&favorite).unwrap();
        let watched = Watched {
            play: 1,
            favorite: favorite.clone(),
            url: NEW.into(),
            stage: Stage::Retrying,
        };
        assert!(replace_address(&db, &watched).unwrap());
        let saved = db.favorite(&favorite.station_id).unwrap().unwrap();
        assert_eq!(saved.stream_url, NEW);
        assert_eq!(saved.name, "Renamed meanwhile");
        assert_eq!(db.list_favorites().unwrap().len(), 1);

        // Removed while the new address was starting: not added back.
        db.remove_favorite(&favorite.station_id).unwrap();
        assert!(!replace_address(&db, &watched).unwrap());
        assert!(db.list_favorites().unwrap().is_empty());
    }
}
