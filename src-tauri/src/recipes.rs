// SPDX-License-Identifier: MPL-2.0
//! Now-playing recipes available to the app, highest priority first: the user's own
//! (`<config dir>/recipes/*.toml`), the shared list downloaded from `getsoundfolk/soundfolk-recipes`
//! (see [`crate::recipe_list`]), and those shipped with the app (`src-tauri/recipes/stations`).
//! See `docs/08-now-playing.md`.
//!
//! A recipe's **id** is its file name without `.toml` (`frequence7`). A recipe replaces the ones
//! with the same id further down, whatever its `match` says; otherwise the order above decides.
//! An invalid local file is reported and the recipe below it stays in use.
//!
//! Recipes are parsed once and kept in memory; [`Recipes::reload_if_changed`] picks up edits to
//! the local folder. The app calls it when a station starts, before a test reading, and when its
//! window gets the focus back (from the editor): the folder is never polled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use serde::Serialize;
use soundfolk_nowplaying::recipe::{LoadError, Recipe};
use soundfolk_nowplaying::Timestamp;

use crate::error::{AppResult, IoContext};
use crate::files::{numbered, write_new};

include!(concat!(env!("OUT_DIR"), "/bundled_recipes.rs"));

/// Name of the JSON Schema written next to local recipes (editor completion through Taplo).
pub const SCHEMA_FILE: &str = "recipe.schema.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Shipped with the app.
    Bundled,
    /// From the shared list (`getsoundfolk/soundfolk-recipes`), downloaded by the app.
    Shared,
    /// In the user's recipes folder.
    Local,
}

/// A loaded recipe.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub origin: Origin,
    /// The file, for a local recipe.
    pub path: Option<PathBuf>,
    pub recipe: Arc<Recipe>,
}

/// A local file that could not be used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileError {
    pub id: String,
    pub path: PathBuf,
    pub error: String,
    /// The file's text as read when the set was loaded (empty if unreadable), to tell which
    /// station a broken file is for without reading it again.
    #[serde(skip)]
    pub text: String,
}

/// The recipes in use, highest priority first.
#[derive(Debug, Default)]
pub struct RecipeSet {
    entries: Vec<Entry>,
    errors: Vec<FileError>,
    local: usize,
}

impl RecipeSet {
    /// The recipe for a station (Radio-Browser UUID) or stream URL.
    pub fn find(&self, station_id: Option<&str>, stream: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.recipe.matches(station_id, stream))
    }

    #[cfg(test)]
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Local files that could not be used.
    pub fn errors(&self) -> &[FileError] {
        &self.errors
    }

    pub fn error_for(&self, id: &str) -> Option<&FileError> {
        self.errors.iter().find(|e| e.id == id)
    }

    /// Number of local files (valid or not).
    pub fn local_count(&self) -> usize {
        self.local
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A local file that appeared or changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub id: String,
    pub path: PathBuf,
    /// Why it cannot be used, if it cannot.
    pub error: Option<String>,
}

type Fingerprint = HashMap<PathBuf, (Option<SystemTime>, u64)>;

/// What the set in use is built from, besides the shipped recipes.
#[derive(Default)]
struct Layers {
    fingerprint: Fingerprint,
    local: Vec<Entry>,
    errors: Vec<FileError>,
    local_count: usize,
    shared: Vec<Entry>,
}

/// All recipes, rebuilt when the local folder or the shared list changes.
pub struct Recipes {
    local_dir: Option<PathBuf>,
    bundled: Vec<Entry>,
    current: RwLock<Arc<RecipeSet>>,
    /// Incremented every time the recipes in use change.
    generation: tokio::sync::watch::Sender<u64>,
    layers: Mutex<Layers>,
}

