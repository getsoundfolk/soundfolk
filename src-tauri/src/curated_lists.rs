// SPDX-License-Identifier: MPL-2.0
//! Curated lists of stations, from `getsoundfolk/soundfolk-stations`: every `lists/<id>.toml` of
//! that repository, compiled into one `lists.json` by its CI and published on GitHub Pages. See
//! `docs/14-curated-lists.md`.
//!
//! * At startup, the copy cached by the last download is used right away (offline included).
//! * The file is downloaded again when that copy is a day old (see `daily_list.rs`), like the
//!   donation links.
//! * A file that does not parse, or of another `version`, is ignored and the cached one stays.
//!   Unknown keys are ignored (no `deny_unknown_fields` here: new optional fields keep `v1`), and a
//!   list this version cannot read is skipped on its own.
//!
//! `SOUNDFOLK_LISTS_URL` replaces the file's address (a fork's lists, a local file server).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use soundfolk_nowplaying::recipe::Match;

use crate::catalog::{Health, Station};
use crate::commands::{save_download, with_db, CatalogState, ExportedFile};
use crate::daily_list::{self, Files, CHECK_EVERY, FIRST_CHECK};
use crate::db::Database;
use crate::error::AppResult;

/// Where the lists are published. `v1` is the format: a change older apps can't read moves it.
pub const DEFAULT_URL: &str = "https://getsoundfolk.github.io/soundfolk-stations/v1/lists.json";
const VERSION: u32 = 1;

const FILES: Files = Files {
    list: "lists.json",
    etag: "lists.etag",
};

#[derive(Debug, Deserialize)]
struct File {
    version: u32,
    /// Read one by one by [`parse`].
    lists: Vec<serde_json::Value>,
}

/// One list, as published.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CuratedList {
    pub id: String,
    pub title: String,
    pub description: String,
    pub curators: Vec<String>,
    #[serde(rename = "use", default)]
    pub uses: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub countries: Vec<String>,
    pub items: Vec<Item>,
}

/// A station of a list: its Radio-Browser UUID, and the stream of that entry, which also
/// recognises the other Radio-Browser copies of the station playing the same stream.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Item {
    pub uuid: String,
    #[serde(default)]
    pub stream: Option<String>,
}

impl Item {
    fn matches(&self, station_id: Option<&str>, stream: &str) -> bool {
        Match {
            stations: vec![self.uuid.clone()],
            streams: self.stream.iter().cloned().collect(),
        }
        .matches(station_id, stream)
    }
}

/// What the station sheet shows of a list the station is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListRef {
    pub id: String,
    pub title: String,
}

fn parse(text: &str) -> Result<Vec<CuratedList>, String> {
    let file: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!(
            "version {} (this app reads {VERSION})",
            file.version
        ));
    }
    Ok(file
        .lists
        .into_iter()
        .filter_map(|list| serde_json::from_value::<CuratedList>(list).ok())
        .filter(|list| !list.id.is_empty() && !list.title.trim().is_empty())
        .collect())
}

/// The lists known so far, in the published order.
#[derive(Debug, Default)]
pub struct CuratedLists(RwLock<Vec<CuratedList>>);

impl CuratedLists {
    fn set(&self, lists: Vec<CuratedList>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = lists;
    }

    pub fn all(&self) -> Vec<CuratedList> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn get(&self, id: &str) -> Option<CuratedList> {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|l| l.id == id)
            .cloned()
    }

    /// The lists this station (Radio-Browser UUID) or stream is in, in the published order.
    pub fn for_station(&self, station_id: Option<&str>, stream: &str) -> Vec<ListRef> {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|l| l.items.iter().any(|i| i.matches(station_id, stream)))
            .map(|l| ListRef {
                id: l.id.clone(),
                title: l.title.clone(),
            })
            .collect()
    }
}

