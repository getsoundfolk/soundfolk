// SPDX-License-Identifier: MPL-2.0
//! Station catalog.
//!
//! The rest of the application only talks to [`CatalogSource`]: today the Radio-Browser online API
//! ([`radio_browser`]); other sources can be plugged in without touching the interface.
//! A station is never silently hidden: an unreachable station is ranked after the others
//! and flagged, not removed.

pub mod dedup;
pub mod name_query;
pub mod radio_browser;
pub mod surprise;

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use serde::Serialize;

pub type CatalogFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CatalogError>> + Send + 'a>>;

pub trait CatalogSource: Send + Sync {
    fn search<'a>(&'a self, query: &'a SearchQuery) -> CatalogFuture<'a, SearchPage>;

    /// One station by its catalog ID (`None` if the catalog no longer has it).
    fn station<'a>(&'a self, id: &'a str) -> CatalogFuture<'a, Option<Station>>;

    /// Several stations by their catalog IDs, in the order of `ids`; the ones the catalog no
    /// longer has are left out.
    fn stations<'a>(&'a self, ids: &'a [String]) -> CatalogFuture<'a, Vec<Station>>;

    /// `limit` stations drawn at random among those the catalog's last check found up (Surprise
    /// me, see [`surprise`]).
    fn random<'a>(&'a self, limit: u32) -> CatalogFuture<'a, Vec<Station>>;

    /// Reports that a user started playing the station (popularity rankings).
    fn count_click<'a>(&'a self, station_id: &'a str) -> CatalogFuture<'a, ()>;

    /// The playable catalog station with this stream address, if any (a station shared or
    /// received as a personal one may be in the catalog, docs/16-sharing.md §2.2).
    fn by_stream<'a>(&'a self, url: &'a str) -> CatalogFuture<'a, Option<Station>>;
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("Catalog unreachable: {0}")]
    Network(String),
    #[error("Unexpected catalog response: {0}")]
    Invalid(String),
    #[error("{0}")]
    BadRequest(String),
}

/// Health status of a station, as known to the catalog.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Health {
    Ok,
    /// The catalog's last check failed. May be a false negative (brief outage,
    /// geo-blocking of the checking servers): playback is still allowed.
    Unreachable {
        last_ok: Option<String>,
    },
    /// Cannot be played by our engine (HLS for now).
    Unsupported {
        reason: String,
    },
}

impl Health {
    /// Display rank: healthy stations first, then suspect ones, then unplayable ones.
    fn rank(&self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Unreachable { .. } => 1,
            Self::Unsupported { .. } => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub url: String,
    pub homepage: Option<String>,
    pub favicon: Option<String>,
    pub tags: Vec<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub bitrate: u32,
    pub votes: u32,
    pub health: Health,
}

impl Station {
    /// Whether the engine can play this station (HLS is not supported yet): unsupported stations
    /// are never returned by a search.
    pub fn is_supported(&self) -> bool {
        !matches!(self.health, Health::Unsupported { .. })
    }
}

/// One page of search results.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchPage {
    pub stations: Vec<Station>,
    /// Offset to pass to get the next page, or `None` when the catalog has no more results.
    /// It counts the catalog's own entries (including the ones filtered out), not `stations.len()`.
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    pub text: Option<String>,
    pub country_code: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// IDs of the user's favorites, filled by the app (never by the frontend): a duplicate group
    /// that holds one shows that copy (see [`dedup`]).
    #[serde(skip)]
    pub favorites: HashSet<String>,
}

pub const DEFAULT_LIMIT: u32 = 30;
pub const MAX_LIMIT: u32 = 100;

impl SearchQuery {
    /// Normalizes frontend input (whitespace, case, bounds); an empty value becomes `None`.
    pub fn sanitized(&self) -> Result<Self, CatalogError> {
        let clean = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let clean_text =
            |v: &Option<String>| v.as_deref().map(catalog_text).filter(|s| !s.is_empty());
        let country_code = clean(&self.country_code).map(|c| c.to_ascii_uppercase());
        if let Some(code) = &country_code
            && (code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()))
        {
            return Err(CatalogError::BadRequest(format!(
                "Invalid country code: {code}"
            )));
        }
        Ok(Self {
            text: clean_text(&self.text),
            country_code,
            tag: clean_text(&self.tag).map(|t| t.to_lowercase()),
            limit: Some(self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)),
            offset: Some(self.offset.unwrap_or(0)),
            favorites: self.favorites.clone(),
        })
    }
}

/// Rewrites pasted text into the characters the catalog stores. The catalog already ignores case,
/// accents, full-width forms and no-break spaces, but compares the rest character by character:
/// a typographic `’` or `–`, a doubled space or an invisible character copied from a web page
/// finds nothing ("Wonder 80’s Flashback" did). Most catalog names use the ASCII forms.
/// Also trims the text.
fn catalog_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' | '\u{00B4}' | '`' => {
                out.push('\'')
            }
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' | '\u{00AB}'
            | '\u{00BB}' => out.push('"'),
            // Hyphen, non-breaking hyphen, figure dash, en and em dashes, minus sign.
            '\u{2010}'..='\u{2015}' | '\u{2212}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            // Zero-width characters, soft hyphen and byte order mark.
            '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{00AD}' | '\u{FEFF}' => {}
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// Ranks stations by health status, keeping the catalog's popularity order
/// within each group (stable sort).
pub fn rank(stations: &mut [Station]) {
    stations.sort_by_key(|s| s.health.rank());
}