impl Recipes {
    pub fn new(local_dir: Option<PathBuf>) -> Self {
        let bundled = BUNDLED
            .iter()
            .filter_map(|(file, text)| {
                let id = file.trim_end_matches(".toml").to_owned();
                parse(text)
                    .inspect_err(|e| tracing::warn!("bundled recipe {file}: {e}"))
                    .ok()
                    .map(|recipe| Entry {
                        id,
                        origin: Origin::Bundled,
                        path: None,
                        recipe: Arc::new(recipe),
                    })
            })
            .collect();
        let recipes = Self {
            local_dir,
            bundled,
            current: RwLock::default(),
            generation: tokio::sync::watch::Sender::new(0),
            layers: Mutex::default(),
        };
        recipes.reload_if_changed();
        recipes
    }

    pub fn local_dir(&self) -> Option<&Path> {
        self.local_dir.as_deref()
    }

    /// The recipes in use now.
    pub fn current(&self) -> Arc<RecipeSet> {
        self.current
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Incremented every time the recipes in use change.
    pub fn generation(&self) -> u64 {
        *self.generation.borrow()
    }

    /// Sees each change of [`Recipes::generation`] made after this call.
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// The shipped TOML of a recipe.
    pub(crate) fn bundled_text(&self, id: &str) -> Option<&'static str> {
        BUNDLED
            .iter()
            .find(|(file, _)| file.trim_end_matches(".toml") == id)
            .map(|(_, text)| *text)
    }

    /// A shipped or shared recipe as a local file to edit (see [`local_copy`]).
    pub fn local_copy_of(&self, entry: &Entry) -> Option<String> {
        match entry.origin {
            Origin::Bundled => self.bundled_text(&entry.id).map(local_copy),
            Origin::Shared => Some(shared_copy(entry)),
            Origin::Local => None,
        }
    }

    /// Whether a shipped or shared recipe has this id (a new local file must not take it).
    pub fn is_known_id(&self, id: &str) -> bool {
        self.bundled.iter().any(|e| e.id == id)
            || self.lock_layers().shared.iter().any(|e| e.id == id)
    }

    /// Number of recipes in the shared list in use.
    #[cfg(test)]
    pub fn shared_count(&self) -> usize {
        self.lock_layers().shared.len()
    }

    /// Replaces the shared list (downloaded or read from the cache). The station playing switches
    /// to its new recipe, if it has one.
    pub fn set_shared(&self, recipes: Vec<(String, Recipe)>) {
        let mut layers = self.lock_layers();
        layers.shared = recipes
            .into_iter()
            .map(|(id, recipe)| Entry {
                id,
                origin: Origin::Shared,
                path: None,
                recipe: Arc::new(recipe),
            })
            .collect();
        self.publish(&layers);
    }

    fn lock_layers(&self) -> std::sync::MutexGuard<'_, Layers> {
        self.layers.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Builds the set in use: local, then shared, then shipped; an id appears once, from the
    /// highest layer that has it.
    fn publish(&self, layers: &Layers) {
        let mut seen = std::collections::HashSet::new();
        let entries = layers
            .local
            .iter()
            .chain(&layers.shared)
            .chain(&self.bundled)
            .filter(|e| seen.insert(e.id.clone()))
            .cloned()
            .collect();
        let set = RecipeSet {
            local: layers.local_count,
            entries,
            errors: layers.errors.clone(),
        };
        *self.current.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(set);
        self.generation.send_modify(|g| *g += 1);
    }

