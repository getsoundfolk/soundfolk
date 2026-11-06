// SPDX-License-Identifier: MPL-2.0
//! *Constant volume* (`docs/10-loudness.md`): the setting, and the database as the engine's
//! memory of station levels. The measuring and the gain are in `audio::loudness`.

use tauri::{AppHandle, Manager, State};

use crate::audio::{LevelMemory, StoredLevel};
use crate::commands::{with_db, AudioState};
use crate::db::Database;
use crate::error::AppResult;

pub const SETTING_CONSTANT_VOLUME: &str = "constant_volume";
/// On by default: it is what nearly every listener wants, and its failure mode (a station a bit
/// quieter than its own mastering) is mild.
pub const CONSTANT_VOLUME_DEFAULT: bool = true;

/// Station levels in the database. Read on the engine thread when a playback starts, written
/// every minute while it plays from the engine's `level-writer` thread: errors are logged, never
/// shown.
pub struct DatabaseLevels(pub AppHandle);

impl LevelMemory for DatabaseLevels {
    fn recall(&self, station: &str) -> Option<StoredLevel> {
        self.0
            .state::<Database>()
            .station_level(station)
            .inspect_err(|e| tracing::warn!("station level not read: {e}"))
            .ok()?
    }

    fn guess(&self) -> Option<f64> {
        self.0
            .state::<Database>()
            .median_station_loudness()
            .inspect_err(|e| tracing::warn!("level guess not read: {e}"))
            .ok()?
    }

    fn remember(&self, station: &str, level: &StoredLevel) {
        if let Err(e) = self
            .0
            .state::<Database>()
            .save_station_level(station, level)
        {
            tracing::warn!("station level not saved: {e}");
        }
    }
}

pub fn constant_volume(db: &Database) -> bool {
    db.get_bool(SETTING_CONSTANT_VOLUME, CONSTANT_VOLUME_DEFAULT)
        .unwrap_or_else(|e| {
            tracing::warn!("constant volume setting not read: {e}");
            CONSTANT_VOLUME_DEFAULT
        })
}

#[tauri::command]
pub async fn get_constant_volume(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| {
        Ok(db.get_bool(SETTING_CONSTANT_VOLUME, CONSTANT_VOLUME_DEFAULT)?)
    })
    .await
}

#[tauri::command]
pub async fn set_constant_volume(
    db: State<'_, Database>,
    audio: State<'_, AudioState>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| {
        Ok(db.set_bool(SETTING_CONSTANT_VOLUME, enabled)?)
    })
    .await?;
    audio.engine().set_normalize(enabled);
    Ok(())
}

/// Gain *Constant volume* applies to the station playing, dB, rounded; `None` when it is off or
/// nothing plays.
#[tauri::command]
pub async fn get_level_adjustment(audio: State<'_, AudioState>) -> AppResult<Option<i32>> {
    Ok(audio
        .engine()
        .level_adjustment()
        .map(|db| db.round() as i32))
}
