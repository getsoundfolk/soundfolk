// SPDX-License-Identifier: MPL-2.0
//! Track titles as announced on air (ICY `StreamTitle`): splitting into artist and title,
//! search queries, and comparison keys.
//!
//! The raw title is never altered: everything here is derived from it and can be recomputed
//! (bump [`PARSER_VERSION`] when the rules change, the history is then re-parsed at startup).

use unicode_normalization::UnicodeNormalization;

/// Version of the parsing rules below.
pub const PARSER_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTitle {
    pub artist: Option<String>,
    pub title: String,
}

/// Separators between artist and title (ICY convention: `Artist - Title`).
const DASHES: [&str; 3] = [" - ", " – ", " — "];
/// Separators before a station signature at the end of a title.
const TAIL_SEPARATORS: [&str; 4] = [" - ", " – ", " — ", " | "];
/// Top-level domains recognised in a station signature (`… - Classic Vinyl on walmradio.com`).
const TLDS: [&str; 24] = [
    "com", "net", "org", "fm", "radio", "info", "io", "de", "fr", "uk", "co", "nl", "be", "ch",
    "it", "es", "us", "ca", "au", "live", "online", "music", "am", "pl",
];
/// Words that mark a bracketed group as noise for searching (`(Remastered 2011)`, `[Live]`…).
const NOISE: [&str; 11] = [
    "remaster",
    "live",
    "radio edit",
    "edit",
    "version",
    "mono",
    "stereo",
    "explicit",
    "clean",
    "bonus",
    "single",
];

/// Splits a raw title into artist and title, after removing a trailing station signature.
pub fn parse(raw: &str, station_name: Option<&str>) -> ParsedTitle {
    let mut text = collapse_spaces(raw);

    // A signature may be stacked (`… - Station - site.com`): at most two are removed.
    for _ in 0..2 {
        let Some((pos, sep)) = last_of(&text, &TAIL_SEPARATORS) else {
            break;
        };
        let (head, tail) = (&text[..pos], &text[pos + sep.len()..]);
        if head.trim().is_empty() || !is_signature(tail, station_name) {
            break;
        }
        text = head.trim().to_owned();
    }

    if let Some((pos, sep)) = first_of(&text, &DASHES) {
        let (artist, title) = (text[..pos].trim(), text[pos + sep.len()..].trim());
        if !artist.is_empty() && !title.is_empty() {
            return ParsedTitle {
                artist: Some(artist.to_owned()),
                title: title.to_owned(),
            };
        }
    }
    // `Title by Artist`: the last ` by ` keeps titles such as "Stand By Me by Ben E. King" whole.
    if let Some(pos) = last_by(&text) {
        let (title, artist) = (text[..pos].trim(), text[pos + 4..].trim());
        if !artist.is_empty() && !title.is_empty() {
            return ParsedTitle {
                artist: Some(artist.to_owned()),
                title: title.to_owned(),
            };
        }
    }
    ParsedTitle {
        artist: None,
        title: text,
    }
}

/// What to type in a music service's search box: artist and title, without noise.
pub fn search_query(parsed: &ParsedTitle) -> String {
    let mut parts = Vec::new();
    if let Some(artist) = &parsed.artist {
        parts.push(clean_for_search(artist));
    }
    parts.push(clean_for_search(&parsed.title));
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

/// Title cleaned for searching (also used for MusicBrainz queries).
pub fn clean_for_search(text: &str) -> String {
    let without_noise = strip_noise(text);
    let replaced: String = without_noise
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | ':' | '!' | '?' | '"' | '*' | '|' | ';' | '(' | ')' | '[' | ']'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect();
    collapse_spaces(&replaced)
}

/// Key under which two spellings of the same track compare equal (case, accents, punctuation,
/// leading "The", featured artists and bracketed noise are ignored).
pub fn track_key(parsed: &ParsedTitle) -> String {
    let artist = parsed.artist.as_deref().map(artist_key).unwrap_or_default();
    format!("{artist}|{}", text_key(&strip_noise(&parsed.title)))
}

/// Comparison key of an artist name.
pub fn artist_key(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let main = [" feat. ", " feat ", " ft. ", " ft ", " featuring "]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .map_or(lower.as_str(), |pos| &lower[..pos]);
    let key = text_key(main);
    key.strip_prefix("the ").map(str::to_owned).unwrap_or(key)
}

fn text_key(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    collapse_spaces(&folded)
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes bracketed groups whose content is noise for searching.
fn strip_noise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(len) = rest[open..].find(close_char) else {
            break;
        };
        let inner = rest[open + 1..open + len].to_lowercase();
        out.push_str(&rest[..open]);
        if !NOISE.iter().any(|n| inner.contains(n)) {
            out.push_str(&rest[open..=open + len]);
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    collapse_spaces(&out)
}

/// A trailing segment that names the station or its website rather than the track.
fn is_signature(tail: &str, station_name: Option<&str>) -> bool {
    let lower = tail.to_lowercase();
    if lower.contains("on air") || lower.split_whitespace().any(is_domain) {
        return true;
    }
    let (tail_key, Some(station_key)) = (text_key(tail), station_name.map(text_key)) else {
        return false;
    };
    !tail_key.is_empty()
        && !station_key.is_empty()
        && (tail_key == station_key
            || (tail_key.len() >= 4 && station_key.contains(&tail_key))
            || tail_key.contains(&station_key))
}

fn is_domain(word: &str) -> bool {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    let word = word
        .strip_prefix("https://")
        .or_else(|| word.strip_prefix("http://"))
        .unwrap_or(word);
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && TLDS.contains(&parts[parts.len() - 1])
}

fn first_of<'a>(text: &str, seps: &[&'a str]) -> Option<(usize, &'a str)> {
    seps.iter()
        .filter_map(|s| text.find(s).map(|p| (p, *s)))
        .min_by_key(|(p, _)| *p)
}