/// Loads the cached lists, then keeps them up to date in the background. `on_change` runs after
/// each download that changed them.
pub fn start(
    lists: Arc<CuratedLists>,
    cache_dir: Option<PathBuf>,
    on_change: impl Fn() + Send + 'static,
) {
    let Some(dir) = cache_dir else { return };
    if let Some(cached) = read_cached(&dir) {
        tracing::info!("curated lists: {} from the cache", cached.len());
        lists.set(cached);
    }
    let url = std::env::var("SOUNDFOLK_LISTS_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    tauri::async_runtime::spawn(async move {
        let client = match daily_list::client() {
            Ok(client) => client,
            Err(e) => return tracing::warn!("curated lists unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if daily_list::is_due(&dir, FILES) {
                match daily_list::refresh(&client, &url, &dir, FILES, parse).await {
                    Ok(Some(fresh)) => {
                        tracing::info!("curated lists: {} downloaded", fresh.len());
                        lists.set(fresh);
                        on_change();
                    }
                    Ok(None) => tracing::debug!("curated lists: unchanged"),
                    Err(e) => tracing::warn!("curated lists: {e}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Every list, in the published order (the app sorts them by title).
#[tauri::command]
pub async fn get_curated_lists(
    lists: tauri::State<'_, Arc<CuratedLists>>,
) -> AppResult<Vec<CuratedList>> {
    Ok(lists.all())
}

/// The lists the station being played or shown is in.
#[tauri::command]
pub async fn get_lists_for_station(
    lists: tauri::State<'_, Arc<CuratedLists>>,
    station_id: Option<String>,
    url: String,
) -> AppResult<Vec<ListRef>> {
    Ok(lists.for_station(station_id.as_deref(), &url))
}

/// A list's stations from the catalog, in the curator's order, the ones the catalog's last check
/// could not reach ranked last (shown with a warning).
#[tauri::command]
pub async fn get_curated_list_stations(
    lists: tauri::State<'_, Arc<CuratedLists>>,
    catalog: tauri::State<'_, CatalogState>,
    id: String,
) -> AppResult<Vec<Station>> {
    let list = lists.get(&id).ok_or_else(|| format!("no list {id}"))?;
    stations_of(&list, &catalog).await
}

/// Saves a list as an M3U playlist in the Downloads folder, for another player.
#[tauri::command]
pub async fn export_curated_list(
    app: tauri::AppHandle,
    lists: tauri::State<'_, Arc<CuratedLists>>,
    catalog: tauri::State<'_, CatalogState>,
    id: String,
) -> AppResult<ExportedFile> {
    let list = lists.get(&id).ok_or_else(|| format!("no list {id}"))?;
    let stations = stations_of(&list, &catalog).await?;
    save_download(
        &app,
        &format!("soundfolk-{}", list.id),
        "m3u",
        &m3u(stations.iter().map(|s| (s.name.as_str(), s.url.as_str()))),
        stations.len(),
    )
}

/// Settings key of the lists marked as favorites: their ids, in the order they were marked.
const FAVORITE_LISTS: &str = "favorite_lists";

/// The ids of the lists the listener marked as favorites (14 §6.4). Only the id is kept: a list
/// that left `lists.json` is simply not shown, and comes back if it is published again.
#[tauri::command]
pub async fn get_favorite_lists(db: tauri::State<'_, Database>) -> AppResult<Vec<String>> {
    with_db(&db, |db| {
        Ok(db.get_list(FAVORITE_LISTS)?.unwrap_or_default())
    })
    .await
}

/// Marks or unmarks a list as a favorite; returns the favorite list ids.
#[tauri::command]
pub async fn set_favorite_list(
    db: tauri::State<'_, Database>,
    id: String,
    favorite: bool,
) -> AppResult<Vec<String>> {
    if !is_list_id(&id) {
        return Err(format!("not a list id: {id}").into());
    }
    with_db(&db, move |db| {
        let mut ids = db.get_list(FAVORITE_LISTS)?.unwrap_or_default();
        ids.retain(|i| *i != id);
        if favorite {
            ids.push(id);
        }
        db.set_list(FAVORITE_LISTS, &ids)?;
        Ok(ids)
    })
    .await
}

/// A list id as soundfolk-stations writes them (lower case, digits, `-`): never a comma, which
/// separates the ids in the setting.
fn is_list_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The listener's stored lists that can be exported, like a curated list.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MyList {
    Favorites,
    MyStations,
}

/// Saves the favorites (stations only) or the personal stations as an M3U playlist.
#[tauri::command]
pub async fn export_my_list(
    app: tauri::AppHandle,
    db: tauri::State<'_, Database>,
    list: MyList,
) -> AppResult<ExportedFile> {
    let entries: Vec<(String, String)> = with_db(&db, move |db| {
        Ok(match list {
            MyList::Favorites => db
                .list_favorites()?
                .into_iter()
                .map(|f| (f.name, f.stream_url))
                .collect(),
            MyList::MyStations => db
                .list_personal_stations()?
                .into_iter()
                .map(|p| (p.name, p.stream_url))
                .collect(),
        })
    })
    .await?;
    let stem = match list {
        MyList::Favorites => "soundfolk-favorites",
        MyList::MyStations => "soundfolk-my-stations",
    };
    save_download(
        &app,
        stem,
        "m3u",
        &m3u(entries.iter().map(|(n, u)| (n.as_str(), u.as_str()))),
        entries.len(),
    )
}

async fn stations_of(list: &CuratedList, catalog: &CatalogState) -> AppResult<Vec<Station>> {
    let ids: Vec<String> = list.items.iter().map(|i| i.uuid.clone()).collect();
    let mut stations = catalog.0.stations(&ids).await?;
    // Stable: the curator's order within each group.
    stations.sort_by_key(|s| matches!(s.health, Health::Unreachable { .. }));
    Ok(stations)
}

fn read_cached(dir: &Path) -> Option<Vec<CuratedList>> {
    daily_list::read_cached(dir, FILES, parse)
}

/// M3U playlist of `(name, stream URL)` pairs, in their order.
pub fn m3u<'a>(stations: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut out = String::from("#EXTM3U\n");
    for (name, url) in stations {
        // A line break in a name would start a line the players read as a URL.
        let name = name.replace(['\r', '\n'], " ");
        out.push_str(&format!("#EXTINF:-1,{name}\n{url}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{"version": 1, "future": true, "lists": [
        {"id": "ambient-focus", "title": "Ambient to focus", "description": "Few voices.",
         "curators": ["raphaelh"], "use": ["focus"], "genres": ["ambient"], "countries": ["US"],
         "items": [
           {"uuid": "960cf833-0601-11e8-ae97-52543be04c81", "stream": "ice2.somafm.com/groovesalad-128-mp3"},
           {"uuid": "ae7aeb65-5a30-4848-8059-91b2bc2dcfd9", "stream": "ice6.somafm.com/dronezone-256-mp3", "note": "x"}
         ], "cover": "later"},
        {"id": "broken", "title": 3, "items": []},
        "not a list",
        {"id": "fr-community-radios", "title": "Radios associatives en France", "description": "…",
         "curators": ["raphaelh"], "use": ["discovery", "shows"], "genres": [], "countries": ["FR"],
         "items": [{"uuid": "960cf833-0601-11e8-ae97-52543be04c81"}]}
    ]}"#;

    fn lists() -> CuratedLists {
        let lists = CuratedLists::default();
        lists.set(parse(FILE).unwrap());
        lists
    }

    #[test]
    fn unknown_keys_are_ignored_and_unreadable_lists_skipped() {
        let lists = parse(FILE).unwrap();
        assert_eq!(
            lists.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["ambient-focus", "fr-community-radios"]
        );
        assert_eq!(lists[0].uses, ["focus"]);
        assert_eq!(lists[0].items.len(), 2);
    }

    #[test]
    fn another_version_or_a_broken_file_is_refused() {
        assert!(parse(r#"{"version": 2, "lists": []}"#).is_err());
        assert!(parse("<html>").is_err());
        assert!(parse(r#"{"version": 1, "lists": []}"#).unwrap().is_empty());
    }

    #[test]
    fn a_station_is_found_by_uuid_in_every_list_that_has_it() {
        let found = lists().for_station(
            Some("960CF833-0601-11E8-AE97-52543BE04C81"),
            "https://other.example/live",
        );
        assert_eq!(
            found.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["ambient-focus", "fr-community-radios"]
        );
    }

    #[test]
    fn another_copy_of_a_station_is_found_by_its_stream() {
        let found = lists().for_station(
            Some("00000000-0000-0000-0000-000000000000"),
            "https://ice6.somafm.com/dronezone-256-mp3?x=1",
        );
        assert_eq!(
            found,
            [ListRef {
                id: "ambient-focus".into(),
                title: "Ambient to focus".into()
            }]
        );
        assert!(lists()
            .for_station(None, "https://ice6.somafm.com/other")
            .is_empty());
    }

    #[test]
    fn m3u_keeps_the_order_and_one_line_per_name() {
        assert_eq!(
            m3u([
                ("Groove\nSalad", "https://a.example/1"),
                ("FIP", "https://b.example/2")
            ]),
            "#EXTM3U\n#EXTINF:-1,Groove Salad\nhttps://a.example/1\n#EXTINF:-1,FIP\nhttps://b.example/2\n"
        );
    }

    #[test]
    fn list_ids_never_hold_a_comma() {
        assert!(is_list_id("fr-community-radios"));
        assert!(is_list_id("top40"));
        assert!(!is_list_id(""));
        assert!(!is_list_id("a,b"));
        assert!(!is_list_id("Ambient"));
    }

    #[test]
    fn the_cache_is_read_back() {
        let dir = std::env::temp_dir().join(format!("soundfolk-lists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_cached(&dir).is_none());
        std::fs::write(dir.join(FILES.list), FILE).unwrap();
        assert_eq!(read_cached(&dir).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
