// SPDX-License-Identifier: MPL-2.0
//! Recipes: one TOML file per station saying where its current track is published and how to
//! read it. Recipes are data, not code: no scripting, one URL each.
//!
//! ```toml
//! schema = 1
//! name = "Fréquence 7"
//! match = { streams = ["live.aurafm.org/Frequence7"] }
//! fetch = { url = "https://www.frequence7.net/player/" }
//!
//! [extract]
//! type = "html"
//! item = "li.event_0"
//! artist = ".artist"
//! title = ".title"
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracker::Policy;
use crate::SignedDuration;

/// The recipe format version this crate reads. Recipes declaring a newer one are skipped.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Recipe format version (currently 1).
    pub schema: u32,
    /// Station name, for people reading the recipe.
    pub name: String,
    /// Which stations the recipe applies to.
    #[serde(rename = "match")]
    pub matches: Match,
    pub fetch: Fetch,
    pub extract: Extract,
    /// Regular expressions: a title matching one of them (jingle, ad, station ID) is not shown
    /// as a track.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// Fixtures checked by `recipe-check test`; stripped from the published list.
    #[serde(default, rename = "test", skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<Test>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// Radio-Browser station UUIDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<String>,
    /// Stream URLs without the scheme (`host/path`), case-insensitive. A trailing `*` matches
    /// any path starting with what precedes it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    /// The page to read (http or https).
    pub url: String,
    /// How often to read the page when the end of the track is unknown (`"30s"`, `"1m"`).
    /// Only a hint: the engine enforces its own minimum and adapts to the track times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
}

/// How to read the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Extract {
    /// CSS selectors. A field is `selector`, `selector@attribute` or `@attribute` (attribute of
    /// the item itself); otherwise the element's text.
    Html(Fields),
    /// Dotted paths (`now_playing.song.title`, `items.0.title`).
    Json(Fields),
    /// One regular expression over the whole document, with named groups `artist`, `title`,
    /// `text`, `start`, `duration`.
    Text(Pattern),
}

/// Where each value is, relative to `item` (and to each `next` element for upcoming tracks).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    /// The element holding the track on air; the whole document when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `Artist - Title` in one value, when the page does not split them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Start time: Unix timestamp, ISO 8601, or the station's local `HH:MM:SS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Duration: seconds, `MM:SS` or ISO 8601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Cover art URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// Name of the show on air (absolute, not relative to `item`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<String>,
    /// The upcoming track(s), read with the same fields. Several matches = several tracks,
    /// in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub pattern: String,
}

/// A saved page and what the recipe must read from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Test {
    /// Path of the saved page, relative to the recipe file.
    pub page: String,
    /// When the page was saved (RFC 3339), used to resolve local times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Expected start, RFC 3339 (requires `at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Expected title of the next track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_title: Option<String>,
}

/// Why a recipe file was not loaded.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// Written for a newer format: not an error, the app is too old for it.
    NewerSchema(u32),
    Invalid(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerSchema(v) => write!(
                f,
                "written for recipe schema {v}, this app reads {SCHEMA_VERSION}"
            ),
            Self::Invalid(e) => f.write_str(e),
        }
    }
}

impl Recipe {
    /// Parses and validates a TOML recipe.
    pub fn from_toml(text: &str) -> Result<Self, LoadError> {
        // The version is read first: a newer recipe may have fields this version rejects.
        #[derive(Deserialize)]
        struct Version {
            schema: Option<u32>,
        }
        let version: Version =
            toml::from_str(text).map_err(|e| LoadError::Invalid(e.to_string()))?;
        match version.schema {
            Some(v) if v > SCHEMA_VERSION => return Err(LoadError::NewerSchema(v)),
            Some(_) => {}
            None => return Err(LoadError::Invalid("missing `schema`".into())),
        }
        let recipe: Recipe = toml::from_str(text).map_err(|e| LoadError::Invalid(e.to_string()))?;
        let problems = recipe.problems();
        if problems.is_empty() {
            Ok(recipe)
        } else {
            Err(LoadError::Invalid(problems.join("; ")))
        }
    }

