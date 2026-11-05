// SPDX-License-Identifier: MPL-2.0
//! Turning a page just read into what the shared recipes repository needs: a **fixture** (the
//! excerpt of the page the recipe reads) and the recipe file with a `[[test]]` that replays it.
//! Used by `recipe-check fixture` and by the app's *Share this recipe*.
//!
//! The excerpt keeps what the `[[test]]` checks: the track on air, the **first** upcoming track and
//! the show, with the chain of their ancestors and their attributes, so that the selectors still
//! match. Nothing else: the app reads the live page, the fixture only serves the tests. Tests run
//! [`oversized`] so that every fixture of the repository follows this rule. It
//! must read exactly as the whole page did: otherwise a wider excerpt is tried (whole parents),
//! and failing that, an error says to save the page by hand.

use std::collections::HashSet;

use ego_tree::NodeId;
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use serde_json::Value;

use crate::check;
use crate::recipe::{Extract, Fields, Recipe, Test};
use crate::{extract, Snapshot, Timestamp};

/// Largest fixture the repository accepts.
pub const MAX_BYTES: usize = 64 * 1024;
/// Upcoming tracks kept in an excerpt: the one `next_title` checks.
const NEXT_KEPT: usize = 1;
/// Slack over the excerpt before a fixture counts as oversized (formatting, header comment).
const OVERSIZE_SLACK: usize = 1024;

/// What to add to the repository.
#[derive(Debug, Clone, PartialEq)]
pub struct Shared {
    /// `stations/<id>.toml`: the recipe, with `#:schema ../recipe.schema.json` and one `[[test]]`.
    pub recipe: String,
    /// `fixtures/<fixture_file>`.
    pub fixture: String,
    /// `<id>.html`, `<id>.json` or `<id>.txt`.
    pub fixture_file: String,
}

/// Builds the fixture and the test of a recipe from a page read at `at` (its HTTP `Date`).
/// `recipe_text` is the recipe file as written (comments are kept, earlier tests are replaced).
pub fn prepare(id: &str, recipe_text: &str, recipe: &Recipe, url: &str, page: &str, at: Timestamp) -> Result<Shared, String> {
    let fixture_file = format!("{id}.{}", extension(&recipe.extract));
    let fixture = excerpt(recipe, url, page, at)?;
    let reading = check::read(recipe, &fixture, at)?;
    let on_air = reading.on_air.ok_or("nothing is on air on the page: try again while a track is playing")?;
    let test = Test {
        page: format!("../fixtures/{fixture_file}"),
        at: Some(at.to_string()),
        artist: on_air.current.artist.clone(),
        title: Some(on_air.current.title.clone()),
        started_at: on_air.current.started_at.map(|t| t.to_string()),
        next_title: on_air.next.as_ref().map(|t| t.title.clone()),
    };
    let failures = check::run_test(recipe, &test, &fixture);
    if !failures.is_empty() {
        return Err(format!("the test written for the excerpt fails: {}", failures.join("; ")));
    }
    Ok(Shared { recipe: with_test(recipe_text, &test), fixture, fixture_file })
}

/// The fixture's file extension.
pub fn extension(extract: &Extract) -> &'static str {
    match extract {
        Extract::Html(_) => "html",
        Extract::Json(_) => "json",
        Extract::Text(_) => "txt",
    }
}

/// The recipe file for the repository: schema line pointing to the repository's schema, earlier
/// tests removed, `test` appended.
pub fn with_test(recipe_text: &str, test: &Test) -> String {
    #[derive(Serialize)]
    struct Tests<'a> {
        test: [&'a Test; 1],
    }
    let mut out = String::from("#:schema ../recipe.schema.json\n");
    let mut in_test = false;
    for line in recipe_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#:schema") {
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
    let block = toml::to_string(&Tests { test: [test] }).expect("test serializes");
    out.trim_end().to_owned() + "\n\n" + block.trim_end() + "\n"
}

