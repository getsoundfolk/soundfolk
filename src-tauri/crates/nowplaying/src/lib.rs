// SPDX-License-Identifier: MPL-2.0
//! Now-playing engine for streams that publish no in-stream metadata.
//!
//! This crate does **no I/O**: the host fetches pages and passes their content in, which keeps
//! the same code usable by the desktop app, a server daemon or WebAssembly.
//!
//! ```text
//! Recipe / Adapter ──▶ extract(page) ──▶ Snapshot ──▶ Tracker ──▶ Plan
//! (what to read)       (what the page says)           (clock, schedule)  (what to show when,
//!                                                                         when to fetch again)
//! ```
//!
//! * [`recipe`]: declarative recipes (TOML), one per station, and how they match a stream.
//! * [`extract`]: reads a page (HTML, JSON or text) as a recipe says.
//! * [`adapters`]: generic readers for hosting platforms (Icecast, Shoutcast).
//! * [`clock`]: turns a station's local `HH:MM:SS` into instants without knowing its time zone.
//! * [`tracker`]: decides what to show and when, and when to fetch again, within enforced limits.

pub mod adapters;
pub mod check;
pub mod clock;
pub mod extract;
pub mod recipe;
pub mod tracker;
mod value;

pub use jiff::{civil::Time, SignedDuration, Timestamp};

/// When a track starts, as a source gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Start {
    /// An absolute instant (Unix timestamp, ISO 8601 with offset).
    At(Timestamp),
    /// A wall-clock time in the station's (unknown) time zone, without a date.
    Local(Time),
}

/// A track as read from a page, before times are resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawTrack {
    pub artist: Option<String>,
    /// The title, or the whole `Artist - Title` text when the source does not split it
    /// (`artist` is then `None`).
    pub title: Option<String>,
    pub start: Option<Start>,
    pub duration: Option<SignedDuration>,
    pub cover: Option<String>,
}

impl RawTrack {
    fn is_empty(&self) -> bool {
        self.artist.is_none() && self.title.is_none()
    }
}

/// What a page says is on air.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub current: Option<RawTrack>,
    /// Coming up, in order.
    pub next: Vec<RawTrack>,
    /// Name of the show on air, if the page gives it.
    pub show: Option<String>,
}

/// A track with its times resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub artist: Option<String>,
    pub title: String,
    pub started_at: Option<Timestamp>,
    pub duration: Option<SignedDuration>,
    pub cover: Option<String>,
}

impl Track {
    /// `Artist - Title`, the form ICY uses.
    pub fn display(&self) -> String {
        match &self.artist {
            Some(artist) => format!("{artist} - {}", self.title),
            None => self.title.clone(),
        }
    }

    /// When the track ends, if known.
    pub fn ends_at(&self) -> Option<Timestamp> {
        self.started_at?.checked_add(self.duration?).ok()
    }
}

/// Why a page could not be read.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtractError {
    /// The document is not valid JSON.
    Json(String),
    /// A selector, path or pattern of the recipe is invalid.
    Recipe(String),
    /// The document does not have the expected shape (e.g. not an Icecast status).
    Shape(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid JSON: {e}"),
            Self::Recipe(e) => write!(f, "invalid recipe: {e}"),
            Self::Shape(e) => write!(f, "unexpected document: {e}"),
        }
    }
}

impl std::error::Error for ExtractError {}
