// SPDX-License-Identifier: MPL-2.0
//! Cover art of the track on air, from the sources the user put in order in ⚙ › Cover art.
//!
//! * **Station**: the cover the station's page or streaming server gives with the title (recipes,
//!   AzuraCast). Nothing more leaves the device.
//! * **Cover Art Archive**: the track is looked up on MusicBrainz, then the front cover of its album
//!   on the Cover Art Archive. Off by default: the artist and title leave the device.
//!
//! Sources are asked from top to bottom and the first cover found wins; a disabled source is never
//! asked. Without any, the station's logo (its Radio-Browser favicon) is shown. While an online
//! lookup runs, the next source's cover is shown. A result (even "none") is cached in the database,
//! so a track that comes back on air is not looked up again.
//!
//! The desktop's media controls (MPRIS) show the station, not the track: next / previous change the
//! station there. Their picture is the station's logo, or the Soundfolk icon when it has none.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::audio::NowPlaying;
use crate::db::Database;
use crate::media_controls::SystemMedia;
use crate::musicbrainz::{LookupError, MusicBrainz};
use crate::tracks;

/// Setting: the cover sources in order, with their switch (JSON list of [`CoverSource`]).
pub const SETTING_COVER_SOURCES: &str = "cover_sources";
/// Event emitted when the cover of the track on air changes (payload: [`Cover`]).
const COVER_EVENT: &str = "player:cover";
/// After a failed lookup (network), the same track is not looked up again for this long.
const RETRY_FAILED: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Station,
    CoverArtArchive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverSource {
    pub id: SourceId,
    pub enabled: bool,
}

/// The order and switches of a fresh install: only what needs nothing more to leave the device.
pub fn default_sources() -> Vec<CoverSource> {
    vec![
        CoverSource {
            id: SourceId::Station,
            enabled: true,
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
    let stored = db.get_str(SETTING_COVER_SOURCES)?;
    let list = stored
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    Ok(normalize(list))
}

pub fn save_sources(db: &Database, list: Vec<CoverSource>) -> rusqlite::Result<Vec<CoverSource>> {
    let list = normalize(list);
    let json = serde_json::to_string(&list).expect("cover sources serialize");
    db.set_str(SETTING_COVER_SOURCES, &json)?;
    Ok(list)
}

/// The cover shown for a title (`title` as in `NowPlaying.title`, so the window can tell it is
/// still the track on air).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    let parsed = match &now_playing.track {
        Some(track) => tracks::ParsedTitle {
            artist: track.artist.clone(),
            title: track.title.clone(),
        },
        None => tracks::parse(raw, station),
    };
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

/// The first cover found, asking the enabled sources in order, and whether a lookup must run.
fn pick(
    sources: &[CoverSource],
    mut ask: impl FnMut(SourceId) -> Answer,
) -> (Option<String>, bool) {
    let mut lookup = false;
    for source in sources.iter().filter(|s| s.enabled) {
        match ask(source.id) {
            Answer::Found(url) => return (Some(url), lookup),
            Answer::Lookup => lookup = true,
            Answer::None => {}
        }
    }
    (None, lookup)
}

#[derive(Default)]
struct State {
    now_playing: NowPlaying,
    /// The station's name as the user knows it.
    station: Option<String>,
    shown: Cover,
    in_flight: HashSet<String>,
    failed: HashMap<String, Instant>,
    /// Radio-Browser UUID of the station playing.
    station_id: Option<String>,
    /// Logos of the stations played since launch (`None`: the station has none).
    logos: HashMap<String, Option<String>>,
    /// Picture last given to the media controls.
    media_picture: Option<String>,
}

pub struct Covers {
    app: tauri::AppHandle,
    mb: Option<Arc<MusicBrainz>>,
    media: Arc<SystemMedia>,
    /// `file://` address of the Soundfolk icon, for the media controls of a station without a logo.
    app_icon: Option<String>,
    state: Mutex<State>,
}

/// The app icon, shipped in the binary: the media controls need it as a file.
const APP_ICON: &[u8] = include_bytes!("../icons/icon.png");

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
    stream_download::http::reqwest::Url::from_file_path(&path)
        .ok()
        .map(String::from)
}

