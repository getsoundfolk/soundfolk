// SPDX-License-Identifier: MPL-2.0
//! Cover art of the track on air, from the sources the user put in order in ⚙ › Cover art.
//!
//! * **Station**: the cover the station's page or streaming server gives with the title (recipes,
//!   AzuraCast). Nothing more leaves the device.
//! * **Deezer**: the track is looked up on Deezer's public search, which gives its album's cover
//!   in one request (see [`deezer`]). Off by default: the artist and title leave the device.
//! * **iTunes**: the same from Apple's iTunes Search API (see [`itunes`]). Off by default too.
//! * **Cover Art Archive**: the track is looked up on MusicBrainz, then the front cover of its album
//!   on the Cover Art Archive. Off by default: the artist and title leave the device. Slow: its
//!   images are served by the Internet Archive, often after several seconds.
//!
//! Sources are asked from top to bottom and the first cover found wins; a disabled source is never
//! asked. Without any, the station's logo is shown: the one the user chose on the station sheet,
//! else its Radio-Browser favicon, kept on disk (see [`logos`]). Online sources are looked up one at a time, in order: the next one only when the one
//! before has no cover. While a lookup runs, the cover of a source further down is shown. A result
//! (even "none") is cached in the database for each source, so a track that comes back on air is
//! not looked up again.
//!
//! The desktop's media controls (MPRIS) show the station, not the track: next / previous change the
//! station there. Their picture is the station's logo, or the Soundfolk icon when it has none.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::audio::NowPlaying;
use crate::db::{settings, Database};
use crate::events;
use crate::media_controls::SystemMedia;
use crate::musicbrainz::{LookupError, MusicBrainz};
use crate::notifications::TrackNotifications;
use crate::state::with_db;
use crate::tracks;

mod deezer;
mod itunes;
mod logos;
mod search;
use deezer::Deezer;
use itunes::Itunes;
use logos::{ChosenLogos, Logo, LogoCache};

/// After a failed lookup (network), the same track is not looked up again for this long.
const RETRY_FAILED: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Station,
    Deezer,
    #[serde(rename = "itunes")]
    Itunes,
    CoverArtArchive,
}