    /// Everything wrong with the recipe (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.schema == 0 || self.schema > SCHEMA_VERSION {
            problems.push(format!("unsupported schema {}", self.schema));
        }
        if self.name.trim().is_empty() {
            problems.push("`name` is empty".into());
        }
        if self.matches.stations.is_empty() && self.matches.streams.is_empty() {
            problems.push("`match` needs `stations` or `streams`".into());
        }
        if self.matches.streams.iter().any(|s| s.contains("://")) {
            problems.push("`match.streams` are written without the scheme (`host/path`)".into());
        }
        if !(self.fetch.url.starts_with("https://") || self.fetch.url.starts_with("http://")) {
            problems.push("`fetch.url` must be http or https".into());
        }
        if let Some(every) = &self.fetch.every {
            match parse_every(every) {
                None => problems.push(format!("`fetch.every`: cannot read {every:?}")),
                Some(d) if d < Policy::default().floor => problems.push(format!(
                    "`fetch.every` is below the minimum of {}s",
                    Policy::default().floor.as_secs()
                )),
                Some(_) => {}
            }
        }
        match &self.extract {
            Extract::Html(f) | Extract::Json(f) => {
                let named = [
                    ("item", &f.item),
                    ("artist", &f.artist),
                    ("title", &f.title),
                    ("text", &f.text),
                    ("start", &f.start),
                    ("duration", &f.duration),
                    ("cover", &f.cover),
                    ("show", &f.show),
                    ("next", &f.next),
                ];
                let empty: Vec<&str> = named
                    .iter()
                    .filter(|(_, v)| v.as_deref().is_some_and(|v| v.trim().is_empty()))
                    .map(|(n, _)| *n)
                    .collect();
                if !empty.is_empty() {
                    problems.push(format!(
                        "fill in or remove the empty `extract` field(s): {}",
                        empty.join(", ")
                    ));
                    // Empty selectors would only add confusing errors below.
                    return problems;
                }
                if f.title.is_none() && f.text.is_none() {
                    problems.push("`extract` needs `title` or `text`".into());
                }
            }
            Extract::Text(p) => {
                if !["title", "text"]
                    .iter()
                    .any(|g| p.pattern.contains(&format!("(?P<{g}>")))
                    && !["title", "text"]
                        .iter()
                        .any(|g| p.pattern.contains(&format!("(?<{g}>")))
                {
                    problems.push("`extract.pattern` needs a `title` or `text` group".into());
                }
            }
        }
        if let Err(e) = crate::extract::compile(&self.extract) {
            problems.push(e.to_string());
        }
        for pattern in &self.ignore {
            if let Err(e) = regex_lite::Regex::new(pattern) {
                problems.push(format!("`ignore` {pattern:?}: {e}"));
            }
        }
        problems
    }

    /// Whether the recipe applies to this station (Radio-Browser UUID) or stream URL.
    pub fn matches(&self, station_id: Option<&str>, stream: &str) -> bool {
        if let Some(id) = station_id {
            if self
                .matches
                .stations
                .iter()
                .any(|s| s.eq_ignore_ascii_case(id))
            {
                return true;
            }
        }
        let Some(stream) = normalize_stream(stream) else {
            return false;
        };
        self.matches.streams.iter().any(|pattern| {
            let pattern = pattern.trim().to_ascii_lowercase();
            match pattern.strip_suffix('*') {
                Some(prefix) => stream.starts_with(prefix),
                None => stream == pattern.trim_end_matches('/'),
            }
        })
    }

    /// The polling hint, if the recipe gives one.
    pub fn every(&self) -> Option<SignedDuration> {
        self.fetch.every.as_deref().and_then(parse_every)
    }

    /// Whether a title is noise (jingle, ad…) according to `ignore`.
    pub fn ignores(&self, title: &str) -> bool {
        self.ignore
            .iter()
            .filter_map(|p| regex_lite::Regex::new(p).ok())
            .any(|re| re.is_match(title))
    }

    /// The recipe without its tests, as published in `recipes.json`.
    pub fn published(&self) -> Recipe {
        Recipe {
            tests: Vec::new(),
            ..self.clone()
        }
    }

    /// The recipe as a TOML file (comments are not kept: written from a published list).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("recipe serializes to TOML")
    }
}

/// The form `match.streams` uses for a stream URL (`host[:port]/path`), to write a recipe for it.
pub fn stream_pattern(url: &str) -> Option<String> {
    normalize_stream(url)
}

/// `host[:port]/path`, lowercase, without scheme, query or trailing slash.
fn normalize_stream(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .split_once("://")
        .map_or(url.trim(), |(_, rest)| rest);
    let rest = rest.split(['?', '#']).next()?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next()?; // drop credentials
    let host = host
        .strip_suffix(":80")
        .or_else(|| host.strip_suffix(":443"))
        .unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    Some(
        format!("{host}/{path}")
            .trim_end_matches('/')
            .to_ascii_lowercase(),
    )
}

fn parse_every(text: &str) -> Option<SignedDuration> {
    text.trim()
        .parse::<SignedDuration>()
        .ok()
        .filter(|d| d.is_positive())
}

