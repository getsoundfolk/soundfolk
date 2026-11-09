// SPDX-License-Identifier: MPL-2.0
//! *Constant volume* (`docs/11-loudness.md`): the setting, and the database as the engine's
//! memory of station levels. The measuring and the gain are in `audio::loudness`.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::{AudioEngine, LevelMemory, StoredLevel};
use crate::db::Database;
use crate::error::AppResult;
use crate::player::AudioState;
use crate::state::with_db;

pub const SETTING_CONSTANT_VOLUME: &str = "constant_volume";
/// Volume the user last chose (`0.0..=1.0`), restored at the next launch.
const SETTING_VOLUME: &str = "volume";
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
#[specta::specta]
pub async fn get_constant_volume(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| {
        Ok(db.get_bool(SETTING_CONSTANT_VOLUME, CONSTANT_VOLUME_DEFAULT)?)
    })
    .await
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
pub async fn get_level_adjustment(audio: State<'_, AudioState>) -> AppResult<Option<i32>> {
    Ok(audio
        .engine()
        .level_adjustment()
        .map(|db| db.round() as i32))
}

/// The slider level and the mute. Held here rather than in a window: the tray mutes too, and the
/// main window and the mini player must show the same thing.
#[derive(Default)]
pub struct Sound(Mutex<SoundLevel>);

#[derive(Default)]
struct SoundLevel {
    /// Slider position, `None` until a window sets it: the engine then plays at full gain.
    level: Option<f32>,
    muted: bool,
}

impl Sound {
    fn lock(&self) -> std::sync::MutexGuard<'_, SoundLevel> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn muted(&self) -> bool {
        self.lock().muted
    }
}

/// The slider moved: the level is kept, and heard unless the sound is muted. Unmuting is the
/// window's call (moving the slider does it), not this one's: a window that opens sets its
/// level, and must not bring back a sound the tray cut.
pub fn set_level(app: &AppHandle, level: f32) -> AppResult<()> {
    if !level.is_finite() {
        return Err("Invalid volume".to_owned().into());
    }
    let (Some(sound), Some(audio)) = (app.try_state::<Sound>(), app.try_state::<AudioState>())
    else {
        return Ok(());
    };
    // Held while the engine changes, so that a mute from the tray cannot come in between
    let mut s = sound.lock();
    s.level = Some(level);
    if !s.muted {
        audio.engine().set_volume(level)?;
    }
    Ok(())
}

/// Event emitted when the sound is cut or comes back (payload: whether it is cut).
pub(crate) const MUTED_EVENT: &str = "player:muted";

/// Cuts the sound or brings it back, and tells the windows and the tray menu.
pub fn mute(app: &AppHandle, muted: bool) -> AppResult<()> {
    let (Some(sound), Some(audio)) = (app.try_state::<Sound>(), app.try_state::<AudioState>())
    else {
        return Ok(());
    };
    {
        let mut s = sound.lock();
        s.muted = muted;
        audio
            .engine()
            .set_volume(if muted { 0.0 } else { s.level.unwrap_or(1.0) })?;
    }
    if let Err(e) = app.emit(MUTED_EVENT, muted) {
        tracing::warn!("mute not sent to the windows: {e}");
    }
    crate::tray::show_muted(app, muted);
    Ok(())
}

/// The tray's Mute / Unmute.
pub fn toggle_mute(app: &AppHandle) {
    let muted = app.try_state::<Sound>().is_some_and(|s| s.muted());
    if let Err(e) = mute(app, !muted) {
        tracing::warn!("mute not toggled: {e}");
    }
}

#[tauri::command]
#[specta::specta]
pub async fn set_volume(app: AppHandle, volume: f32) -> AppResult<()> {
    set_level(&app, volume)
}

/// Volume saved by `save_volume`, or `None` if the user never moved the slider.
#[tauri::command]
#[specta::specta]
pub async fn get_volume(db: State<'_, Database>) -> AppResult<Option<f32>> {
    let saved = with_db(&db, |db| Ok(db.get_str(SETTING_VOLUME)?)).await?;
    let saved = saved.and_then(|v| v.parse::<f32>().ok());
    Ok(saved.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)))
}

/// Remembers the volume for the next launch. Called once the slider is released, not on every
/// step of a drag: concurrent commands could otherwise land out of order and keep a stale value.
#[tauri::command]
#[specta::specta]
pub async fn save_volume(db: State<'_, Database>, volume: f32) -> AppResult<()> {
    if !volume.is_finite() {
        return Err("Invalid volume".to_owned().into());
    }
    let volume = volume.clamp(0.0, 1.0).to_string();
    with_db(&db, move |db| Ok(db.set_str(SETTING_VOLUME, &volume)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_muted(sound: State<'_, Sound>) -> AppResult<bool> {
    Ok(sound.muted())
}

#[tauri::command]
#[specta::specta]
pub async fn set_muted(app: AppHandle, muted: bool) -> AppResult<()> {
    mute(&app, muted)
}