impl SourceId {
    /// Key of a track's cover from this online source in the database cache. The Cover Art
    /// Archive keeps the bare track key it had when it was the only online source.
    fn cache_key(self, track_key: &str) -> String {
        match self {
            Self::Deezer => format!("deezer:{track_key}"),
            Self::Itunes => format!("itunes:{track_key}"),
            Self::Station | Self::CoverArtArchive => track_key.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CoverSource {
    pub id: SourceId,
    pub enabled: bool,
}

/// The order and switches of a fresh install: only what needs nothing more to leave the device.
fn default_sources() -> Vec<CoverSource> {
    vec![
        CoverSource {
            id: SourceId::Station,
            enabled: true,
        },
        CoverSource {
            id: SourceId::Deezer,
            enabled: false,
        },
        CoverSource {
            id: SourceId::Itunes,
            enabled: false,
        },
        CoverSource {
            id: SourceId::CoverArtArchive,
            enabled: false,
        },
    ]
}

/// Each source once, in the given order; a source the list does not name (added by a later version)
/// comes last, with its default switch.
pub fn normalize(list: Vec<CoverSource>) -> Vec<CoverSource> {
    let mut seen = HashSet::new();
    let mut out: Vec<CoverSource> = list.into_iter().filter(|s| seen.insert(s.id)).collect();
    out.extend(default_sources().into_iter().filter(|s| seen.insert(s.id)));
    out
}

pub fn load_sources(db: &Database) -> rusqlite::Result<Vec<CoverSource>> {
    let stored = db.text(settings::COVER_SOURCES)?;
    let list = stored
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    Ok(normalize(list))
}

pub fn save_sources(db: &Database, list: Vec<CoverSource>) -> rusqlite::Result<Vec<CoverSource>> {
    let list = normalize(list);
    let json = serde_json::to_string(&list).expect("cover sources serialize");
    db.set_text(settings::COVER_SOURCES, &json)?;
    Ok(list)
}

/// The cover shown for a title (`title` as in `NowPlaying.title`, so the window can tell it is
/// still the track on air).
#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct Cover {
    pub title: Option<String>,
    pub url: Option<String>,
    /// `url` is the station's logo, not the track's cover art.
    pub logo: bool,
}

/// A web address, trimmed.
fn web_address(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_owned())
}

/// Cover the station gives with the track on air, when it is a web address.
fn station_cover(now_playing: &NowPlaying) -> Option<String> {
    web_address(now_playing.track.as_ref()?.cover.as_deref()?)
}

/// The track to look up online: artist and title, and its cache key. A title without an artist
/// is too vague to look up; the station's own name between tracks is not a track.
fn online_query(
    now_playing: &NowPlaying,
    station: Option<&str>,
) -> Option<(String, tracks::ParsedTitle)> {
    let raw = now_playing
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    let is_station = |name: Option<&str>| name.is_some_and(|n| n.trim().eq_ignore_ascii_case(raw));
    if is_station(station) || is_station(now_playing.station.as_deref()) {
        return None;
    }
    let parsed = now_playing.parsed(raw, station);
    parsed.artist.as_ref()?;
    Some((tracks::track_key(&parsed), parsed))
}

/// What a source says for the track on air.
#[derive(Debug, PartialEq)]
enum Answer {
    Found(String),
    None,
    /// An online lookup must run first.
    Lookup,
}

/// The first cover found, asking the enabled sources in order, and the first source that must be
/// looked up before it (the ones below wait for its answer).
fn pick(
    sources: &[CoverSource],
    mut ask: impl FnMut(SourceId) -> Answer,
) -> (Option<String>, Option<SourceId>) {
    let mut lookup = None;
    for source in sources.iter().filter(|s| s.enabled) {
        match ask(source.id) {
            Answer::Found(url) => return (Some(url), lookup),
            Answer::Lookup => lookup = lookup.or(Some(source.id)),
            Answer::None => {}
        }
    }
    (None, lookup)
}

#[derive(Default)]
struct State {
    now_playing: NowPlaying,
    /// The station's own name (finds its signature in the titles).
    station: Option<String>,
    /// The name the user gave the station, shown instead of its own.
    label: Option<String>,
    shown: Cover,
    /// Lookups running, and failed lately, by cache key ([`SourceId::cache_key`]).
    in_flight: HashSet<String>,
    failed: HashMap<String, Instant>,
    /// Radio-Browser UUID of the station playing.
    station_id: Option<String>,
    /// Logos of the stations played since launch (`None`: the station has none).
    logos: HashMap<String, Option<Logo>>,
    /// Logos the user chose, read from disk the first time a station plays (`None`: not chosen).
    chosen: HashMap<String, Option<Logo>>,
    /// Stations whose logo was read from disk, and fetched again if need be, since launch.
    logos_checked: HashSet<String>,
    /// Picture last given to the media controls.
    media_picture: Option<String>,
    /// The cover sources, read from the database once, then kept by [`Covers::set_sources`].
    sources: Option<Vec<CoverSource>>,
}

pub struct Covers {
    app: tauri::AppHandle,
    mb: Option<Arc<MusicBrainz>>,
    deezer: Option<Arc<Deezer>>,
    itunes: Option<Arc<Itunes>>,
    media: Arc<SystemMedia>,
    logo_cache: LogoCache,
    chosen_logos: ChosenLogos,
    /// `file://` address of the Soundfolk icon, for the media controls of a station without a logo.
    app_icon: Option<String>,
    state: Mutex<State>,
}

/// The app icon, shipped in the binary: the media controls need it as a file.
pub(crate) const APP_ICON: &[u8] = include_bytes!("../icons/icon.png");

/// Writes the app icon to the cache folder (once) and returns its `file://` address (also the
/// icon of the track notifications).
pub fn app_icon_file(app: &tauri::AppHandle) -> Option<String> {
    let dir = app.path().app_cache_dir().ok()?;
    let path = dir.join("soundfolk.png");
    let current = std::fs::read(&path).is_ok_and(|bytes| bytes == APP_ICON);
    if !current {
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, APP_ICON));
        if let Err(e) = written {
            tracing::debug!("app icon not written for the media controls: {e}");
            return None;
        }
    }
    reqwest::Url::from_file_path(&path).ok().map(String::from)
}