    /// Reloads the local folder if a file appeared, changed or disappeared. Returns the files that
    /// appeared or changed (`None`: nothing changed).
    pub fn reload_if_changed(&self) -> Option<Vec<Change>> {
        let files = self
            .local_dir
            .as_deref()
            .map(list_local)
            .unwrap_or_default();
        let fingerprint: Fingerprint = files
            .iter()
            .map(|path| {
                let meta = std::fs::metadata(path).ok();
                (
                    path.clone(),
                    (
                        meta.as_ref().and_then(|m| m.modified().ok()),
                        meta.map_or(0, |m| m.len()),
                    ),
                )
            })
            .collect();
        let mut layers = self.lock_layers();
        let first = self.generation() == 0;
        if !first && layers.fingerprint == fingerprint {
            return None;
        }
        let changed: Vec<&PathBuf> = files
            .iter()
            .filter(|p| !first && layers.fingerprint.get(*p) != fingerprint.get(*p))
            .collect();

        let mut local = Vec::new();
        let mut errors = Vec::new();
        for path in &files {
            let id = file_id(path);
            let text = std::fs::read_to_string(path);
            let result = text
                .as_ref()
                .map_err(|e| e.to_string())
                .and_then(|text| parse(text));
            match result {
                Ok(recipe) => local.push(Entry {
                    id,
                    origin: Origin::Local,
                    path: Some(path.clone()),
                    recipe: Arc::new(recipe),
                }),
                Err(error) => {
                    tracing::warn!("recipe {}: {error}", path.display());
                    errors.push(FileError {
                        id,
                        path: path.clone(),
                        error,
                        text: text.unwrap_or_default(),
                    });
                }
            }
        }
        let changes = changed
            .into_iter()
            .map(|path| {
                let id = file_id(path);
                let error = errors
                    .iter()
                    .find(|e| e.path == *path)
                    .map(|e| e.error.clone());
                Change {
                    id,
                    path: path.clone(),
                    error,
                }
            })
            .collect();

        layers.fingerprint = fingerprint;
        layers.local = local;
        layers.errors = errors;
        layers.local_count = files.len();
        self.publish(&layers);
        Some(changes)
    }
}

fn list_local(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file())
        .collect();
    paths.sort();
    paths
}

fn file_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parse(text: &str) -> Result<Recipe, String> {
    Recipe::from_toml(text).map_err(|e| match e {
        LoadError::NewerSchema(_) => format!("{e} (update Soundfolk to use it)"),
        LoadError::Invalid(e) => e,
    })
}

/// A file name for a new recipe, from the station name: lowercase ASCII words joined by `-`.
pub fn slug(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = name
        .nfkd()
        .filter(char::is_ascii)
        .collect::<String>()
        .to_ascii_lowercase();
    let words: Vec<&str> = folded
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let slug = words.join("-");
    if slug.is_empty() {
        "station".into()
    } else {
        slug.chars()
            .take(48)
            .collect::<String>()
            .trim_end_matches('-')
            .to_owned()
    }
}

/// What to put in a new recipe file.
pub struct Template<'a> {
    pub name: &'a str,
    /// Radio-Browser UUID.
    pub station_id: Option<&'a str>,
    pub stream: &'a str,
    pub homepage: Option<&'a str>,
}

/// A recipe to fill in: what is known about the station is written, the rest is explained.
pub fn template(t: &Template) -> String {
    let quote = |s: &str| toml_string(s);
    let mut matches = Vec::new();
    if let Some(id) = t.station_id {
        matches.push(format!("stations = [{}]", quote(id)));
    }
    if let Some(stream) = soundfolk_nowplaying::recipe::stream_pattern(t.stream) {
        matches.push(format!("streams = [{}]", quote(&stream)));
    }
    let homepage = t.homepage.filter(|h| h.starts_with("http")).unwrap_or("");
    format!(
        r#"#:schema {SCHEMA_FILE}
# Where the website of {name} shows the track on air, and how to read it.
# Save the file: Soundfolk reloads it and reads the page again while the station plays.
# Format: https://github.com/getsoundfolk/soundfolk/blob/main/docs/08-now-playing.md
# Works? Share it with every listener: https://github.com/getsoundfolk/soundfolk-recipes
schema = 1
name = {name_toml}
match = {{ {matches} }}
# The page (or JSON API) showing the track on air. Often not the homepage: look for a
# "now playing", "playlist" or "titres diffusés" page, or in your browser's network tools.
fetch = {{ url = {homepage} }}

[extract]
type = "html"     # html (CSS selectors), json (dotted paths) or text (one regex)
item = ""         # the element holding the track on air, e.g. "li.current"
artist = ""       # inside the item, e.g. ".artist"
title = ""        # or text = "…" when the page shows "Artist - Title" in one piece
# start = ""      # when the track started: Unix time, ISO 8601 or the station's local HH:MM
# duration = ""   # seconds, MM:SS or ISO 8601
# next = ""       # the upcoming tracks, in order, read with the same fields
"#,
        name = t.name,
        name_toml = quote(t.name),
        matches = matches.join(", "),
        homepage = quote(homepage),
    )
}