/// The part of `page` the recipe reads, checked to read the same as the whole page.
pub fn excerpt(recipe: &Recipe, url: &str, page: &str, at: Timestamp) -> Result<String, String> {
    let whole = extract::read(recipe, page).map_err(|e| e.to_string())?;
    if whole.current.is_none() {
        return Err("nothing is on air on the page: try again while a track is playing".into());
    }
    let candidates = match &recipe.extract {
        Extract::Html(fields) => html_excerpts(fields, page, url, at)?,
        Extract::Json(fields) => json_excerpts(fields, page)?,
        Extract::Text(pattern) => text_excerpt(&pattern.pattern, page).into_iter().collect(),
    };
    let mut too_large = None;
    for candidate in candidates {
        let reads_the_same = extract::read(recipe, &candidate).is_ok_and(|s| same(&s, &whole));
        if !reads_the_same {
            continue;
        }
        if candidate.len() > MAX_BYTES {
            too_large = Some(candidate.len());
            continue;
        }
        return Ok(candidate);
    }
    Err(match too_large {
        Some(n) => format!("the excerpt is {} KiB, over the {} KiB limit", n / 1024, MAX_BYTES / 1024),
        None => "the page could not be cut down to what the recipe reads: save the useful part by hand".into(),
    })
}

/// Why a saved fixture holds much more than what the recipe reads (`None`: it doesn't). It is
/// compared with its own excerpt: more than twice its size plus 1 KiB is too much.
pub fn oversized(recipe: &Recipe, page: &str) -> Option<String> {
    let excerpt = excerpt(recipe, "", page, Timestamp::UNIX_EPOCH).ok()?;
    let limit = excerpt.len() * 2 + OVERSIZE_SLACK;
    (page.len() > limit).then(|| {
        format!(
            "the fixture is {} bytes, its excerpt {}: keep only the track on air, the next one and the show \
             (recipe-check fixture <recipe.toml> --page <fixture> --at <its `at`>)",
            page.len(),
            excerpt.len()
        )
    })
}

/// The current track, the upcoming tracks kept, and the show read the same.
fn same(excerpt: &Snapshot, whole: &Snapshot) -> bool {
    let kept = whole.next.len().min(NEXT_KEPT);
    excerpt.current == whole.current
        && excerpt.show == whole.show
        && excerpt.next.len() >= kept
        && excerpt.next[..kept] == whole.next[..kept]
}

// ── HTML ────────────────────────────────────────────────────────────────────

/// The selector part of a field (`sel`, `sel@attr` or `@attr`).
fn field_selector(spec: &str) -> Option<Selector> {
    let spec = spec.trim();
    let selector = match spec.rsplit_once('@') {
        Some((sel, attr)) if !attr.is_empty() && attr.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')) => sel.trim(),
        _ => spec,
    };
    (!selector.is_empty()).then(|| Selector::parse(selector).ok()).flatten()
}

/// The minimal excerpt, then one keeping the whole parents of what is read.
fn html_excerpts(fields: &Fields, page: &str, url: &str, at: Timestamp) -> Result<Vec<String>, String> {
    let document = Html::parse_document(page);
    let root = document.root_element();
    let selector = |spec: &Option<String>| spec.as_deref().map(|s| Selector::parse(s).map_err(|e| format!("CSS selector {s:?}: {e}"))).transpose();

    let mut kept: Vec<ElementRef> = Vec::new();
    match selector(&fields.item)? {
        Some(item) => kept.extend(root.select(&item).next()),
        // No item: the fields are read from the whole document.
        None => {
            for spec in [&fields.artist, &fields.title, &fields.text, &fields.start, &fields.duration, &fields.cover] {
                if let Some(sel) = spec.as_deref().and_then(field_selector) {
                    kept.extend(root.select(&sel).next());
                }
            }
        }
    }
    if let Some(next) = selector(&fields.next)? {
        kept.extend(root.select(&next).take(NEXT_KEPT));
    }
    if let Some(sel) = fields.show.as_deref().and_then(field_selector) {
        kept.extend(root.select(&sel).next());
    }
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let header = format!(
        "<!-- Excerpt of {} saved on {at} (what the recipe reads). Third-party content, not covered by the recipes licence. -->\n",
        url.replace("--", "%2D%2D")
    );
    let parents: Vec<ElementRef> =
        kept.iter().map(|el| el.parent().and_then(ElementRef::wrap).unwrap_or(*el)).collect();
    Ok([kept, parents].iter().map(|keep| header.clone() + &write_kept(root, keep)).collect())
}

/// `root` with only the `keep` elements (whole) and their ancestors (tags and attributes only).
fn write_kept(root: ElementRef, keep: &[ElementRef]) -> String {
    let keep_ids: HashSet<_> = keep.iter().map(|el| el.id()).collect();
    let path_ids: HashSet<_> =
        keep.iter().flat_map(|el| el.ancestors().map(|a| a.id())).collect();
    let mut out = String::new();
    write_element(root, &keep_ids, &path_ids, 0, &mut out);
    out
}