impl Covers {
    pub fn new(
        app: tauri::AppHandle,
        mb: Option<Arc<MusicBrainz>>,
        media: Arc<SystemMedia>,
    ) -> Arc<Self> {
        let app_icon = app_icon_file(&app);
        Arc::new(Self {
            app,
            mb,
            media,
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
    pub fn update(self: &Arc<Self>, now_playing: NowPlaying, station: Option<String>) {
        {
            let mut state = self.state();
            state.now_playing = now_playing;
            state.station = station;
        }
        self.resolve(true);
    }

    /// A station starts: its logo is fetched once per launch from the catalog, to stand in for
    /// missing cover art. `catalog_id` is `None` for a station outside the catalog (no logo).
    pub fn set_station(self: &Arc<Self>, catalog_id: Option<String>) {
        let fetch = {
            let mut state = self.state();
            state.station_id = catalog_id.clone();
            catalog_id.filter(|id| !state.logos.contains_key(id))
        };
        self.resolve(false);
        let Some(id) = fetch else { return };
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let catalog = this.app.state::<crate::commands::CatalogState>().0.clone();
            let logo = match catalog.station(&id).await {
                Ok(station) => station
                    .and_then(|s| s.favicon)
                    .and_then(|f| web_address(&f)),
                Err(e) => {
                    // Not remembered: tried again the next time the station starts.
                    return tracing::debug!("station logo not found: {e}");
                }
            };
            this.state().logos.insert(id, logo);
            this.resolve(false);
        });
    }

    /// The sources changed: the cover of the track on air is chosen again.
    pub fn refresh(self: &Arc<Self>) {
        self.resolve(false);
    }

    fn resolve(self: &Arc<Self>, title_changed: bool) {
        let (now_playing, station, logo) = {
            let state = self.state();
            let logo = state
                .station_id
                .as_ref()
                .and_then(|id| state.logos.get(id).cloned().flatten());
            (state.now_playing.clone(), state.station.clone(), logo)
        };
        let db = self.app.state::<Database>();
        let sources = load_sources(&db).unwrap_or_else(|e| {
            tracing::warn!("cover sources not read: {e}");
            default_sources()
        });
        let query = online_query(&now_playing, station.as_deref());

        let (url, lookup) = pick(&sources, |source| match source {
            SourceId::Station => station_cover(&now_playing).map_or(Answer::None, Answer::Found),
            SourceId::CoverArtArchive => {
                let Some((key, _)) = &query else {
                    return Answer::None;
                };
                match db.cached_cover(key) {
                    Ok(Some(Some(url))) => Answer::Found(url),
                    Ok(Some(None)) => Answer::None,
                    Ok(None) if self.mb.is_some() => Answer::Lookup,
                    Ok(None) => Answer::None,
                    Err(e) => {
                        tracing::warn!("cover cache not read: {e}");
                        Answer::None
                    }
                }
            }
        });
        if lookup {
            if let Some((key, parsed)) = query {
                self.look_up(key, parsed);
            }
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
                url: logo.clone(),
            },
        };
        let picture = logo.or_else(|| self.app_icon.clone());
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
        if title_changed || picture_changed {
            self.media
                .show_now_playing(&now_playing, station.as_deref(), picture.as_deref());
        }
        if changed {
            if let Err(e) = self.app.emit(COVER_EVENT, cover) {
                tracing::warn!("failed to emit the cover: {e}");
            }
        }
    }

    /// Looks the track up in the background (once at a time, not again soon after a failure), then
    /// chooses the cover again.
    fn look_up(self: &Arc<Self>, key: String, parsed: tracks::ParsedTitle) {
        let Some(mb) = self.mb.clone() else { return };
        {
            let mut state = self.state();
            state.failed.retain(|_, at| at.elapsed() < RETRY_FAILED);
            if state.failed.contains_key(&key) || !state.in_flight.insert(key.clone()) {
                return;
            }
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let artist = parsed.artist.as_deref().unwrap_or_default();
            let result = find_cover(&mb, artist, &parsed.title).await;
            let db = this.app.state::<Database>();
            match &result {
                Ok(url) => {
                    if let Err(e) = db.save_cover(&key, url.as_deref()) {
                        tracing::warn!("cover not cached: {e}");
                    }
                }
                Err(e) => tracing::debug!("cover lookup failed: {e}"),
            }
            {
                let mut state = this.state();
                state.in_flight.remove(&key);
                if result.is_err() {
                    state.failed.insert(key, Instant::now());
                }
            }
            this.resolve(false);
        });
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
            (Some("s".into()), false)
        );
        assert_eq!(
            pick(&both(a, s), answers(found("s"), found("a"))),
            (Some("a".into()), false)
        );
        // The archive must be asked first: the station's cover is shown meanwhile
        assert_eq!(
            pick(&both(a, s), answers(found("s"), Answer::Lookup)),
            (Some("s".into()), true)
        );
        // The station has a cover: no need to look anything up
        assert_eq!(
            pick(&both(s, a), answers(found("s"), Answer::Lookup)),
            (Some("s".into()), false)
        );
        assert_eq!(
            pick(&both(s, a), answers(Answer::None, Answer::None)),
            (None, false)
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
            (None, false)
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