/// The shared recipes repository.
pub const REPOSITORY: &str = "https://github.com/getsoundfolk/soundfolk-recipes";
/// Longest link opened in the browser: GitHub refuses longer ones.
pub const MAX_LINK: usize = 7500;

/// The "Share a recipe" issue form, filled in (the fixture is left out when `None`). The
/// repository's `share` workflow turns the issue into a pull request.
pub fn share_link(id: &str, name: &str, recipe: &str, fixture: Option<&str>) -> String {
    let mut params = vec![
        ("template", "share.yml".to_owned()),
        ("title", format!("Recipe: {name}")),
        ("recipe-id", id.to_owned()),
        ("recipe", recipe.to_owned()),
    ];
    if let Some(fixture) = fixture {
        params.push(("fixture", fixture.to_owned()));
    }
    crate::audio::Url::parse_with_params(&format!("{REPOSITORY}/issues/new"), &params)
        .expect("valid URL")
        .to_string()
}

/// A recipe of the shared list as a local file (the list carries no comments).
fn shared_copy(entry: &Entry) -> String {
    format!(
        "#:schema {SCHEMA_FILE}\n# {name}: copy of the shared recipe `{id}`. This file replaces it on this device.\n\
         # Improved it? Share it: https://github.com/getsoundfolk/soundfolk-recipes\n{toml}",
        name = entry.recipe.name,
        id = entry.id,
        toml = entry.recipe.to_toml(),
    )
}

/// A shipped recipe as a local file: the schema line points next to it, fixtures are dropped
/// (they live in the app's sources, not next to the copy).
pub fn local_copy(text: &str) -> String {
    let mut out = String::new();
    let mut in_test = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#:schema") {
            out.push_str(&format!("#:schema {SCHEMA_FILE}\n"));
            continue;
        }
        if trimmed.starts_with("[[test]]") {
            in_test = true;
            continue;
        }
        if in_test && trimmed.starts_with('[') {
            in_test = false;
        }
        if !in_test {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_owned() + "\n"
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Local recipe files (Edit recipe, Share this recipe) ─────────────────────────

/// The local recipes folder, created with the JSON Schema next to the recipes (completion and
/// checks in editors that read the `#:schema` line).
pub fn folder(recipes: &Recipes) -> AppResult<PathBuf> {
    let dir = recipes
        .local_dir()
        .ok_or_else(|| "no configuration folder on this system".to_owned())?
        .to_path_buf();
    std::fs::create_dir_all(&dir).at(&dir)?;
    let schema = serde_json::to_string_pretty(&soundfolk_nowplaying::recipe::json_schema())
        .expect("schema serializes");
    let schema_path = dir.join(SCHEMA_FILE);
    if std::fs::read_to_string(&schema_path).ok().as_deref() != Some(schema.as_str()) {
        std::fs::write(&schema_path, schema).at(&schema_path)?;
    }
    Ok(dir)
}

/// A station, as *Edit recipe* knows it.
pub struct EditedStation<'a> {
    /// Catalog id, if the station has one.
    pub id: Option<&'a str>,
    pub stream: &'a str,
    pub name: Option<&'a str>,
    pub homepage: Option<&'a str>,
    /// The station's local file that does not load (see `WebTitles::local_error_for`).
    pub broken: Option<PathBuf>,
}

