// SPDX-License-Identifier: MPL-2.0
//! New versions, on desktop only: the app says that one is out and links to its release page. It
//! installs nothing, the listener updates the way they installed (docs/05-features.md §4).
//!
//! * Each tagged release carries a `latest.json` (`release.yml`), and GitHub serves the one of the
//!   latest release that is not a pre-release at a fixed address: no API call, no rate limit.
//! * Downloaded when the cached copy is a day old (`daily_list.rs`), only while the Settings switch
//!   is on (on by default): GitHub sees the IP address once a day at most.
//! * No check from a nightly (its version is the next release's base, so a comparison means
//!   nothing), from a Flatpak or Snap, or from a build whose packager set
//!   `SOUNDFOLK_NO_UPDATE_CHECK`: their package manager updates them.
//!
//! `SOUNDFOLK_UPDATE_URL` replaces the file's address (a local file server, to try it).

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::daily_list::{self, Files, CHECK_EVERY, FIRST_CHECK};
use crate::db::Database;
use crate::error::AppResult;
use crate::state::with_db;

pub const DEFAULT_URL: &str =
    "https://github.com/getsoundfolk/soundfolk/releases/latest/download/latest.json";
/// The only pages the notice may open: a release of the app.
const RELEASE_PAGES: &str = "https://github.com/getsoundfolk/soundfolk/releases/";
/// Event emitted when a newer version was found (no payload: the window asks [`get_update`]).
pub const UPDATE_EVENT: &str = "updates:available";
/// Whether to check (Settings › Privacy), on by default.
const SETTING_CHECK: &str = "update_check";
/// The version the listener chose to skip: no notice for it, the next one has one again.
const SETTING_SKIPPED: &str = "update_skipped";

const FILES: Files = Files {
    list: "latest.json",
    etag: "latest.etag",
};

/// The latest release, as `latest.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Release {
    pub version: String,
    /// Its release page, where the packages are.
    pub url: String,
}

fn parse(text: &str) -> Result<Release, String> {
    let release: Release = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Version::parse(&release.version).map_err(|e| format!("version {}: {e}", release.version))?;
    let page = release_page(&release.url).ok_or(format!("not a release page: {}", release.url))?;
    Ok(Release {
        url: page,
        ..release
    })
}

/// The address as a page of this project's releases, read as a browser would (`..` resolved),
/// or `None`: nothing but `https://github.com/getsoundfolk/soundfolk/releases/…`, without login,
/// query or fragment.
fn release_page(text: &str) -> Option<String> {
    let base = url::Url::parse(RELEASE_PAGES).ok()?;
    let url = url::Url::parse(text).ok()?;
    let same_site = url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port_or_known_default() == base.port_or_known_default();
    // `@` anywhere reads as a login to a person, wherever the parser puts it.
    let plain = !text.contains('@')
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    (same_site && plain && url.path().starts_with(base.path())).then(|| url.into())
}

/// Whether this copy may check, from its version and how it was installed.
fn may_check(current: &Version, packaged: bool) -> bool {
    !packaged && !current.build.as_str().contains("nightly")
}

/// Installed through a package manager that updates it, as far as the app can tell.
fn packaged() -> bool {
    option_env!("SOUNDFOLK_NO_UPDATE_CHECK").is_some()
        || std::env::var_os("FLATPAK_ID").is_some()
        || std::env::var_os("SNAP").is_some()
}

/// `latest` when it is newer than `current` and not the skipped version. Build metadata does not
/// count (`0.3.0+…` is `0.3.0`).
fn newer(current: &Version, latest: &Release, skipped: Option<&str>) -> bool {
    Version::parse(&latest.version).is_ok_and(|v| v.cmp_precedence(current).is_gt())
        && skipped != Some(latest.version.as_str())
}

/// The latest release known, and what wakes the check when it is switched on.
pub struct Updates {
    current: Version,
    supported: bool,
    latest: RwLock<Option<Release>>,
    wake: Notify,
}

impl Updates {
    pub fn new(app: &AppHandle) -> Self {
        let current = app.package_info().version.clone();
        let supported = may_check(&current, packaged());
        Self {
            current,
            supported,
            latest: RwLock::default(),
            wake: Notify::new(),
        }
    }

    fn set(&self, release: Release) {
        *self.latest.write().unwrap_or_else(|e| e.into_inner()) = Some(release);
    }

    /// The release to tell the listener about, if any.
    fn available(&self, db: &Database) -> AppResult<Option<Release>> {
        if !self.supported || !db.get_bool(SETTING_CHECK, true)? {
            return Ok(None);
        }
        let skipped = db.get_str(SETTING_SKIPPED)?;
        let latest = self.latest.read().unwrap_or_else(|e| e.into_inner());
        Ok(latest
            .as_ref()
            .filter(|l| newer(&self.current, l, skipped.as_deref()))
            .cloned())
    }
}