impl Covers {
    pub fn new(
        app: tauri::AppHandle,
        mb: Option<Arc<MusicBrainz>>,
        media: Arc<SystemMedia>,
    ) -> Arc<Self> {
        let app_icon = app_icon_file(&app);
        let logo_cache = LogoCache::new(app.path().app_cache_dir().ok().map(|d| d.join("logos")));
        let chosen_logos = ChosenLogos::new(
            app.path()
                .app_data_dir()
                .ok()
                .map(|d| d.join("station_logos")),
        );
        Arc::new(Self {
            app,
            mb,
            deezer: Deezer::new().map(Arc::new),
            itunes: Itunes::new().map(Arc::new),
            media,
            logo_cache,
            chosen_logos,
            app_icon,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The cover shown now.
    pub fn current(&self) -> Cover {
        self.state().shown.clone()
    }

    /// The title on air changed: shows it on the desktop, and finds its cover, looked up if need be.
    pub fn update(
        self: &Arc<Self>,
        now_playing: NowPlaying,
        station: Option<String>,
        label: Option<String>,
    ) {
        {
            let mut state = self.state();
            state.now_playing = now_playing;
            state.station = station;
            state.label = label;
        }
        self.resolve(true);
    }

    /// The user renamed the station on air: the media controls show the new name.
    pub fn rename(self: &Arc<Self>, label: Option<String>) {
        let (now_playing, station, picture) = {
            let mut state = self.state();
            state.label = label.clone();
            (
                state.now_playing.clone(),
                state.station.clone(),
                state.media_picture.clone(),
            )
        };
        self.media.show_now_playing(
            &now_playing,
            station.as_deref(),
            label.as_deref(),
            picture.as_deref(),
        );
    }

    /// A station starts: its logo stands in for missing cover art. The one kept on disk is shown at
    /// once; it is fetched from the catalog when there is none yet or it is old, once per launch.
    /// `catalog_id` is `None` for a station outside the catalog (no logo).
    pub fn set_station(self: &Arc<Self>, catalog_id: Option<String>) {
        if let Some(id) = &catalog_id
            && !self.state().chosen.contains_key(id)
        {
            let chosen = self.chosen_logos.get(id);
            self.state().chosen.entry(id.clone()).or_insert(chosen);
        }
        let check = {
            let mut state = self.state();
            state.station_id = catalog_id.clone();
            catalog_id.filter(|id| state.logos_checked.insert(id.clone()))
        };
        let Some(id) = check else {
            return self.resolve(false);
        };
        let cached = self.logo_cache.cached(&id);
        let fresh = cached.as_ref().is_some_and(|(_, stale)| !stale);
        if let Some((logo, _)) = cached {
            self.state().logos.insert(id.clone(), Some(logo));
        }
        self.resolve(false);
        if fresh {
            return;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let catalog = this.app.state::<crate::state::CatalogState>().0.clone();
            let favicon = match catalog.station(&id).await {
                Ok(station) => station
                    .and_then(|s| s.favicon)
                    .and_then(|f| web_address(&f)),
                Err(e) => {
                    // Tried again the next time the station starts; a kept logo stays meanwhile.
                    this.state().logos_checked.remove(&id);
                    return tracing::debug!("station logo not found: {e}");
                }
            };
            let logo = match favicon {
                Some(url) => match this.logo_cache.fetch(&id, &url).await {
                    Ok(logo) => Some(logo),
                    Err(e) => {
                        this.state().logos_checked.remove(&id);
                        return tracing::debug!("station logo not downloaded: {e}");
                    }
                },
                None => {
                    this.logo_cache.forget(&id);
                    None
                }
            };
            this.state().logos.insert(id, logo);
            this.resolve_later();
        });
    }

    /// The logo the user chose for a station (`data:` address), if any. Reads the disk.
    pub fn chosen_logo(&self, id: &str) -> Option<String> {
        if let Some(chosen) = self.state().chosen.get(id) {
            return chosen.as_ref().map(|l| l.inline.clone());
        }
        let chosen = self.chosen_logos.get(id);
        let inline = chosen.as_ref().map(|l| l.inline.clone());
        self.state().chosen.insert(id.to_owned(), chosen);
        inline
    }

    /// The user chose an image as the station's logo, or took it back (`None`): shown at once if
    /// the station is on air. Returns the logo kept (`data:` address); the error is for the user.
    /// Reads the database: never on the async runtime.
    pub fn choose_logo(
        self: &Arc<Self>,
        id: &str,
        image: Option<&[u8]>,
    ) -> Result<Option<String>, String> {
        let logo = match image {
            Some(bytes) => Some(self.chosen_logos.set(id, bytes)?),
            None => {
                self.chosen_logos.clear(id);
                None
            }
        };
        let inline = logo.as_ref().map(|l| l.inline.clone());
        let on_air = {
            let mut state = self.state();
            state.chosen.insert(id.to_owned(), logo);
            state.station_id.as_deref() == Some(id)
        };
        if on_air {
            self.resolve(false);
        }
        Ok(inline)
    }

    /// The sources changed (as saved): the cover of the track on air is chosen again.
    pub fn set_sources(self: &Arc<Self>, sources: Vec<CoverSource>) {
        self.state().sources = Some(sources);
        self.resolve_later();
    }

    /// [`Covers::resolve`] from async code: it reads the database, which an export can hold for
    /// a while, so it runs on a blocking thread instead of a runtime worker.
    fn resolve_later(self: &Arc<Self>) {
        let this = self.clone();
        tauri::async_runtime::spawn_blocking(move || this.resolve(false));
    }

    /// The cover sources, read from the database the first time.
    fn sources(&self, db: &Database) -> Vec<CoverSource> {
        if let Some(sources) = &self.state().sources {
            return sources.clone();
        }
        match load_sources(db) {
            // Sources saved meanwhile are newer than this read.
            Ok(sources) => self.state().sources.get_or_insert(sources).clone(),
            // Not kept: read again for the next title.
            Err(e) => {
                tracing::warn!("cover sources not read: {e}");
                default_sources()
            }
        }
    }

    /// Reads the database: never on the async runtime (see [`Covers::resolve_later`]).
    fn resolve(self: &Arc<Self>, title_changed: bool) {
        let (now_playing, station, label, logo) = {
            let state = self.state();
            let logo = state.station_id.as_ref().and_then(|id| {
                let chosen = state.chosen.get(id).cloned().flatten();
                chosen.or_else(|| state.logos.get(id).cloned().flatten())
            });
            (
                state.now_playing.clone(),
                state.station.clone(),
                state.label.clone(),
                logo,
            )
        };
        let db = self.app.state::<Database>();
        let sources = self.sources(&db);
        let query = online_query(&now_playing, station.as_deref());

        let (url, lookup) = pick(&sources, |source| {
            if source == SourceId::Station {
                return station_cover(&now_playing).map_or(Answer::None, Answer::Found);
            }
            let Some((track_key, _)) = &query else {
                return Answer::None;
            };
            let key = source.cache_key(track_key);
            match db.cached_cover(&key) {
                Ok(Some(Some(url))) => Answer::Found(url),
                Ok(Some(None)) => Answer::None,
                // Failed lately (network): the next source is asked meanwhile.
                Ok(None) if self.can_look_up(source) && !self.failed_lately(&key) => Answer::Lookup,
                Ok(None) => Answer::None,
                Err(e) => {
                    tracing::warn!("cover cache not read: {e}");
                    Answer::None
                }
            }
        });
        // Nothing more to wait for: a track notification waiting for its picture can be shown.
        let settled = lookup
            .is_none()
            .then(|| (url.clone(), logo.as_ref().and_then(|l| l.file.clone())));
        if let (Some(source), Some((track_key, parsed))) = (lookup, query) {
            self.look_up(source, source.cache_key(&track_key), parsed);
        }

        let cover = match url {
            Some(url) => Cover {
                title: now_playing.title.clone(),
                url: Some(url),
                logo: false,
            },
            None => Cover {
                title: now_playing.title.clone(),
                logo: logo.is_some(),
                url: logo.as_ref().map(|l| l.inline.clone()),
            },
        };
        let picture = logo.and_then(|l| l.file).or_else(|| self.app_icon.clone());
        let (changed, picture_changed) = {
            let mut state = self.state();
            // The title moved on while this one was being resolved: the newer call shows it.
            if state.now_playing != now_playing {
                return;
            }
            let changed = state.shown != cover;
            state.shown = cover.clone();
            let picture_changed = state.media_picture != picture;
            state.media_picture = picture.clone();
            (changed, picture_changed)
        };
        if let Some((cover, logo)) = settled {
            self.notify_picture(now_playing.title.clone(), cover, logo);
        }
        if title_changed || picture_changed {
            self.media.show_now_playing(
                &now_playing,
                station.as_deref(),
                label.as_deref(),
                picture.as_deref(),
            );
        }
        if changed && let Err(e) = self.app.emit(events::COVER, cover) {
            tracing::warn!("failed to emit the cover: {e}");
        }
    }

    /// Gives the track notification waiting for this title its picture: the cover, downloaded to
    /// the cache folder (the desktop wants a file), else the station's logo.
    fn notify_picture(
        self: &Arc<Self>,
        title: Option<String>,
        cover: Option<String>,
        logo: Option<String>,
    ) {
        let Some(notifications) = self.app.try_state::<TrackNotifications>() else {
            return;
        };
        if !notifications.waits_for(title.as_deref()) {
            return;
        }
        let logo = logo
            .and_then(|l| Url::parse(&l).ok())
            .and_then(|l| l.to_file_path().ok());
        let Some(cover) = cover else {
            return notifications.picture(title.as_deref(), logo);
        };
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let path = match this
                .logo_cache
                .download(&cover, NOTIFICATION_MAX_BYTES)
                .await
            {
                Ok((bytes, ext, _)) => {
                    // Directory listing and writes: off the runtime.
                    let app = this.app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        notification_file(&app, &bytes, ext)
                    })
                    .await
                    .unwrap_or_else(|e| {
                        tracing::debug!("cover not written for the notification: {e}");
                        None
                    })
                }
                Err(e) => {
                    tracing::debug!("cover not downloaded for the notification: {e}");
                    None
                }
            };
            let notifications = this.app.state::<TrackNotifications>();
            notifications.picture(title.as_deref(), path.or(logo));
        });
    }

    fn can_look_up(&self, source: SourceId) -> bool {
        match source {
            SourceId::Station => false,
            SourceId::Deezer => self.deezer.is_some(),
            SourceId::Itunes => self.itunes.is_some(),
            SourceId::CoverArtArchive => self.mb.is_some(),
        }
    }

    fn failed_lately(&self, key: &str) -> bool {
        let mut state = self.state();
        state.failed.retain(|_, at| at.elapsed() < RETRY_FAILED);
        state.failed.contains_key(key)
    }

    /// Looks the track up on an online source in the background (once at a time, not again soon
    /// after a failure), then chooses the cover again.
    fn look_up(self: &Arc<Self>, source: SourceId, key: String, parsed: tracks::ParsedTitle) {
        if !self.can_look_up(source) || self.failed_lately(&key) {
            return;
        }
        if !self.state().in_flight.insert(key.clone()) {
            return;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = match source {
                SourceId::Deezer => match &this.deezer {
                    Some(deezer) => deezer.find_cover(&parsed).await,
                    None => Ok(None),
                },
                SourceId::Itunes => match &this.itunes {
                    Some(itunes) => itunes.find_cover(&parsed).await,
                    None => Ok(None),
                },
                SourceId::CoverArtArchive => match &this.mb {
                    Some(mb) => {
                        let artist = parsed.artist.as_deref().unwrap_or_default();
                        find_cover(mb, artist, &parsed.title)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    None => Ok(None),
                },
                SourceId::Station => Ok(None),
            };
            match &result {
                Ok(url) => {
                    let (key, url) = (key.clone(), url.clone());
                    let db = this.app.state::<Database>();
                    let saved =
                        with_db(&db, move |db| Ok(db.save_cover(&key, url.as_deref())?)).await;
                    if let Err(e) = saved {
                        tracing::warn!("cover not cached: {e}");
                    }
                }
                Err(e) => tracing::debug!("cover lookup on {source:?} failed: {e}"),
            }
            {
                let mut state = this.state();
                state.in_flight.remove(&key);
                if result.is_err() {
                    state.failed.insert(key, Instant::now());
                }
            }
            this.resolve_later();
        });
    }
}