/// The local file to open for the station's recipe: the local recipe in use, a copy of the
/// shipped or shared one, the station's local file that does not load yet, or a new recipe to
/// fill in. Files written here are picked up at once, without a "saved" notice.
pub fn file_to_edit(recipes: &Recipes, station: EditedStation) -> AppResult<PathBuf> {
    let dir = folder(recipes)?;
    let existing = recipes.current().find(station.id, station.stream).cloned();
    let path = match existing {
        Some(entry) if entry.origin == Origin::Local => entry.path.clone().unwrap_or_default(),
        Some(entry) => {
            let path = dir.join(format!("{}.toml", entry.id));
            if !path.exists() {
                let text = recipes
                    .local_copy_of(&entry)
                    .ok_or_else(|| "recipe not found".to_owned())?;
                std::fs::write(&path, text).at(&path)?;
            }
            path
        }
        None => match station.broken {
            Some(broken) => broken,
            None => {
                let name = station.name.map(str::trim).filter(|n| !n.is_empty());
                let host = crate::audio::Url::parse(station.stream)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned));
                let base = slug(name.or(host.as_deref()).unwrap_or("station"));
                let text = template(&Template {
                    name: name.or(host.as_deref()).unwrap_or("Station"),
                    station_id: station.id,
                    stream: station.stream,
                    homepage: station.homepage,
                });
                write_new(
                    numbered(&base)
                        .filter(|id| !recipes.is_known_id(id))
                        .map(|id| dir.join(format!("{id}.toml"))),
                    &text,
                )?
            }
        },
    };
    // Written by the app, not by the user: no "saved" notice for it.
    recipes.reload_if_changed();
    Ok(path)
}

/// The station's recipe and its file's text, if it is one of the user's own: only those can be
/// shared.
pub fn local_recipe(
    recipes: &Recipes,
    station_id: Option<&str>,
    stream: &str,
) -> AppResult<(Entry, String)> {
    let entry = recipes
        .current()
        .find(station_id, stream)
        .cloned()
        .ok_or_else(|| "No recipe matches this station.".to_owned())?;
    let path = match (&entry.origin, &entry.path) {
        (Origin::Local, Some(path)) => path.clone(),
        _ => {
            return Err(
                "Only your own recipes can be shared: edit this one locally first."
                    .to_owned()
                    .into(),
            )
        }
    };
    let text = std::fs::read_to_string(&path).at(&path)?;
    Ok((entry, text))
}

/// A recipe ready to share.
pub struct PreparedShare {
    /// Where the recipe and fixture were saved, laid out as in the repository (`stations/`,
    /// `fixtures/`), for a pull request by hand.
    pub folder: PathBuf,
    /// The "Share a recipe" issue form, filled in.
    pub link: String,
    /// The fixture, when it was too long for the link: to paste in the form.
    pub fixture_to_paste: Option<String>,
}

