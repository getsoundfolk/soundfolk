// SPDX-License-Identifier: MPL-2.0
//! Export of the listening history and the liked tracks as CSV or JSON files.
//!
//! Times are written in ISO 8601 UTC (`2026-11-05T21:36:51Z`): the database stores UTC without a
//! zone marker, which spreadsheets and scripts would read as local time.

use serde::{Deserialize, Serialize};

use crate::db::{Database, HistoryEntry, LikedTrack};
use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Json,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// What is exported: the whole listening history or the liked tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum List {
    History,
    Likes,
}

impl List {
    /// Start of the file name (`soundfolk-history-2026-11-05.csv`).
    pub fn file_stem(self) -> &'static str {
        match self {
            Self::History => "soundfolk-history",
            Self::Likes => "soundfolk-likes",
        }
    }
}

#[derive(Debug, Serialize)]
struct HistoryRow<'a> {
    played_at: String,
    artist: Option<&'a str>,
    title: &'a str,
    /// Title as announced on air, before splitting artist and title.
    raw_title: &'a str,
    station_name: Option<&'a str>,
    station_id: &'a str,
    liked: bool,
}

#[derive(Debug, Serialize)]
struct LikedRow<'a> {
    liked_at: String,
    artist: Option<&'a str>,
    title: &'a str,
    raw_title: &'a str,
    station_name: Option<&'a str>,
    station_id: &'a str,
}

const HISTORY_COLUMNS: [&str; 7] = [
    "played_at",
    "artist",
    "title",
    "raw_title",
    "station_name",
    "station_id",
    "liked",
];
const LIKED_COLUMNS: [&str; 6] = [
    "liked_at",
    "artist",
    "title",
    "raw_title",
    "station_name",
    "station_id",
];

/// The whole history or all liked tracks (narrowed by `query`, as in the History tab) in `format`,
/// with the number of rows.
pub fn render(
    db: &Database,
    list: List,
    format: Format,
    query: Option<&str>,
) -> AppResult<(String, usize)> {
    Ok(match list {
        List::History => {
            let entries = db.recent_history(u32::MAX, query)?;
            (history(&entries, format), entries.len())
        }
        List::Likes => {
            let tracks = db.list_likes(u32::MAX, query)?;
            (likes(&tracks, format), tracks.len())
        }
    })
}

pub fn history(entries: &[HistoryEntry], format: Format) -> String {
    let rows: Vec<HistoryRow> = entries
        .iter()
        .map(|e| HistoryRow {
            played_at: iso_utc(&e.played_at),
            artist: e.artist.as_deref(),
            title: &e.title,
            raw_title: &e.track_title,
            station_name: e.station_name.as_deref(),
            station_id: &e.station_id,
            liked: e.liked,
        })
        .collect();
    match format {
        Format::Json => json(&rows),
        Format::Csv => csv(
            &HISTORY_COLUMNS,
            rows.iter().map(|r| {
                vec![
                    r.played_at.clone(),
                    r.artist.unwrap_or_default().to_owned(),
                    r.title.to_owned(),
                    r.raw_title.to_owned(),
                    r.station_name.unwrap_or_default().to_owned(),
                    r.station_id.to_owned(),
                    r.liked.to_string(),
                ]
            }),
        ),
    }
}

pub fn likes(tracks: &[LikedTrack], format: Format) -> String {
    let rows: Vec<LikedRow> = tracks
        .iter()
        .map(|t| LikedRow {
            liked_at: iso_utc(&t.liked_at),
            artist: t.artist.as_deref(),
            title: &t.title,
            raw_title: &t.raw_title,
            station_name: t.station_name.as_deref(),
            station_id: &t.station_id,
        })
        .collect();
    match format {
        Format::Json => json(&rows),
        Format::Csv => csv(
            &LIKED_COLUMNS,
            rows.iter().map(|r| {
                vec![
                    r.liked_at.clone(),
                    r.artist.unwrap_or_default().to_owned(),
                    r.title.to_owned(),
                    r.raw_title.to_owned(),
                    r.station_name.unwrap_or_default().to_owned(),
                    r.station_id.to_owned(),
                ]
            }),
        ),
    }
}

fn json<T: Serialize>(rows: &[T]) -> String {
    let mut out = serde_json::to_string_pretty(rows).expect("rows serialize");
    out.push('\n');
    out
}