fn last_of<'a>(text: &str, seps: &[&'a str]) -> Option<(usize, &'a str)> {
    seps.iter()
        .filter_map(|s| text.rfind(s).map(|p| (p, *s)))
        .max_by_key(|(p, _)| *p)
}

/// Byte position of the last ` by ` (any case). The pattern is ASCII, so a match is always on a
/// character boundary.
fn last_by(text: &str) -> Option<usize> {
    text.as_bytes()
        .windows(4)
        .rposition(|w| w.eq_ignore_ascii_case(b" by "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(raw: &str, station: Option<&str>) -> (Option<String>, String) {
        let parsed = parse(raw, station);
        (parsed.artist, parsed.title)
    }

    fn pair(artist: &str, title: &str) -> (Option<String>, String) {
        (Some(artist.to_owned()), title.to_owned())
    }

    #[test]
    fn parses_real_titles() {
        type Case = (&'static str, Option<&'static str>, (Option<String>, String));
        let cases: &[Case] = &[
            (
                "Overture / Food, Glorious Food by Original Broadway Cast of Oliver! - Classic Vinyl on walmradio.com",
                Some("Classic Vinyl HD"),
                pair("Original Broadway Cast of Oliver!", "Overture / Food, Glorious Food"),
            ),
            ("Daft Punk - Around the World", None, pair("Daft Punk", "Around the World")),
            ("Ben E. King - Stand by Me", None, pair("Ben E. King", "Stand by Me")),
            ("Stand By Me by Ben E. King", None, pair("Ben E. King", "Stand By Me")),
            ("Miles Davis – So What", None, pair("Miles Davis", "So What")),
            ("Nina Simone — Feeling Good", None, pair("Nina Simone", "Feeling Good")),
            ("  Air  -  La femme d'argent ", None, pair("Air", "La femme d'argent")),
            ("Jay-Z - 99 Problems", None, pair("Jay-Z", "99 Problems")),
            ("Artist - Title - Extended Mix", None, pair("Artist", "Title - Extended Mix")),
            ("Bonobo - Kerala | FIP", Some("FIP"), pair("Bonobo", "Kerala")),
            ("Bonobo - Kerala - Radio Paradise", Some("Radio Paradise - Main Mix"), pair("Bonobo", "Kerala")),
            ("Moby - Porcelain | www.example.fm", None, pair("Moby", "Porcelain")),
            ("Moby - Porcelain - On Air", None, pair("Moby", "Porcelain")),
            ("Moby - Porcelain - Station Name - site.net", Some("Station Name"), pair("Moby", "Porcelain")),
            ("Clair de lune", None, (None, "Clair de lune".into())),
            ("Debussy: Clair de lune", None, (None, "Debussy: Clair de lune".into())),
            ("- Untitled", None, (None, "- Untitled".into())),
            ("Sigur Rós - Hoppípolla", None, pair("Sigur Rós", "Hoppípolla")),
            ("Ölüm by Ezhel", None, pair("Ezhel", "Ölüm")),
            ("Kerala | Bonobo", None, (None, "Kerala | Bonobo".into())),
        ];
        for (raw, station, expected) in cases {
            assert_eq!(&p(raw, *station), expected, "{raw}");
        }
    }

    #[test]
    fn search_queries_drop_noise() {
        let parsed = parse("The Beatles - Help! (Remastered 2009) [Live]", None);
        assert_eq!(search_query(&parsed), "The Beatles Help");
        let walm = parse(
            "Overture / Food, Glorious Food by Original Broadway Cast of Oliver!",
            None,
        );
        assert_eq!(
            search_query(&walm),
            "Original Broadway Cast of Oliver Overture Food, Glorious Food"
        );
        // A bracketed group that is part of the title stays.
        assert_eq!(
            search_query(&parse("Prince - Purple Rain (Extended)", None)),
            "Prince Purple Rain Extended"
        );
    }

    #[test]
    fn keys_ignore_spelling_differences() {
        let key = |raw: &str| track_key(&parse(raw, None));
        assert_eq!(
            key("The Beatles - Help!"),
            key("beatles - HELP (Remastered 2009)")
        );
        assert_eq!(
            key("Beyoncé feat. Jay-Z - Crazy in Love"),
            key("Beyonce - Crazy In Love")
        );
        assert_ne!(
            key("Daft Punk - Around the World"),
            key("Daft Punk - One More Time")
        );
        assert_eq!(
            artist_key("The Original Broadway Cast"),
            "original broadway cast"
        );
    }
}
