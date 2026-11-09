// SPDX-License-Identifier: MPL-2.0
//! Services the app keeps in Tauri's state (besides the player's `AudioState` and the database),
//! and the way to do blocking work from async code.
//!
//! Commands and background tasks are `async` so they run off the main thread. Their blocking work
//! (SQLite, files, waiting for the audio engine) goes to Tokio's blocking pool through
//! [`blocking`] and [`with_db`], so it never holds one of the runtime's few workers either.

use std::sync::Arc;

use crate::audio::WebTitles;
use crate::catalog::CatalogSource;
use crate::covers::Covers;
use crate::db::Database;
use crate::error::{AppError, AppResult};

/// Station catalog (today Radio-Browser, behind `CatalogSource`).
pub struct CatalogState(pub Arc<dyn CatalogSource>);

/// Web title sources, to turn them on or off from the settings.
pub struct WebTitlesState(pub Arc<WebTitles>);

/// Read the title of streams that do not announce it from the web (station page, server status).
pub const SETTING_WEB_TITLES: &str = "web_titles";

/// Cover art of the track on air, chosen from the sources the user put in order.
pub struct CoversState(pub Arc<Covers>);

/// Runs blocking work (SQLite, files, waiting for the audio engine) on Tokio's blocking pool: an
/// async command must not hold a runtime worker while it waits for the database lock or the disk.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| AppError::from(format!("Task interrupted: {e}")))?
}

/// [`blocking`] with the database (a cheap clone that shares the connection).
pub(crate) async fn with_db<T: Send + 'static>(
    db: &Database,
    work: impl FnOnce(&Database) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let db = db.clone();
    blocking(move || work(&db)).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// One runtime thread: if the work ran on it, the timer below could not fire before the work
    /// gives up waiting, and the work would be done first.
    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_leaves_the_runtime_free() {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let work = tokio::spawn(blocking(move || {
            Ok(released.recv_timeout(Duration::from_secs(5)).is_ok())
        }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!work.is_finished());
        release.send(()).unwrap();
        assert!(
            work.await.unwrap().unwrap(),
            "the work was not released by the runtime"
        );
    }

    #[tokio::test]
    async fn a_panic_in_blocking_work_is_an_error_not_a_crash() {
        let result: AppResult<()> = blocking(|| panic!("broken")).await;
        assert!(result.unwrap_err().to_string().contains("interrupted"));
    }

    #[tokio::test]
    async fn database_work_sees_the_same_connection() {
        let db = Database::open_in_memory().unwrap();
        with_db(&db, |db| Ok(db.set_bool("some_setting", false)?))
            .await
            .unwrap();
        assert!(!db.get_bool("some_setting", true).unwrap());
        let read = with_db(&db, |db| Ok(db.get_bool("some_setting", true)?)).await;
        assert!(!read.unwrap());
    }
}