/// RFC 4180: CRLF line ends, fields quoted when needed. Starts with a UTF-8 byte order mark so
/// spreadsheets do not mangle accented station names and titles.
fn csv(columns: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut out = String::from('\u{feff}');
    let mut line = |fields: &mut dyn Iterator<Item = &str>| {
        let fields: Vec<String> = fields.map(csv_field).collect();
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    };
    line(&mut columns.iter().copied());
    for row in rows {
        line(&mut row.iter().map(String::as_str));
    }
    out
}

/// Characters that make a spreadsheet read a cell as a formula (OWASP "CSV injection").
const FORMULA_TRIGGERS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

fn csv_field(field: &str) -> String {
    // Titles come from radio streams: a cell like `=HYPERLINK(...)` must stay text. A leading `'`
    // tells the spreadsheet so, and is not displayed.
    let neutralized;
    let field = if field.starts_with(FORMULA_TRIGGERS) {
        neutralized = format!("'{field}");
        neutralized.as_str()
    } else {
        field
    };
    if field.contains([',', '"', '\n', '\r']) || field.starts_with(' ') || field.ends_with(' ') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// `YYYY-MM-DD HH:MM:SS` (SQLite, UTC) → `YYYY-MM-DDTHH:MM:SSZ`; anything else is kept as is.
fn iso_utc(sqlite: &str) -> String {
    let b = sqlite.as_bytes();
    if b.len() == 19 && b[10] == b' ' {
        format!("{}T{}Z", &sqlite[..10], &sqlite[11..])
    } else {
        sqlite.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, artist: Option<&str>, station: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            id: 1,
            station_id: "f7".into(),
            station_name: station.map(Into::into),
            track_title: match artist {
                Some(a) => format!("{a} - {title}"),
                None => title.into(),
            },
            artist: artist.map(Into::into),
            title: title.into(),
            track_key: String::new(),
            played_at: "2026-11-04 21:36:51".into(),
            liked: true,
            search_query: String::new(),
        }
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        let out = history(
            &[
                entry("Home, \"Abroad\"", Some("Mellow Mood"), Some("Fréquence 7")),
                entry("Jingle", None, None),
            ],
            Format::Csv,
        );
        let lines: Vec<&str> = out.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert!(out.starts_with('\u{feff}'));
        assert_eq!(
            lines[0],
            "played_at,artist,title,raw_title,station_name,station_id,liked"
        );
        assert_eq!(
            lines[1],
            r#"2026-11-04T21:36:51Z,Mellow Mood,"Home, ""Abroad""","Mellow Mood - Home, ""Abroad""",Fréquence 7,f7,true"#
        );
        assert_eq!(lines[2], "2026-11-04T21:36:51Z,,Jingle,Jingle,,f7,true");
        assert_eq!(lines[3], "");
    }

    #[test]
    fn json_keeps_missing_values_as_null() {
        let out = history(&[entry("Jingle", None, None)], Format::Json);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value[0]["artist"], serde_json::Value::Null);
        assert_eq!(value[0]["played_at"], "2026-11-04T21:36:51Z");
        assert_eq!(value[0]["liked"], true);
        assert_eq!(history(&[], Format::Json), "[]\n");
    }

    #[test]
    fn likes_export_their_own_time() {
        let like = LikedTrack {
            track_key: "air|sexy boy".into(),
            artist: Some("Air".into()),
            title: "Sexy Boy".into(),
            raw_title: "Air - Sexy Boy".into(),
            station_id: "fip".into(),
            station_name: Some("FIP".into()),
            liked_at: "2026-11-05 08:00:00".into(),
            search_query: String::new(),
        };
        let out = likes(&[like], Format::Csv);
        assert!(out.contains("\r\n2026-11-05T08:00:00Z,Air,Sexy Boy,Air - Sexy Boy,FIP,fip\r\n"));
    }

    #[test]
    fn fields_with_line_breaks_or_edge_spaces_are_quoted() {
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
        assert_eq!(csv_field(" padded"), "\" padded\"");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn formula_cells_are_neutralized() {
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("+33 1 23"), "'+33 1 23");
        assert_eq!(
            csv_field("-M- - Qui de nous deux"),
            "'-M- - Qui de nous deux"
        );
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("\tcmd"), "'\tcmd");
        assert_eq!(
            csv_field("=HYPERLINK(\"http://x\",\"a\")"),
            "\"'=HYPERLINK(\"\"http://x\"\",\"\"a\"\")\""
        );
        assert_eq!(csv_field("Air - Sexy Boy"), "Air - Sexy Boy");
        let out = history(&[entry("=cmd|' /C calc'!A0", None, None)], Format::Csv);
        assert!(out.contains(",'=cmd|' /C calc'!A0,'=cmd|' /C calc'!A0,"));
    }
}
