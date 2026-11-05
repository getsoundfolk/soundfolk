// SPDX-License-Identifier: MPL-2.0
//! Now-playing recipes available to the app: the user's own (`<config dir>/recipes/*.toml`) and
//! those shipped with the app (`src-tauri/recipes/stations`). See `docs/07-now-playing.md`.
//!
//! A recipe's **id** is its file name without `.toml` (`frequence7`). A local file replaces the
//! shipped recipe with the same id, whatever its `match` says; other local recipes come before
//! the shipped ones. An invalid local file is reported and the shipped recipe stays in use.
//!
//! Recipes are parsed once and kept in memory; [`Recipes::reload_if_changed`] picks up edits to
//! the local folder. The app calls it when a station starts, before a test reading, and when its
//! window gets the focus back (from the editor): the folder is never polled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use serde::Serialize;
use soundfolk_nowplaying::recipe::{LoadError, Recipe};

include!(concat!(env!("OUT_DIR"), "/bundled_recipes.rs"));

/// Name of the JSON Schema written next to local recipes (editor completion through Taplo).
pub const SCHEMA_FILE: &str = "recipe.schema.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Shipped with the app.
    Bundled,
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
        self.entries.iter().find(|e| e.recipe.matches(station_id, stream))
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

/// All recipes, reloaded when the local folder changes.
pub struct Recipes {
    local_dir: Option<PathBuf>,
    bundled: Vec<Entry>,
    current: RwLock<Arc<RecipeSet>>,
    generation: AtomicU64,
    fingerprint: Mutex<Fingerprint>,
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
                    .map(|recipe| Entry { id, origin: Origin::Bundled, path: None, recipe: Arc::new(recipe) })
            })
            .collect();
        let recipes = Self {
            local_dir,
            bundled,
            current: RwLock::default(),
            generation: AtomicU64::new(0),
            fingerprint: Mutex::default(),
        };
        recipes.reload_if_changed();
        recipes
    }

    pub fn local_dir(&self) -> Option<&Path> {
        self.local_dir.as_deref()
    }

    /// The recipes in use now.
    pub fn current(&self) -> Arc<RecipeSet> {
        self.current.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Incremented every time the recipes in use change.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The shipped TOML of a recipe, to copy it into the local folder.
    pub fn bundled_text(&self, id: &str) -> Option<&'static str> {
        BUNDLED.iter().find(|(file, _)| file.trim_end_matches(".toml") == id).map(|(_, text)| *text)
    }

    pub fn is_bundled_id(&self, id: &str) -> bool {
        self.bundled.iter().any(|e| e.id == id)
    }

    /// Reloads the local folder if a file appeared, changed or disappeared. Returns the files that
    /// appeared or changed (`None`: nothing changed).
    pub fn reload_if_changed(&self) -> Option<Vec<Change>> {
        let files = self.local_dir.as_deref().map(list_local).unwrap_or_default();
        let fingerprint: Fingerprint = files
            .iter()
            .map(|path| {
                let meta = std::fs::metadata(path).ok();
                (path.clone(), (meta.as_ref().and_then(|m| m.modified().ok()), meta.map_or(0, |m| m.len())))
            })
            .collect();
        let mut previous = self.fingerprint.lock().unwrap_or_else(|p| p.into_inner());
        let first = self.generation() == 0;
        if !first && *previous == fingerprint {
            return None;
        }
        let changed: Vec<&PathBuf> =
            files.iter().filter(|p| !first && previous.get(*p) != fingerprint.get(*p)).collect();

        let mut local = Vec::new();
        let mut errors = Vec::new();
        for path in &files {
            let id = file_id(path);
            let result = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| parse(&text));
            match result {
                Ok(recipe) => local.push(Entry {
                    id,
                    origin: Origin::Local,
                    path: Some(path.clone()),
                    recipe: Arc::new(recipe),
                }),
                Err(error) => {
                    tracing::warn!("recipe {}: {error}", path.display());
                    errors.push(FileError { id, path: path.clone(), error });
                }
            }
        }
        let changes = changed
            .into_iter()
            .map(|path| {
                let id = file_id(path);
                let error = errors.iter().find(|e| e.path == *path).map(|e| e.error.clone());
                Change { id, path: path.clone(), error }
            })
            .collect();

        // A valid local file replaces the shipped recipe with the same id.
        let replaced: Vec<&str> = local.iter().map(|e| e.id.as_str()).collect();
        let bundled = self.bundled.iter().filter(|e| !replaced.contains(&e.id.as_str())).cloned();
        let set = RecipeSet { local: files.len(), entries: local.iter().cloned().chain(bundled).collect(), errors };

        *self.current.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(set);
        *previous = fingerprint;
        self.generation.fetch_add(1, Ordering::SeqCst);
        Some(changes)
    }
}