/// Bigger than this, a cover is not downloaded for a notification.
const NOTIFICATION_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Writes the cover of a track notification to the cache folder, replacing the previous one. Each
/// cover gets its own name: a desktop may keep an image by its path.
fn notification_file(app: &tauri::AppHandle, bytes: &[u8], ext: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let dir = app.path().app_cache_dir().ok()?.join("notification");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let path = dir.join(format!("{:016x}.{ext}", hasher.finish()));
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten().filter(|e| e.path() != path) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes));
    match written {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::debug!("cover not written for the notification: {e}");
            None
        }
    }
}

/// The track's albums on MusicBrainz, then the first front cover the Cover Art Archive has for one.
async fn find_cover(
    mb: &MusicBrainz,
    artist: &str,
    title: &str,
) -> Result<Option<String>, LookupError> {
    for album in mb.albums_of(artist, title).await? {
        if let Some(url) = mb.release_group_cover(&album).await? {
            return Ok(Some(url));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::TrackInfo;

    fn on_air(title: &str, cover: Option<&str>) -> NowPlaying {
        NowPlaying {
            title: Some(title.into()),
            track: cover.map(|c| TrackInfo {
                artist: Some("Air".into()),
                title: "Sexy Boy".into(),
                started_at: None,
                duration_secs: None,
                cover: Some(c.into()),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn station_cover_is_a_web_address() {
        assert_eq!(
            station_cover(&on_air("Air - Sexy Boy", Some(" https://x/c.jpg "))).as_deref(),
            Some("https://x/c.jpg")
        );
        assert_eq!(
            station_cover(&on_air("Air - Sexy Boy", Some("file:///etc/passwd"))),
            None
        );
        assert_eq!(station_cover(&on_air("Air - Sexy Boy", None)), None);
    }

    #[test]
    fn stored_sources_are_completed() {
        let caa_first = vec![CoverSource {
            id: SourceId::CoverArtArchive,
            enabled: true,
        }];
        assert_eq!(
            normalize(caa_first),
            [
                CoverSource {
                    id: SourceId::CoverArtArchive,
                    enabled: true
                },
                CoverSource {
                    id: SourceId::Station,
                    enabled: true
                },
                CoverSource {
                    id: SourceId::Deezer,
                    enabled: false
                },
                CoverSource {
                    id: SourceId::Itunes,
                    enabled: false
                },
            ]
        );
        let twice = vec![
            CoverSource {
                id: SourceId::Station,
                enabled: false,
            },
            CoverSource {
                id: SourceId::Station,
                enabled: true,
            },
        ];
        assert_eq!(
            normalize(twice)[0],
            CoverSource {
                id: SourceId::Station,
                enabled: false
            }
        );
        assert_eq!(normalize(Vec::new()), default_sources());
    }

    #[test]
    fn sources_are_asked_in_order() {
        let both = |first, second| {
            vec![
                CoverSource {
                    id: first,
                    enabled: true,
                },
                CoverSource {
                    id: second,
                    enabled: true,
                },
            ]
        };
        let answers = |station: Answer, archive: Answer| {
            let mut answers = HashMap::from([
                (SourceId::Station, Some(station)),
                (SourceId::CoverArtArchive, Some(archive)),
            ]);
            move |id| answers.get_mut(&id).and_then(Option::take).unwrap()
        };
        let found = |u: &str| Answer::Found(u.into());
        let (s, a) = (SourceId::Station, SourceId::CoverArtArchive);

        assert_eq!(
            pick(&both(s, a), answers(found("s"), found("a"))),
            (Some("s".into()), None)
        );
        assert_eq!(
            pick(&both(a, s), answers(found("s"), found("a"))),
            (Some("a".into()), None)
        );
        // The archive must be asked first: the station's cover is shown meanwhile
        assert_eq!(
            pick(&both(a, s), answers(found("s"), Answer::Lookup)),
            (Some("s".into()), Some(a))
        );
        // The station has a cover: no need to look anything up
        assert_eq!(
            pick(&both(s, a), answers(found("s"), Answer::Lookup)),
            (Some("s".into()), None)
        );
        assert_eq!(
            pick(&both(s, a), answers(Answer::None, Answer::None)),
            (None, None)
        );

        let disabled = vec![
            CoverSource {
                id: s,
                enabled: false,
            },
            CoverSource {
                id: a,
                enabled: true,
            },
        ];
        assert_eq!(
            pick(&disabled, answers(found("s"), Answer::None)),
            (None, None)
        );
    }

    #[test]
    fn online_sources_are_looked_up_one_at_a_time() {
        let all = [
            SourceId::Deezer,
            SourceId::CoverArtArchive,
            SourceId::Station,
        ]
        .map(|id| CoverSource { id, enabled: true });
        let ask = |deezer: fn() -> Answer| {
            move |id| match id {
                SourceId::Deezer => deezer(),
                SourceId::CoverArtArchive => Answer::Lookup,
                SourceId::Station => Answer::Found("s".into()),
                SourceId::Itunes => unreachable!("not in the list"),
            }
        };
        // Both must be looked up: Deezer first, the station's cover meanwhile
        assert_eq!(
            pick(&all, ask(|| Answer::Lookup)),
            (Some("s".into()), Some(SourceId::Deezer))
        );
        // Deezer has none: the archive's turn
        assert_eq!(
            pick(&all, ask(|| Answer::None)),
            (Some("s".into()), Some(SourceId::CoverArtArchive))
        );
        assert_eq!(
            pick(&all, ask(|| Answer::Found("d".into()))),
            (Some("d".into()), None)
        );
        assert_ne!(
            SourceId::Deezer.cache_key("air|sexy boy"),
            SourceId::CoverArtArchive.cache_key("air|sexy boy")
        );
    }

    /// Against the real APIs: `cargo test -- --ignored live_cover`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_cover_of_a_classic() {
        let mb = MusicBrainz::new().unwrap();
        let url = find_cover(&mb, "Ben E. King", "Stand by Me")
            .await
            .unwrap()
            .unwrap();
        assert!(
            url.starts_with("https://") && url.contains("archive.org"),
            "{url}"
        );
    }

    #[test]
    fn only_tracks_with_an_artist_are_looked_up() {
        let (key, parsed) = online_query(&on_air("Air - Sexy Boy", None), Some("FIP")).unwrap();
        assert_eq!(parsed.artist.as_deref(), Some("Air"));
        assert_eq!(key, tracks::track_key(&parsed));
        assert!(online_query(&on_air("Jingle", None), Some("FIP")).is_none());
        assert!(online_query(&on_air("fip", None), Some("FIP")).is_none());
        assert!(online_query(&NowPlaying::default(), Some("FIP")).is_none());
    }
}
