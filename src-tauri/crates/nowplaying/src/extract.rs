// SPDX-License-Identifier: MPL-2.0
//! Reading a page as a recipe says.

use regex_lite::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

use crate::recipe::{Extract, Fields, Recipe};
use crate::value::{
    clean_text, duration_from_secs, parse_duration, parse_start, start_from_number,
};
use crate::{clock, ExtractError, RawTrack, Snapshot, Start};
use jiff::SignedDuration;

/// Reads the page with the recipe, and drops the titles the recipe ignores.
pub fn read(recipe: &Recipe, page: &str) -> Result<Snapshot, ExtractError> {
    let mut snapshot = extract(&recipe.extract, page)?;
    let ignored = |t: &RawTrack| {
        t.title.as_deref().is_some_and(|title| {
            let shown = match &t.artist {
                Some(artist) => format!("{artist} - {title}"),
                None => title.to_owned(),
            };
            recipe.ignores(&shown) || recipe.ignores(title)
        })
    };
    if snapshot.current.as_ref().is_some_and(ignored) {
        snapshot.current = None;
    }
    // An ignored next track (a jingle, a time signal) still says when the current one ends.
    if let (Some(current), Some(first)) = (&mut snapshot.current, snapshot.next.first())
        && current.duration.is_none()
        && ignored(first)
    {
        current.duration = gap(current.start, first.start);
    }
    snapshot.next.retain(|t| !ignored(t));
    Ok(snapshot)
}

/// The time from one start to the next, when both are of the same kind.
fn gap(from: Option<Start>, to: Option<Start>) -> Option<SignedDuration> {
    let gap = match (from?, to?) {
        (Start::At(a), Start::At(b)) => b.duration_since(a),
        (Start::Local(a), Start::Local(b)) => clock::forward_gap(a, b),
        _ => return None,
    };
    gap.is_positive().then_some(gap)
}

/// Reads the page as `extract` says.
pub fn extract(extract: &Extract, page: &str) -> Result<Snapshot, ExtractError> {
    match compile(extract)? {
        Compiled::Html(fields) => Ok(read_html(&fields, page)),
        Compiled::Json(fields) => read_json(fields, page),
        Compiled::Text(re) => Ok(read_text(&re, page)),
    }
}

/// Selectors and patterns, checked once.
pub(crate) enum Compiled<'a> {
    Html(HtmlFields),
    Json(&'a Fields),
    Text(Regex),
}

pub(crate) fn compile(extract: &Extract) -> Result<Compiled<'_>, ExtractError> {
    Ok(match extract {
        Extract::Html(fields) => Compiled::Html(HtmlFields::new(fields)?),
        Extract::Json(fields) => Compiled::Json(fields),
        Extract::Text(p) => Compiled::Text(
            Regex::new(&p.pattern).map_err(|e| ExtractError::Recipe(format!("pattern: {e}")))?,
        ),
    })
}

// ── HTML ────────────────────────────────────────────────────────────────────

/// `selector`, `selector@attribute` or `@attribute`.
pub(crate) struct HtmlField {
    selector: Option<Selector>,
    attribute: Option<String>,
}

impl HtmlField {
    fn parse(spec: &str) -> Result<Self, ExtractError> {
        let spec = spec.trim();
        let (selector, attribute) = match spec.rsplit_once('@') {
            Some((sel, attr))
                if !attr.is_empty()
                    && attr
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')) =>
            {
                (sel.trim(), Some(attr.to_owned()))
            }
            _ => (spec, None),
        };
        let selector = if selector.is_empty() {
            None
        } else {
            Some(selector_of(selector)?)
        };
        Ok(Self {
            selector,
            attribute,
        })
    }

    /// First value found under `scope` (the scope itself when there is no selector).
    fn value(&self, scope: ElementRef<'_>) -> Option<String> {
        let element = match &self.selector {
            Some(selector) => scope.select(selector).next()?,
            None => scope,
        };
        match &self.attribute {
            Some(name) => element.value().attr(name).and_then(clean_text),
            None => clean_text(&element.text().collect::<String>()),
        }
    }
}

