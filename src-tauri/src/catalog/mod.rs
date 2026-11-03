// SPDX-License-Identifier: MPL-2.0
//! Station catalog.
//!
//! The rest of the application only talks to [`CatalogSource`]: today the Radio-Browser online API
//! ([`radio_browser`]); other sources can be plugged in without touching the interface.
//! A station is never silently hidden: an unreachable station is ranked after the others
//! and flagged, not removed.

pub mod radio_browser;

use std::future::Future;
use std::pin::Pin;

use serde::Serialize;

pub type CatalogFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CatalogError>> + Send + 'a>>;

pub trait CatalogSource: Send + Sync {
    fn search<'a>(&'a self, query: &'a SearchQuery) -> CatalogFuture<'a, SearchPage>;

    /// One station by its catalog ID (`None` if the catalog no longer has it).
    fn station<'a>(&'a self, id: &'a str) -> CatalogFuture<'a, Option<Station>>;

    /// Reports that a user started playing the station (popularity rankings).
    fn count_click<'a>(&'a self, station_id: &'a str) -> CatalogFuture<'a, ()>;
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
    Unreachable { last_ok: Option<String> },
    /// Cannot be played by our engine (HLS for now).
    Unsupported { reason: String },
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
}

pub const DEFAULT_LIMIT: u32 = 30;
pub const MAX_LIMIT: u32 = 100;

impl SearchQuery {
    /// Normalizes frontend input (whitespace, case, bounds); an empty value becomes `None`.
    pub fn sanitized(&self) -> Result<Self, CatalogError> {
        let clean = |v: &Option<String>| {
            v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
        };
        let country_code = clean(&self.country_code).map(|c| c.to_ascii_uppercase());
        if let Some(code) = &country_code {
            if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(CatalogError::BadRequest(format!("Invalid country code: {code}")));
            }
        }
        Ok(Self {
            text: clean(&self.text),
            country_code,
            tag: clean(&self.tag).map(|t| t.to_lowercase()),
            limit: Some(self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)),
            offset: Some(self.offset.unwrap_or(0)),
        })
    }
}

/// Ranks stations by health status, keeping the catalog's popularity order
/// within each group (stable sort).
pub fn rank(stations: &mut [Station]) {
    stations.sort_by_key(|s| s.health.rank());
}