/// Prepares a local recipe for the shared list: keeps the excerpt of `page` the recipe reads as
/// its fixture, writes the `[[test]]`, saves both under `to-share/`, and builds the issue link.
pub fn prepare_share(
    recipes: &Recipes,
    entry: &Entry,
    text: &str,
    page_url: &str,
    page: &str,
    at: Timestamp,
) -> AppResult<PreparedShare> {
    let shared =
        soundfolk_nowplaying::fixture::prepare(&entry.id, text, &entry.recipe, page_url, page, at)
            .map_err(|e| format!("Not ready to share: {e}."))?;

    let folder = folder(recipes)?.join("to-share");
    let recipe_path = folder.join("stations").join(format!("{}.toml", entry.id));
    let fixture_path = folder.join("fixtures").join(&shared.fixture_file);
    for path in [&recipe_path, &fixture_path] {
        let parent = path.parent().expect("a file in a folder");
        std::fs::create_dir_all(parent).at(parent)?;
    }
    std::fs::write(&recipe_path, &shared.recipe).at(&recipe_path)?;
    std::fs::write(&fixture_path, &shared.fixture).at(&fixture_path)?;

    let name = &entry.recipe.name;
    let link = share_link(&entry.id, name, &shared.recipe, Some(&shared.fixture));
    if link.len() <= MAX_LINK {
        return Ok(PreparedShare {
            folder,
            link,
            fixture_to_paste: None,
        });
    }
    Ok(PreparedShare {
        folder,
        link: share_link(&entry.id, name, &shared.recipe, None),
        fixture_to_paste: Some(shared.fixture),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("soundfolk-recipes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frequence7() -> &'static str {
        BUNDLED
            .iter()
            .find(|(n, _)| *n == "frequence7.toml")
            .unwrap()
            .1
    }

    #[test]
    fn bundled_recipes_are_valid_and_pass_their_tests() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/stations");
        assert!(!BUNDLED.is_empty());
        for (name, text) in BUNDLED {
            let recipe = Recipe::from_toml(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!recipe.tests.is_empty(), "{name}: no [[test]]");
            for test in &recipe.tests {
                let page = std::fs::read_to_string(dir.join(&test.page)).unwrap();
                let mut failures = soundfolk_nowplaying::check::run_test(&recipe, test, &page);
                failures.extend(soundfolk_nowplaying::fixture::oversized(&recipe, &page));
                assert!(failures.is_empty(), "{name} ({}): {failures:?}", test.page);
            }
        }
    }

    #[test]
    fn published_schema_is_up_to_date() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/recipe.schema.json");
        let published: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            published,
            soundfolk_nowplaying::recipe::json_schema(),
            "regenerate: cargo run -p soundfolk-nowplaying --bin recipe-check -- schema > recipes/recipe.schema.json"
        );
    }

    #[test]
    fn local_recipes_come_first_and_replace_shipped_ones_by_id() {
        let dir = temp_dir("order");
        std::fs::write(
            dir.join("mine.toml"),
            frequence7().replace("name = \"Fréquence 7\"", "name = \"Mine\""),
        )
        .unwrap();
        std::fs::write(dir.join("broken.toml"), "schema = 1\nname = ").unwrap();
        let recipes = Recipes::new(Some(dir.clone()));
        let set = recipes.current();
        assert_eq!(
            set.find(None, "https://live.aurafm.org/Frequence7")
                .unwrap()
                .id,
            "mine"
        );
        // Shipped recipes, plus "mine"; the broken file counts as local too.
        assert_eq!((set.len(), set.local_count()), (BUNDLED.len() + 1, 2));
        assert_eq!(set.errors()[0].id, "broken");

        // Same id as the shipped recipe: replaces it even when it matches another stream.
        let moved =
            local_copy(frequence7()).replace("live.aurafm.org/Frequence7", "example.org/other");
        std::fs::write(dir.join("frequence7.toml"), moved).unwrap();
        recipes.reload_if_changed().unwrap();
        let set = recipes.current();
        assert_eq!(set.get("frequence7").unwrap().origin, Origin::Local);
        assert_eq!(set.len(), BUNDLED.len() + 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reloads_report_what_changed() {
        let dir = temp_dir("reload");
        let recipes = Recipes::new(Some(dir.clone()));
        let start = recipes.generation();
        assert_eq!(recipes.reload_if_changed(), None);

        std::fs::write(dir.join("frequence7.toml"), "schema = 1\nname = ").unwrap();
        let changes = recipes.reload_if_changed().unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].error.is_some());
        // Invalid local file: the shipped recipe stays in use, the error is reported.
        let set = recipes.current();
        assert_eq!(set.get("frequence7").unwrap().origin, Origin::Bundled);
        assert!(set.error_for("frequence7").is_some());

        std::fs::write(dir.join("frequence7.toml"), local_copy(frequence7())).unwrap();
        let changes = recipes.reload_if_changed().unwrap();
        assert_eq!(changes[0].error, None);
        assert_eq!(
            recipes.current().get("frequence7").unwrap().origin,
            Origin::Local
        );

        std::fs::remove_file(dir.join("frequence7.toml")).unwrap();
        assert_eq!(recipes.reload_if_changed(), Some(Vec::new()));
        assert_eq!(
            recipes.current().get("frequence7").unwrap().origin,
            Origin::Bundled
        );
        assert_eq!(recipes.generation(), start + 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shared_recipes_sit_between_local_and_shipped_ones() {
        let dir = temp_dir("shared");
        let recipes = Recipes::new(Some(dir.clone()));
        let shipped = Recipe::from_toml(frequence7()).unwrap();
        let other = Recipe::from_toml(
            &frequence7().replace("live.aurafm.org/Frequence7", "example.org/other"),
        )
        .unwrap();
        let start = recipes.generation();
        recipes.set_shared(vec![
            ("frequence7".into(), other.published()),
            ("new".into(), shipped.published()),
        ]);
        assert_eq!(recipes.generation(), start + 1);
        let set = recipes.current();
        // Same id as the shipped recipe: the shared one replaces it, and only it is used.
        assert_eq!(set.get("frequence7").unwrap().origin, Origin::Shared);
        assert_eq!(set.len(), BUNDLED.len() + 1);
        assert_eq!(
            set.find(None, "https://live.aurafm.org/Frequence7")
                .unwrap()
                .id,
            "new"
        );
        assert!(recipes.is_known_id("new") && recipes.shared_count() == 2);

        // A local file replaces the shared recipe, and the shared list survives local reloads.
        std::fs::write(dir.join("new.toml"), local_copy(frequence7())).unwrap();
        recipes.reload_if_changed().unwrap();
        let set = recipes.current();
        assert_eq!(set.get("new").unwrap().origin, Origin::Local);
        assert_eq!(set.get("frequence7").unwrap().origin, Origin::Shared);

        // An edited copy of a shared recipe reads back as the same recipe.
        let entry = set.get("frequence7").unwrap().clone();
        let copy = recipes.local_copy_of(&entry).unwrap();
        assert!(
            copy.starts_with(&format!("#:schema {SCHEMA_FILE}\n")),
            "{copy}"
        );
        assert_eq!(Recipe::from_toml(&copy).unwrap(), *entry.recipe);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn local_copies_point_to_the_local_schema_and_drop_fixtures() {
        let copy = local_copy(frequence7());
        assert!(copy.starts_with(&format!("#:schema {SCHEMA_FILE}\n")));
        assert!(!copy.contains("[[test]]") && !copy.contains("fixtures"));
        let recipe = Recipe::from_toml(&copy).unwrap();
        assert!(recipe.tests.is_empty());
        assert_eq!(recipe.name, "Fréquence 7");
    }

    #[test]
    fn templates_match_the_station_and_explain_the_rest() {
        let text = template(&Template {
            name: "Radio \"Test\" Été",
            station_id: Some("11111111-1111-1111-1111-111111111111"),
            stream: "https://Stream.Example.org:443/live.mp3?token=1",
            homepage: Some("https://example.org/"),
        });
        assert!(text.contains(r#"name = "Radio \"Test\" Été""#), "{text}");
        assert!(text.contains(r#"stations = ["11111111-1111-1111-1111-111111111111"], streams = ["stream.example.org/live.mp3"]"#), "{text}");
        assert!(
            text.contains(r#"fetch = { url = "https://example.org/" }"#),
            "{text}"
        );
        // Parses as TOML; invalid as a recipe until the selectors are filled in.
        let error = parse(&text).unwrap_err();
        assert_eq!(
            error, "fill in or remove the empty `extract` field(s): item, artist, title",
            "{text}"
        );
        let filled = text
            .replace("item = \"\"", "item = \"li\"")
            .replace("artist = \"\"", "artist = \".a\"")
            .replace("title = \"\"", "title = \".t\"");
        assert!(parse(&filled).is_ok(), "{:?}", parse(&filled));
    }

    #[test]
    fn share_links_fill_in_the_issue_form() {
        let link = share_link(
            "radio-x",
            "Radio X & co",
            "schema = 1\nname = \"X\"\n",
            Some("<li>a</li>"),
        );
        let url = crate::audio::Url::parse(&link).unwrap();
        assert_eq!(
            url.as_str().split('?').next(),
            Some("https://github.com/getsoundfolk/soundfolk-recipes/issues/new")
        );
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(params["template"], "share.yml");
        assert_eq!(params["title"], "Recipe: Radio X & co");
        assert_eq!(params["recipe-id"], "radio-x");
        assert_eq!(params["recipe"], "schema = 1\nname = \"X\"\n");
        assert_eq!(params["fixture"], "<li>a</li>");
        assert!(!share_link("a", "A", "", None).contains("fixture"));
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(slug("Fréquence 7"), "frequence-7");
        assert_eq!(slug("  RADIO 1 ROCK — ЕДНО  "), "radio-1-rock");
        assert_eq!(slug("ЕДНО"), "station");
    }
}