fn selector_of(text: &str) -> Result<Selector, ExtractError> {
    Selector::parse(text).map_err(|e| ExtractError::Recipe(format!("CSS selector {text:?}: {e}")))
}

pub(crate) struct HtmlFields {
    item: Option<Selector>,
    artist: Option<HtmlField>,
    title: Option<HtmlField>,
    text: Option<HtmlField>,
    start: Option<HtmlField>,
    duration: Option<HtmlField>,
    cover: Option<HtmlField>,
    show: Option<HtmlField>,
    next: Option<Selector>,
}

impl HtmlFields {
    fn new(f: &Fields) -> Result<Self, ExtractError> {
        let field = |spec: &Option<String>| spec.as_deref().map(HtmlField::parse).transpose();
        let selector = |spec: &Option<String>| spec.as_deref().map(selector_of).transpose();
        Ok(Self {
            item: selector(&f.item)?,
            artist: field(&f.artist)?,
            title: field(&f.title)?,
            text: field(&f.text)?,
            start: field(&f.start)?,
            duration: field(&f.duration)?,
            cover: field(&f.cover)?,
            show: field(&f.show)?,
            next: selector(&f.next)?,
        })
    }

    fn track(&self, scope: ElementRef<'_>) -> Option<RawTrack> {
        let get = |field: &Option<HtmlField>| field.as_ref().and_then(|f| f.value(scope));
        let track = raw_track(
            get(&self.artist),
            get(&self.title),
            get(&self.text),
            get(&self.start).and_then(|s| parse_start(&s)),
            get(&self.duration).and_then(|d| parse_duration(&d)),
            get(&self.cover),
        );
        (!track.is_empty()).then_some(track)
    }
}

fn read_html(fields: &HtmlFields, page: &str) -> Snapshot {
    let document = Html::parse_document(page);
    let root = document.root_element();
    let current = match &fields.item {
        Some(item) => root.select(item).next().and_then(|el| fields.track(el)),
        None => fields.track(root),
    };
    let next = fields
        .next
        .as_ref()
        .map(|sel| root.select(sel).filter_map(|el| fields.track(el)).collect())
        .unwrap_or_default();
    let show = fields.show.as_ref().and_then(|f| f.value(root));
    Snapshot {
        current,
        next,
        show,
    }
}

/// A track from its fields; `text` (`Artist - Title` in one value) fills in a missing title.
fn raw_track(
    artist: Option<String>,
    title: Option<String>,
    text: Option<String>,
    start: Option<crate::Start>,
    duration: Option<crate::SignedDuration>,
    cover: Option<String>,
) -> RawTrack {
    let (artist, title) = match (artist, title) {
        (artist, Some(title)) => (artist, Some(title)),
        // Artist alone: still a track, e.g. a page listing only the performer.
        (Some(artist), None) if text.is_none() => (None, Some(artist)),
        (_, None) => (None, text),
    };
    RawTrack {
        artist,
        title,
        start,
        duration,
        cover,
    }
}

// ── JSON ────────────────────────────────────────────────────────────────────

fn read_json(fields: &Fields, page: &str) -> Result<Snapshot, ExtractError> {
    let document: Value =
        serde_json::from_str(page).map_err(|e| ExtractError::Json(e.to_string()))?;
    let scope = match &fields.item {
        Some(path) => lookup(&document, path),
        None => Some(&document),
    };
    let current = scope.and_then(|item| json_track(fields, item));
    let next = match fields.next.as_deref().and_then(|p| lookup(&document, p)) {
        Some(Value::Array(items)) => items.iter().filter_map(|i| json_track(fields, i)).collect(),
        Some(item) => json_track(fields, item).into_iter().collect(),
        None => Vec::new(),
    };
    let show = fields
        .show
        .as_deref()
        .and_then(|p| lookup(&document, p))
        .and_then(json_text);
    Ok(Snapshot {
        current,
        next,
        show,
    })
}

