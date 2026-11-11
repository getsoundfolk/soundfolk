// SPDX-License-Identifier: MPL-2.0
//! New versions, on desktop only: the app says that one is out and links to its release page
//! (docs/05-features.md §4). On macOS, Windows and from an AppImage it installs it too, when the
//! listener asks, through Tauri's updater (docs/19-in-app-updates.md); other copies update the way
//! they were installed.
//!
//! * Each tagged release carries a `latest.json` (`release.yml`), and GitHub serves the one of the
//!   latest release that is not a pre-release at a fixed address: no API call, no rate limit.
//!   Besides the version and its page, it lists the signed packages the updater installs.
//! * Downloaded when the cached copy is a day old (`daily_list.rs`), only while the Settings switch
//!   is on (on by default): GitHub sees the IP address once a day at most.
//! * No check from a nightly (its version is the next release's base, so a comparison means
//!   nothing), from a Flatpak or Snap, or from a build whose packager set
//!   `SOUNDFOLK_NO_UPDATE_CHECK`: their package manager updates them.
//!
//! `SOUNDFOLK_UPDATE_URL` replaces the file's address (a local file server, to try it).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::daily_list::{self, Files, CHECK_EVERY, FIRST_CHECK};
use crate::db::{settings, Database};
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{blocking, with_db};

pub const DEFAULT_URL: &str =
    "https://github.com/getsoundfolk/soundfolk/releases/latest/download/latest.json";
/// The only pages the notice may open: a release of the app.
const RELEASE_PAGES: &str = "https://github.com/getsoundfolk/soundfolk/releases/";

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
    /// This system gets new versions from Soundfolk's APT repository: its software updater
    /// installs this one (docs/17-apt-repository.md). Decided by the app: a value in
    /// `latest.json` is replaced ([`Updates::available`]).
    #[serde(default)]
    pub through_apt: bool,
    /// The app can install it itself: this copy is one the updater replaces, and the release has
    /// a signed package for it. Decided by the app, as `through_apt`.
    #[serde(default)]
    pub installable: bool,
}

/// What `latest.json` says: the release, and the entries of its `platforms`, each a signed
/// package for the updater (`darwin-aarch64-app`, `windows-x86_64-nsis`…).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Latest {
    release: Release,
    packages: BTreeSet<String>,
}

/// The entry of `latest.json` that Tauri's updater installs over this copy, if it replaces this
/// kind of copy: an app bundle (macOS, from the disk image), the Windows installer, an AppImage.
/// Never a `.deb` or anything a package manager installed. Read from the marker the bundler
/// writes into each package's binary: a development build has none.
fn package_entry() -> Option<String> {
    use tauri::utils::{config::BundleType, platform::bundle_type};

    let kind = match bundle_type()? {
        BundleType::App => "app",
        BundleType::Nsis => "nsis",
        BundleType::AppImage => "appimage",
        _ => return None,
    };
    package_entry_for(std::env::consts::OS, std::env::consts::ARCH, kind)
}

/// Whether `tauri.conf.json` holds the key that packages must be signed with: without it the
/// updater can verify nothing, so this copy installs nothing.
fn has_update_key(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .is_some_and(|key| !key.trim().is_empty())
}

/// `{os}-{arch}-{kind}`, as the updater names them (`macos` is `darwin`).
fn package_entry_for(os: &str, arch: &str, kind: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" | "windows" => os,
        _ => return None,
    };
    Some(format!("{os}-{arch}-{kind}"))
}

/// The source the soundfolk package adds (`packaging/apt/postinst.sh`).
#[cfg(target_os = "linux")]
const APT_SOURCE: &str = "/etc/apt/sources.list.d/soundfolk.sources";