fn write_element(el: ElementRef, keep: &HashSet<NodeId>, path: &HashSet<NodeId>, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    if keep.contains(&el.id()) {
        out.push_str(&indent);
        out.push_str(&el.html());
        out.push('\n');
        return;
    }
    if !path.contains(&el.id()) {
        return;
    }
    out.push_str(&format!("{indent}<{}", el.value().name()));
    for (name, value) in el.value().attrs() {
        out.push_str(&format!(" {name}=\"{}\"", value.replace('&', "&amp;").replace('"', "&quot;")));
    }
    out.push_str(">\n");
    for child in el.children().filter_map(ElementRef::wrap) {
        write_element(child, keep, path, depth + 1, out);
    }
    out.push_str(&format!("{indent}</{}>\n", el.value().name()));
}

// ── JSON ────────────────────────────────────────────────────────────────────

/// The paths read copied into an otherwise empty document, then the whole document.
fn json_excerpts(fields: &Fields, page: &str) -> Result<Vec<String>, String> {
    let document: Value = serde_json::from_str(page).map_err(|e| e.to_string())?;
    let mut out = Value::Null;
    let mut paths: Vec<&str> = Vec::new();
    match fields.item.as_deref() {
        Some(item) => paths.push(item),
        None => paths.extend(
            [&fields.artist, &fields.title, &fields.text, &fields.start, &fields.duration, &fields.cover]
                .into_iter()
                .filter_map(|p| p.as_deref()),
        ),
    }
    paths.extend(fields.show.as_deref());
    for path in &paths {
        graft(&mut out, &document, &segments(path));
    }
    if let Some(next) = fields.next.as_deref() {
        let segs = segments(next);
        graft(&mut out, &document, &segs);
        if let Some(Value::Array(items)) = lookup_mut(&mut out, &segs) {
            items.truncate(NEXT_KEPT);
        }
    }
    let pretty = |v: &Value| serde_json::to_string_pretty(v).expect("JSON serializes") + "\n";
    Ok(vec![pretty(&out), pretty(&document)])
}

fn segments(path: &str) -> Vec<&str> {
    path.split('.').filter(|s| !s.is_empty()).collect()
}

/// Copies `src` at `path` into `out`, creating the objects and arrays on the way (array slots
/// before the index are `null`, so indexes stay the same).
fn graft(out: &mut Value, src: &Value, path: &[&str]) {
    let Some((key, rest)) = path.split_first() else {
        *out = src.clone();
        return;
    };
    match src {
        Value::Object(map) => {
            let Some(child) = map.get(*key) else { return };
            if !out.is_object() {
                *out = Value::Object(Default::default());
            }
            let slot = out.as_object_mut().expect("object").entry(key.to_string()).or_insert(Value::Null);
            graft(slot, child, rest);
        }
        Value::Array(items) => {
            let Some((index, child)) = key.parse::<usize>().ok().and_then(|i| items.get(i).map(|c| (i, c))) else { return };
            if !out.is_array() {
                *out = Value::Array(Vec::new());
            }
            let array = out.as_array_mut().expect("array");
            if array.len() <= index {
                array.resize(index + 1, Value::Null);
            }
            graft(&mut array[index], child, rest);
        }
        _ => {}
    }
}

fn lookup_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    path.iter().try_fold(value, |v, key| match v {
        Value::Object(map) => map.get_mut(*key),
        Value::Array(items) => items.get_mut(key.parse::<usize>().ok()?),
        _ => None,
    })
}

// ── Text ────────────────────────────────────────────────────────────────────