fn json_track(fields: &Fields, item: &Value) -> Option<RawTrack> {
    let get = |path: &Option<String>| path.as_deref().and_then(|p| lookup(item, p));
    let text = |path: &Option<String>| get(path).and_then(json_text);
    let start = get(&fields.start).and_then(|v| match v {
        Value::Number(n) => n.as_f64().and_then(start_from_number),
        Value::String(s) => parse_start(s),
        _ => None,
    });
    let duration = get(&fields.duration).and_then(|v| match v {
        Value::Number(n) => n.as_f64().and_then(duration_from_secs),
        Value::String(s) => parse_duration(s),
        _ => None,
    });
    let track = raw_track(
        text(&fields.artist),
        text(&fields.title),
        text(&fields.text),
        start,
        duration,
        text(&fields.cover),
    );
    (!track.is_empty()).then_some(track)
}

/// `a.b.0.c`: object keys and array indexes.
fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|s| !s.is_empty())
        .try_fold(value, |v, key| match v {
            Value::Object(map) => map.get(key),
            Value::Array(items) => items.get(key.parse::<usize>().ok()?),
            _ => None,
        })
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => clean_text(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

// ── Text ────────────────────────────────────────────────────────────────────

fn read_text(re: &Regex, page: &str) -> Snapshot {
    let Some(caps) = re.captures(page) else {
        return Snapshot::default();
    };
    let group = |name: &str| caps.name(name).and_then(|m| clean_text(m.as_str()));
    let track = raw_track(
        group("artist"),
        group("title"),
        group("text"),
        group("start").and_then(|s| parse_start(&s)),
        group("duration").and_then(|d| parse_duration(&d)),
        None,
    );
    Snapshot {
        current: (!track.is_empty()).then_some(track),
        ..Snapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Pattern;
    use crate::{SignedDuration, Start, Time};

    const PLAYLIST: &str = r#"<ul class="current-playlist">
        <li class="event_-1 track" data-duration="229">
            <span class="played-at">22:32:40</span>
            <span class="artist">AFRO CUBAN ALL STARS</span>
            <span class="title">Elubé chango</span>
        </li>
        <li class="event_0 track" data-duration="276">
            <span class="played-at">22:36:51</span>
            <span class="artist">MELLOW MOOD / ANTHONY B</span>
            <span class="title">Home or Abroad (feat Anthony B &amp; Dub Inc)</span>
        </li>
        <li class="event_1 track" data-duration="103">
            <span class="played-at">22:41:27</span>
            <span class="artist">THE LINDA LINDAS</span>
            <span class="title">Nino</span>
        </li></ul>"#;

    fn html_fields() -> Fields {
        Fields {
            item: Some("li.event_0".into()),
            artist: Some(".artist".into()),
            title: Some(".title".into()),
            start: Some(".played-at".into()),
            duration: Some("@data-duration".into()),
            next: Some("li.event_1, li.event_2".into()),
            ..Fields::default()
        }
    }

    #[test]
    fn html_reads_current_and_next() {
        let snapshot = extract(&Extract::Html(html_fields()), PLAYLIST).unwrap();
        let current = snapshot.current.unwrap();
        assert_eq!(current.artist.as_deref(), Some("MELLOW MOOD / ANTHONY B"));
        assert_eq!(
            current.title.as_deref(),
            Some("Home or Abroad (feat Anthony B & Dub Inc)")
        );
        assert_eq!(
            current.start,
            Some(Start::Local(Time::constant(22, 36, 51, 0)))
        );
        assert_eq!(current.duration, Some(SignedDuration::from_secs(276)));
        assert_eq!(snapshot.next.len(), 1);
        assert_eq!(snapshot.next[0].title.as_deref(), Some("Nino"));
    }

    #[test]
    fn html_missing_item_gives_nothing() {
        let snapshot = extract(&Extract::Html(html_fields()), "<ul></ul>").unwrap();
        assert_eq!(snapshot, Snapshot::default());
    }

    #[test]
    fn html_text_field_and_attribute_on_child() {
        let fields = Fields {
            text: Some("#now".into()),
            cover: Some("img.cover@src".into()),
            ..Fields::default()
        };
        let page = r#"<p id="now"> Air -  Sexy Boy </p><img class="cover" src="https://x/c.jpg">"#;
        let current = extract(&Extract::Html(fields), page)
            .unwrap()
            .current
            .unwrap();
        assert_eq!(
            (current.artist, current.title.as_deref()),
            (None, Some("Air - Sexy Boy"))
        );
        assert_eq!(current.cover.as_deref(), Some("https://x/c.jpg"));
    }

    #[test]
    fn json_reads_paths_numbers_and_arrays() {
        let fields = Fields {
            item: Some("now_playing.song".into()),
            artist: Some("artist".into()),
            title: Some("title".into()),
            start: Some("played_at".into()),
            duration: Some("duration".into()),
            next: Some("upcoming".into()),
            show: Some("show.name".into()),
            ..Fields::default()
        };
        let page = r#"{"now_playing": {"song": {"artist": "Air", "title": "Sexy Boy",
            "played_at": 1793828211, "duration": 298}},
            "upcoming": [{"artist": "Moby", "title": "Porcelain"}, {"title": 1999}],
            "show": {"name": "Morning"}}"#;
        let snapshot = extract(&Extract::Json(fields), page).unwrap();
        let current = snapshot.current.unwrap();
        assert_eq!(current.title.as_deref(), Some("Sexy Boy"));
        assert_eq!(
            current.start,
            Some(Start::At(
                crate::Timestamp::from_second(1_793_828_211).unwrap()
            ))
        );
        assert_eq!(current.duration, Some(SignedDuration::from_secs(298)));
        assert_eq!(snapshot.next.len(), 2);
        assert_eq!(snapshot.next[1].title.as_deref(), Some("1999"));
        assert_eq!(snapshot.show.as_deref(), Some("Morning"));
        assert!(matches!(
            extract(&Extract::Json(Fields::default()), "<html>"),
            Err(ExtractError::Json(_))
        ));
    }

    #[test]
    fn text_pattern() {
        let pattern = Pattern {
            pattern: r"Now: (?<artist>[^/]+) / (?<title>[^<]+)<".into(),
        };
        let current = extract(&Extract::Text(pattern), "<b>Now: Air / Sexy Boy</b>")
            .unwrap()
            .current
            .unwrap();
        assert_eq!(
            (current.artist.as_deref(), current.title.as_deref()),
            (Some("Air"), Some("Sexy Boy"))
        );
    }

    #[test]
    fn ignored_next_track_still_ends_the_current_one() {
        let recipe = crate::recipe::Recipe::from_toml(
            r#"schema = 1
name = "LibreTime"
match = { streams = ["example.org/live"] }
fetch = { url = "https://example.org/live-info" }
ignore = ["(?i)^top"]
[extract]
type = "json"
item = "current"
title = "title"
start = "starts"
next = "next"
"#,
        )
        .unwrap();
        let page = |next: &str| {
            format!(
                r#"{{"current": {{"title": "Les douze maisons", "starts": "2026-11-06 09:57:52"}},
                "next": {{"title": "{next}", "starts": "2026-11-06 10:00:00"}}}}"#
            )
        };
        let snapshot = read(&recipe, &page("top 10h")).unwrap();
        assert!(snapshot.next.is_empty());
        assert_eq!(
            snapshot.current.unwrap().duration,
            Some(SignedDuration::from_secs(128))
        );
        // A next track that is kept gives the end itself: no duration made up.
        let snapshot = read(&recipe, &page("Nino")).unwrap();
        assert_eq!(snapshot.current.unwrap().duration, None);
    }
}