fn list_local(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file())
        .collect();
    paths.sort();
    paths
}

fn file_id(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
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
    let folded: String = name.nfkd().filter(char::is_ascii).collect::<String>().to_ascii_lowercase();
    let words: Vec<&str> = folded.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let slug = words.join("-");
    if slug.is_empty() {
        "station".into()
    } else {
        slug.chars().take(48).collect::<String>().trim_end_matches('-').to_owned()
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
# Format: https://github.com/getsoundfolk/soundfolk/blob/main/docs/07-now-playing.md
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundfolk-recipes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frequence7() -> &'static str {
        BUNDLED.iter().find(|(n, _)| *n == "frequence7.toml").unwrap().1
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
                let failures = soundfolk_nowplaying::check::run_test(&recipe, test, &page);
                assert!(failures.is_empty(), "{name} ({}): {failures:?}", test.page);
            }
        }
    }

    #[test]
    fn published_schema_is_up_to_date() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/recipe.schema.json");
        let published: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            published,
            soundfolk_nowplaying::recipe::json_schema(),
            "regenerate: cargo run -p soundfolk-nowplaying --bin recipe-check -- schema > recipes/recipe.schema.json"
        );
    }

    #[test]
    fn local_recipes_come_first_and_replace_shipped_ones_by_id() {
        let dir = temp_dir("order");
        std::fs::write(dir.join("mine.toml"), frequence7().replace("name = \"Fréquence 7\"", "name = \"Mine\"")).unwrap();
        std::fs::write(dir.join("broken.toml"), "schema = 1\nname = ").unwrap();
        let recipes = Recipes::new(Some(dir.clone()));
        let set = recipes.current();
        assert_eq!(set.find(None, "https://live.aurafm.org/Frequence7").unwrap().id, "mine");
        assert_eq!((set.len(), set.local_count()), (2, 2));
        assert_eq!(set.errors()[0].id, "broken");

        // Same id as the shipped recipe: replaces it even when it matches another stream.
        let moved = local_copy(frequence7()).replace("live.aurafm.org/Frequence7", "example.org/other");
        std::fs::write(dir.join("frequence7.toml"), moved).unwrap();
        recipes.reload_if_changed().unwrap();
        let set = recipes.current();
        assert_eq!(set.get("frequence7").unwrap().origin, Origin::Local);
        assert_eq!(set.len(), 2);
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
        assert_eq!(recipes.current().get("frequence7").unwrap().origin, Origin::Local);

        std::fs::remove_file(dir.join("frequence7.toml")).unwrap();
        assert_eq!(recipes.reload_if_changed(), Some(Vec::new()));
        assert_eq!(recipes.current().get("frequence7").unwrap().origin, Origin::Bundled);
        assert_eq!(recipes.generation(), start + 3);
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
        assert!(text.contains(r#"fetch = { url = "https://example.org/" }"#), "{text}");
        // Parses as TOML; invalid as a recipe until the selectors are filled in.
        let error = parse(&text).unwrap_err();
        assert_eq!(error, "fill in or remove the empty `extract` field(s): item, artist, title", "{text}");
        let filled = text.replace("item = \"\"", "item = \"li\"").replace("artist = \"\"", "artist = \".a\"").replace("title = \"\"", "title = \".t\"");
        assert!(parse(&filled).is_ok(), "{:?}", parse(&filled));
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(slug("Fréquence 7"), "frequence-7");
        assert_eq!(slug("  RADIO 1 ROCK — ЕДНО  "), "radio-1-rock");
        assert_eq!(slug("ЕДНО"), "station");
    }
}