/// Loads the cached `latest.json`, then keeps it up to date in the background while the check is
/// on. Nothing at all when this copy does not check.
pub fn start(app: &AppHandle, updates: Arc<Updates>, cache_dir: Option<PathBuf>) {
    let Some(dir) = cache_dir.filter(|_| updates.supported) else {
        return;
    };
    if let Some(release) = daily_list::read_cached(&dir, FILES, parse) {
        updates.set(release);
    }
    let url = std::env::var("SOUNDFOLK_UPDATE_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let client = match daily_list::client() {
            Ok(client) => client,
            Err(e) => return tracing::warn!("update check unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let db = app.state::<Database>();
            let enabled = with_db(&db, |db| Ok(db.get_bool(SETTING_CHECK, true)?))
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("update check setting not read: {e}");
                    false
                });
            if enabled && daily_list::is_due(&dir, FILES) {
                match daily_list::refresh(&client, &url, &dir, FILES, parse).await {
                    Ok(Some(release)) => {
                        tracing::info!("latest release: {}", release.version);
                        updates.set(release);
                        if let Err(e) = app.emit(UPDATE_EVENT, ()) {
                            tracing::warn!("failed to emit {UPDATE_EVENT}: {e}");
                        }
                    }
                    Ok(None) => tracing::debug!("latest release: unchanged"),
                    Err(e) => tracing::warn!("latest release: {e}"),
                }
            }
            tokio::select! {
                () = tokio::time::sleep(CHECK_EVERY) => {}
                () = updates.wake.notified() => {}
            }
        }
    });
}

/// Whether the update check applies to this copy, and whether it is on.
#[derive(Debug, Serialize, specta::Type)]
pub struct UpdateCheck {
    pub supported: bool,
    pub enabled: bool,
}

#[tauri::command]
#[specta::specta]
pub async fn get_update_check(
    db: State<'_, Database>,
    updates: State<'_, Arc<Updates>>,
) -> AppResult<UpdateCheck> {
    let supported = updates.supported;
    with_db(&db, move |db| {
        Ok(UpdateCheck {
            supported,
            enabled: db.get_bool(SETTING_CHECK, true)?,
        })
    })
    .await
}

/// Switched on, it checks at once if the last check is a day old.
#[tauri::command]
#[specta::specta]
pub async fn set_update_check(
    db: State<'_, Database>,
    updates: State<'_, Arc<Updates>>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.set_bool(SETTING_CHECK, enabled)?)).await?;
    if enabled {
        updates.wake.notify_one();
    }
    Ok(())
}

/// The newer version to tell the listener about, if there is one.
#[tauri::command]
#[specta::specta]
pub async fn get_update(
    db: State<'_, Database>,
    updates: State<'_, Arc<Updates>>,
) -> AppResult<Option<Release>> {
    let updates = updates.inner().clone();
    with_db(&db, move |db| updates.available(db)).await
}

/// No more notice for this version.
#[tauri::command]
#[specta::specta]
pub async fn skip_update(db: State<'_, Database>, version: String) -> AppResult<()> {
    with_db(&db, move |db| Ok(db.set_str(SETTING_SKIPPED, &version)?)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.to_owned(),
            url: format!("{RELEASE_PAGES}tag/v{version}"),
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn reads_latest_json() {
        let text = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/v0.3.0"}"#;
        assert_eq!(parse(text).unwrap(), release("0.3.0"));
    }

    #[test]
    fn refuses_another_page_or_a_bad_version() {
        let elsewhere = r#"{"version":"0.3.0","url":"https://example.com/soundfolk.deb"}"#;
        assert!(parse(elsewhere).is_err());
        let lookalike = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/@evil.example"}"#;
        assert!(parse(lookalike).is_err());
        let climbing = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/../../other/x"}"#;
        assert!(parse(climbing).is_err());
        let encoded = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/%2e%2e/%2e%2e/other"}"#;
        assert!(parse(encoded).is_err());
        let version = r#"{"version":"next","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/next"}"#;
        assert!(parse(version).is_err());
    }

    #[test]
    fn only_a_newer_version_not_skipped() {
        assert!(newer(&v("0.2.1"), &release("0.3.0"), None));
        assert!(newer(&v("0.2.1"), &release("0.2.10"), None));
        assert!(!newer(&v("0.2.1"), &release("0.2.1"), None));
        assert!(!newer(&v("0.3.0"), &release("0.2.1"), None));
        assert!(!newer(&v("0.2.1"), &release("0.3.0"), Some("0.3.0")));
        assert!(newer(&v("0.2.1"), &release("0.3.1"), Some("0.3.0")));
    }

    #[test]
    fn build_metadata_does_not_make_a_version_newer() {
        assert!(!newer(&v("0.3.0+deb12"), &release("0.3.0"), None));
    }

    #[test]
    fn nightlies_and_packaged_copies_do_not_check() {
        assert!(may_check(&v("0.2.1"), false));
        assert!(!may_check(&v("0.2.1+nightly.20261108.eda4787"), false));
        assert!(!may_check(&v("0.2.1"), true));
    }
}