/// Whether this system is subscribed to Soundfolk's APT repository. Blocking (reads the disk).
fn through_apt() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(APT_SOURCE).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn parse(text: &str) -> Result<Latest, String> {
    #[derive(Deserialize)]
    struct File {
        #[serde(flatten)]
        release: Release,
        /// Read by the updater itself, when it installs: only the names count here.
        #[serde(default)]
        platforms: BTreeMap<String, serde::de::IgnoredAny>,
    }

    let File { release, platforms } = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Version::parse(&release.version).map_err(|e| format!("version {}: {e}", release.version))?;
    let page = release_page(&release.url).ok_or(format!("not a release page: {}", release.url))?;
    Ok(Latest {
        release: Release {
            url: page,
            ..release
        },
        packages: platforms.into_keys().collect(),
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
    /// The entry of `latest.json` this copy installs, if the updater replaces it.
    package: Option<String>,
    latest: RwLock<Option<Latest>>,
    wake: Notify,
}

impl Updates {
    pub fn new(app: &AppHandle) -> Self {
        let current = app.package_info().version.clone();
        let supported = may_check(&current, packaged());
        Self {
            current,
            supported,
            package: package_entry().filter(|_| has_update_key(app)),
            latest: RwLock::default(),
            wake: Notify::new(),
        }
    }

    fn set(&self, latest: Latest) {
        *self.latest.write().unwrap_or_else(|e| e.into_inner()) = Some(latest);
    }

    /// The release to tell the listener about, if any.
    fn available(&self, db: &Database) -> AppResult<Option<Release>> {
        if !self.supported || !db.flag(settings::UPDATE_CHECK)? {
            return Ok(None);
        }
        let skipped = db.text(settings::UPDATE_SKIPPED)?;
        let latest = self.latest.read().unwrap_or_else(|e| e.into_inner());
        Ok(latest
            .as_ref()
            .filter(|l| newer(&self.current, &l.release, skipped.as_deref()))
            .map(|l| Release {
                through_apt: through_apt(),
                installable: self
                    .package
                    .as_ref()
                    .is_some_and(|p| l.packages.contains(p)),
                ..l.release.clone()
            }))
    }
}

/// Loads the cached `latest.json`, then keeps it up to date in the background while the check is
/// on. Nothing at all when this copy does not check.
pub fn start(app: &AppHandle, updates: Arc<Updates>, cache_dir: Option<PathBuf>) {
    let Some(dir) = cache_dir.filter(|_| updates.supported) else {
        return;
    };
    if let Some(latest) = daily_list::read_cached(&dir, FILES, parse) {
        updates.set(latest);
    }
    let url = std::env::var("SOUNDFOLK_UPDATE_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let client = match crate::http::client() {
            Ok(client) => client,
            Err(e) => return tracing::warn!("update check unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let db = app.state::<Database>();
            let enabled = with_db(&db, |db| Ok(db.flag(settings::UPDATE_CHECK)?))
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("update check setting not read: {e}");
                    false
                });
            if enabled && daily_list::is_due(&dir, FILES) {
                match daily_list::refresh(client, &url, &dir, FILES, parse).await {
                    Ok(Some(latest)) => {
                        tracing::info!("latest release: {}", latest.release.version);
                        updates.set(latest);
                        if let Err(e) = app.emit(events::UPDATE_AVAILABLE, ()) {
                            tracing::warn!("failed to emit {}: {e}", events::UPDATE_AVAILABLE);
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
            enabled: db.flag(settings::UPDATE_CHECK)?,
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
    with_db(&db, move |db| {
        Ok(db.set_flag(settings::UPDATE_CHECK, enabled)?)
    })
    .await?;
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

/// Downloads the new version, checks its signature against the key in `tauri.conf.json` (and that
/// it was signed for the version announced), installs it over this copy and starts it. Only when
/// [`Release::installable`]. On Windows the installer closes the app and starts it again itself.
///
/// The updater reads `latest.json` again from `tauri.conf.json`'s address (`SOUNDFOLK_UPDATE_URL`
/// does not apply), with its own HTTP client: our own service, and the package is signed.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle) -> AppResult<()> {
    use tauri_plugin_updater::UpdaterExt;

    let failed = |e: tauri_plugin_updater::Error| AppError::from(format!("Update failed: {e}"));
    let saving = app.clone();
    let updater = app
        .updater_builder()
        // Windows only: the installer is started, then the app exits at once.
        .on_before_exit(move || crate::before_exit(&saving))
        .build()
        .map_err(failed)?;
    let update = updater
        .check()
        .await
        .map_err(failed)?
        .ok_or_else(|| AppError::from("No new version to install".to_owned()))?;
    tracing::info!("downloading version {}", update.version);
    let package = update.download(|_, _| {}, || {}).await.map_err(failed)?;
    let saving = app.clone();
    blocking(move || {
        update.install(package).map_err(failed)?;
        crate::before_exit(&saving);
        Ok(())
    })
    .await?;
    tracing::info!("update installed, restarting");
    app.restart()
}

/// No more notice for this version.
#[tauri::command]
#[specta::specta]
pub async fn skip_update(db: State<'_, Database>, version: String) -> AppResult<()> {
    with_db(&db, move |db| {
        Ok(db.set_text(settings::UPDATE_SKIPPED, &version)?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.to_owned(),
            url: format!("{RELEASE_PAGES}tag/v{version}"),
            through_apt: false,
            installable: false,
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn reads_latest_json() {
        let text = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/v0.3.0"}"#;
        let latest = parse(text).unwrap();
        assert_eq!(latest.release, release("0.3.0"));
        assert!(latest.packages.is_empty());
    }

    #[test]
    fn reads_the_packages_for_the_updater() {
        let text = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/v0.3.0",
            "platforms":{"darwin-aarch64-app":{"url":"https://example.com/a","signature":"s"},
                         "linux-x86_64-appimage":{"url":"https://example.com/b","signature":"s"}}}"#;
        let latest = parse(text).unwrap();
        assert_eq!(latest.release, release("0.3.0"));
        assert_eq!(
            latest.packages.into_iter().collect::<Vec<_>>(),
            ["darwin-aarch64-app", "linux-x86_64-appimage"]
        );
    }

    #[test]
    fn names_the_package_as_the_updater_does() {
        let entry = |os, arch, kind| package_entry_for(os, arch, kind);
        assert_eq!(
            entry("macos", "aarch64", "app").as_deref(),
            Some("darwin-aarch64-app")
        );
        assert_eq!(
            entry("windows", "x86_64", "nsis").as_deref(),
            Some("windows-x86_64-nsis")
        );
        assert_eq!(
            entry("linux", "x86_64", "appimage").as_deref(),
            Some("linux-x86_64-appimage")
        );
        assert_eq!(entry("freebsd", "x86_64", "appimage"), None);
    }

    #[test]
    fn a_value_in_the_file_does_not_make_it_installable() {
        let text = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/v0.3.0","installable":true,"through_apt":true}"#;
        let updates = Updates {
            current: v("0.2.1"),
            supported: true,
            package: None,
            latest: RwLock::new(Some(parse(text).unwrap())),
            wake: Notify::new(),
        };
        let db = Database::open_in_memory().unwrap();
        let shown = updates.available(&db).unwrap().unwrap();
        assert!(!shown.installable);
        assert_eq!(shown.through_apt, through_apt());
    }

    #[test]
    fn installable_when_the_release_has_a_package_for_this_copy() {
        let text = r#"{"version":"0.3.0","url":"https://github.com/getsoundfolk/soundfolk/releases/tag/v0.3.0",
            "platforms":{"linux-x86_64-appimage":{"url":"https://example.com/b","signature":"s"}}}"#;
        let updates = |package: &str| Updates {
            current: v("0.2.1"),
            supported: true,
            package: Some(package.to_owned()),
            latest: RwLock::new(Some(parse(text).unwrap())),
            wake: Notify::new(),
        };
        let db = Database::open_in_memory().unwrap();
        let shown = |package| updates(package).available(&db).unwrap().unwrap();
        assert!(shown("linux-x86_64-appimage").installable);
        assert!(!shown("windows-x86_64-nsis").installable);
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
