// SPDX-License-Identifier: MPL-2.0
//! Services the app keeps in Tauri's state (besides the player's `AudioState` and the database),
//! and the way to do blocking work from async code.
//!
//! Commands and background tasks are `async` so they run off the main thread. Their blocking work
//! (SQLite, files, waiting for the audio engine) goes to Tokio's blocking pool through
//! [`blocking`] and [`with_db`] (or [`spawn_blocking`] when nothing waits for the result), so it
//! never holds one of the runtime's few workers either.
//!
//! Debug builds check it: the database panics when it is used on a runtime thread outside these
//! helpers, or while the player's lock is held ([`check_database_use`]). The tests run in debug,
//! so a regression fails them instead of waiting for the next audit.

use std::sync::Arc;

use crate::catalog::CatalogSource;
use crate::covers::Covers;
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::now_playing::WebTitles;

/// Station catalog (today Radio-Browser, behind `CatalogSource`).
pub struct CatalogState(pub Arc<dyn CatalogSource>);

/// Web title sources, to turn them on or off from the settings.
pub struct WebTitlesState(pub Arc<WebTitles>);

/// Cover art of the track on air, chosen from the sources the user put in order.
pub struct CoversState(pub Arc<Covers>);

/// Runs blocking work (SQLite, files, waiting for the audio engine) on Tokio's blocking pool: an
/// async command must not hold a runtime worker while it waits for the database lock or the disk.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the helper the others go through"
    )]
    tokio::task::spawn_blocking(move || {
        let _off = OffRuntime::enter();
        work()
    })
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

/// Blocking work started from anywhere (an async task, the main thread, the audio thread), on the
/// app runtime's blocking pool; for work that nothing awaits, or whose result is not an
/// [`AppResult`]. Use it rather than `spawn_blocking` itself, which clippy refuses: this one tells
/// the debug check that the database may be used there.
pub(crate) fn spawn_blocking<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> tauri::async_runtime::JoinHandle<R> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the helper the others go through"
    )]
    tauri::async_runtime::spawn_blocking(move || {
        let _off = OffRuntime::enter();
        work()
    })
}

// ── Where the database may be used (debug builds) ───────────────────────────────────────────

#[cfg(debug_assertions)]
thread_local! {
    /// Blocking work running on this thread through [`blocking`] or [`spawn_blocking`].
    static OFF_RUNTIME: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Player locks (`audio::Status`) this thread holds.
    static PLAYER_LOCKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
fn bump(counter: &'static std::thread::LocalKey<std::cell::Cell<u32>>, up: bool) {
    counter.with(|c| c.set(if up { c.get() + 1 } else { c.get() - 1 }));
}

/// Marks the current thread as running blocking work, until dropped.
struct OffRuntime(());

impl OffRuntime {
    fn enter() -> Self {
        #[cfg(debug_assertions)]
        bump(&OFF_RUNTIME, true);
        Self(())
    }
}

impl Drop for OffRuntime {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        bump(&OFF_RUNTIME, false);
    }
}

/// Held with the player's lock: the database must wait until it is released. Every other
/// listener of the player waits for that lock, and an export can hold the database for seconds.
pub(crate) struct PlayerLockHeld(());

impl PlayerLockHeld {
    pub(crate) fn mark() -> Self {
        #[cfg(debug_assertions)]
        bump(&PLAYER_LOCKS, true);
        Self(())
    }
}

impl Drop for PlayerLockHeld {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        bump(&PLAYER_LOCKS, false);
    }
}

/// Called before each use of the database. In debug builds, panics when the thread holds the
/// player's lock, or is a worker of an async runtime (a task, not blocking work): there the wait
/// for the database stops every other task of that worker. Threads of their own (the main thread,
/// the audio thread, a background job) have no runtime and may use it.
pub(crate) fn check_database_use() {
    #[cfg(debug_assertions)]
    {
        assert!(
            PLAYER_LOCKS.with(|c| c.get()) == 0,
            "database used while holding the player's lock: release it first, or move the \
             database work to `state::spawn_blocking`"
        );
        assert!(
            OFF_RUNTIME.with(|c| c.get()) > 0 || tokio::runtime::Handle::try_current().is_err(),
            "database used on an async worker: go through `state::with_db` or `state::blocking`"
        );
    }
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
        let db = blocking(|| Ok(Database::open_in_memory()?)).await.unwrap();
        with_db(&db, |db| {
            Ok(db.set_flag(crate::db::settings::WEB_TITLES, false)?)
        })
        .await
        .unwrap();
        let read = with_db(&db, |db| Ok(db.flag(crate::db::settings::WEB_TITLES)?)).await;
        assert!(!read.unwrap());
        let detached = spawn_blocking(move || db.flag(crate::db::settings::WEB_TITLES));
        assert!(!detached.await.unwrap().unwrap());
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "async worker")]
    async fn the_database_is_refused_on_an_async_worker() {
        let _ = Database::open_in_memory();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "player's lock")]
    fn the_database_is_refused_under_the_player_lock() {
        let db = Database::open_in_memory().unwrap();
        let _held = PlayerLockHeld::mark();
        let _ = db.flag(crate::db::settings::WEB_TITLES);
    }

    #[test]
    fn the_database_is_free_on_a_thread_of_its_own() {
        let db = Database::open_in_memory().unwrap();
        drop(PlayerLockHeld::mark());
        assert!(db.flag(crate::db::settings::WEB_TITLES).is_ok());
    }
}