/// Recipes in priority order: the first one matching a station wins.
#[derive(Debug, Clone, Default)]
pub struct RecipeBook {
    recipes: Vec<Recipe>,
}

impl RecipeBook {
    /// Appends recipes after those already in the book (lower priority).
    pub fn extend(&mut self, recipes: impl IntoIterator<Item = Recipe>) {
        self.recipes.extend(recipes);
    }

    pub fn find(&self, station_id: Option<&str>, stream: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|r| r.matches(station_id, stream))
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

/// JSON Schema of the recipe format (also valid for the TOML files, e.g. with Taplo).
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Recipe)).expect("schema serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQUENCE7: &str = r#"
        schema = 1
        name = "Fréquence 7"
        match = { streams = ["live.aurafm.org/Frequence7"] }
        fetch = { url = "https://www.frequence7.net/player/", every = "30s" }
        ignore = ["^Fréquence 7"]

        [extract]
        type = "html"
        item = "li.event_0"
        artist = ".artist"
        title = ".title"
        start = ".played-at"
        duration = "@data-duration"
        next = "li.event_1"
    "#;

    #[test]
    fn reads_a_recipe() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        assert_eq!(recipe.name, "Fréquence 7");
        assert_eq!(recipe.every(), Some(SignedDuration::from_secs(30)));
        let Extract::Html(fields) = &recipe.extract else {
            panic!("html")
        };
        assert_eq!(fields.duration.as_deref(), Some("@data-duration"));
        assert!(recipe.ignores("Fréquence 7 - le direct"));
        assert!(!recipe.ignores("BLACK RAINBOWS - Children Of Fire"));
    }

    #[test]
    fn matches_streams_and_stations() {
        let mut recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        recipe
            .matches
            .stations
            .push("96202f73-0601-11e8-ae97-52543be04c81".into());
        assert!(recipe.matches(None, "https://live.aurafm.org/Frequence7"));
        assert!(recipe.matches(None, "http://LIVE.aurafm.org:80/frequence7/"));
        assert!(recipe.matches(None, "https://live.aurafm.org/Frequence7?listener=1"));
        assert!(!recipe.matches(None, "https://live.aurafm.org/Frequence7HD"));
        assert!(!recipe.matches(None, "https://example.com/Frequence7"));
        assert!(recipe.matches(
            Some("96202F73-0601-11E8-AE97-52543BE04C81"),
            "https://cdn.example/x"
        ));

        recipe.matches.streams = vec!["stream.example.org/radio/*".into()];
        assert!(recipe.matches(None, "https://stream.example.org/radio/hd.mp3"));
        assert!(!recipe.matches(None, "https://stream.example.org/other"));
    }

    #[test]
    fn rejects_bad_recipes() {
        let bad =
            |from: &str, to: &str| Recipe::from_toml(&FREQUENCE7.replace(from, to)).unwrap_err();
        assert!(matches!(
            bad("schema = 1", "schema = 7"),
            LoadError::NewerSchema(7)
        ));
        assert!(matches!(bad("schema = 1", ""), LoadError::Invalid(_)));
        let LoadError::Invalid(e) = bad("every = \"30s\"", "every = \"2s\"") else {
            panic!()
        };
        assert!(e.contains("minimum"), "{e}");
        let LoadError::Invalid(e) = bad("\"live.aurafm", "\"https://live.aurafm") else {
            panic!()
        };
        assert!(e.contains("scheme"), "{e}");
        let LoadError::Invalid(e) = bad("li.event_0", "li[[") else {
            panic!()
        };
        assert!(e.contains("selector"), "{e}");
        let LoadError::Invalid(e) = bad("type = \"html\"", "type = \"html\"\ncolour = \"red\"")
        else {
            panic!()
        };
        assert!(e.contains("colour"), "{e}");
    }

    #[test]
    fn book_keeps_priority_order() {
        let local = Recipe::from_toml(&FREQUENCE7.replace("Fréquence 7\"", "Local\"")).unwrap();
        let shared = Recipe::from_toml(FREQUENCE7).unwrap();
        let mut book = RecipeBook::default();
        book.extend([local]);
        book.extend([shared]);
        assert_eq!(
            book.find(None, "https://live.aurafm.org/Frequence7")
                .unwrap()
                .name,
            "Local"
        );
    }

    #[test]
    fn schema_describes_the_format() {
        let schema = json_schema().to_string();
        assert!(
            schema.contains("\"streams\"") && schema.contains("\"html\""),
            "{schema}"
        );
    }

    #[test]
    fn toml_export_reads_back() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap().published();
        let text = recipe.to_toml();
        assert_eq!(Recipe::from_toml(&text).unwrap(), recipe, "{text}");
    }
}