fn text_excerpt(pattern: &str, page: &str) -> Option<String> {
    let re = regex_lite::Regex::new(pattern).ok()?;
    Some(re.find(page)?.as_str().to_owned() + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQUENCE7: &str = include_str!("../../../recipes/stations/frequence7.toml");
    const PAGE: &str = include_str!("../testdata/frequence7-player.html");
    const AT: &str = "2026-11-04T21:39:08Z";

    fn wrapped(body: &str) -> String {
        format!(
            "<!DOCTYPE html><html lang=\"fr\"><head><title>Player</title><script>track()</script></head>\
             <body class=\"player\"><nav><a href=\"/\">Home</a></nav><main><section id=\"playlist\">{body}</section>\
             <aside><p>Comments: jean@example.org</p></aside></main></body></html>"
        )
    }

    #[test]
    fn html_excerpts_keep_only_what_is_read_and_read_the_same() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        let page = wrapped(PAGE);
        let at: Timestamp = AT.parse().unwrap();
        let shared = prepare("frequence7", FREQUENCE7, &recipe, "https://www.frequence7.net/player/", &page, at).unwrap();
        assert_eq!(shared.fixture_file, "frequence7.html");
        let fixture = &shared.fixture;
        assert!(fixture.starts_with("<!-- Excerpt of https://www.frequence7.net/player/ saved on 2026-11-04T21:39:08Z"), "{fixture}");
        for gone in ["<script", "Comments", "<nav", "event_-1 ", "event_2", "event_3"] {
            assert!(!fixture.contains(gone), "{gone} kept:\n{fixture}");
        }
        for kept in ["<section id=\"playlist\">", "event_0", "event_1", "BLACK RAINBOWS", "Nino"] {
            assert!(fixture.contains(kept), "{kept} missing:\n{fixture}");
        }
        assert!(fixture.len() < PAGE.len() / 3, "{} bytes", fixture.len());

        // The recipe file: comments kept, the old test replaced by one that passes.
        let text = &shared.recipe;
        assert!(text.starts_with("#:schema ../recipe.schema.json\n# Fréquence 7"), "{text}");
        assert_eq!(text.matches("[[test]]").count(), 1, "{text}");
        let written = Recipe::from_toml(text).unwrap();
        let test = &written.tests[0];
        assert_eq!(test.page, "../fixtures/frequence7.html");
        assert_eq!(test.title.as_deref(), Some("Children Of Fire And Sacrifices"));
        assert_eq!(test.started_at.as_deref(), Some("2026-11-04T21:36:51Z"));
        assert_eq!(test.next_title.as_deref(), Some("Nino"));
        assert!(check::run_test(&written, test, fixture).is_empty());
    }

    #[test]
    fn positional_selectors_fall_back_to_whole_parents() {
        let text = FREQUENCE7.replace("item = \"li.event_0\"", "item = \"li:nth-child(16)\"");
        let recipe = Recipe::from_toml(&text).unwrap();
        let at: Timestamp = AT.parse().unwrap();
        let fixture = excerpt(&recipe, "https://example.org/", &wrapped(PAGE), at).unwrap();
        assert!(fixture.contains("event_-15"), "the whole list is kept:\n{fixture}");
        assert!(!fixture.contains("<script"));
    }

    #[test]
    fn json_excerpts_keep_the_paths_read() {
        let recipe = Recipe::from_toml(
            r#"
            schema = 1
            name = "JSON"
            match = { streams = ["example.org/live"] }
            fetch = { url = "https://example.org/api" }
            [extract]
            type = "json"
            item = "data.items.1"
            artist = "artist"
            title = "title"
            next = "data.upcoming"
            "#,
        )
        .unwrap();
        let page = r#"{"data":{"station":{"listeners":12,"token":"secret"},
            "items":[{"artist":"A","title":"Old"},{"artist":"B","title":"Now"}],
            "upcoming":[{"artist":"C","title":"N1"},{"artist":"D","title":"N2"},{"artist":"E","title":"N3"},{"artist":"F","title":"N4"}]}}"#;
        let fixture = excerpt(&recipe, "https://example.org/api", page, AT.parse().unwrap()).unwrap();
        let value: Value = serde_json::from_str(&fixture).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"data": {
                "items": [null, {"artist": "B", "title": "Now"}],
                "upcoming": [{"artist":"C","title":"N1"}]
            }})
        );
    }

    #[test]
    fn text_excerpts_keep_the_match() {
        let recipe = Recipe::from_toml(
            r#"
            schema = 1
            name = "Text"
            match = { streams = ["example.org/live"] }
            fetch = { url = "https://example.org/now.txt" }
            [extract]
            type = "text"
            pattern = 'NOW: (?P<artist>[^-]+) - (?P<title>.+)'
            "#,
        )
        .unwrap();
        let fixture = excerpt(&recipe, "u", "header\nNOW: A - B\nfooter\n", AT.parse().unwrap()).unwrap();
        assert_eq!(fixture, "NOW: A - B\n");
    }

    #[test]
    fn whole_pages_are_oversized_and_excerpts_are_not() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        let page = wrapped(PAGE);
        let error = oversized(&recipe, &page).unwrap();
        assert!(error.contains("recipe-check fixture"), "{error}");
        let fixture = excerpt(&recipe, "https://www.frequence7.net/player/", &page, AT.parse().unwrap()).unwrap();
        assert_eq!(oversized(&recipe, &fixture), None);
    }

    #[test]
    fn nothing_on_air_is_an_error() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        let error = excerpt(&recipe, "u", "<html><body></body></html>", AT.parse().unwrap()).unwrap_err();
        assert!(error.contains("nothing is on air"), "{error}");
    }
}
