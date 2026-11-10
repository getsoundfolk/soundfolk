// SPDX-License-Identifier: MPL-2.0
//! What the window is told about web titles: where the title comes from, recipe notices and
//! test readings.

use serde::Serialize;

use crate::audio::TrackInfo;
use crate::recipes::{Entry, Origin};

/// Where the title of the current playback comes from, for the station sheet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct TitleStatus {
    pub state: SourceState,
    /// The recipe in use, or the one that would be used.
    pub recipe: Option<RecipeRef>,
    /// The server status adapter in use (`icecast`, `shoutcast-v2`…); with `Stream`, the one
    /// giving the details of the stream's titles (`azuracast`).
    pub adapter: Option<String>,
    /// Why the last reading failed (page unreachable, nothing found…).
    pub error: Option<String>,
    /// The station's local recipe file exists but cannot be used.
    pub local_error: Option<LocalError>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// Nothing playing yet.
    #[default]
    Idle,
    /// Web titles are turned off in the settings.
    Off,
    /// Looking for a source (waiting for the stream to announce a title, probing the server).
    Searching,
    /// The stream announces its titles itself.
    Stream,
    Recipe,
    Server,
    /// No source found.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct RecipeRef {
    pub id: String,
    pub name: String,
    pub origin: Origin,
    pub path: Option<String>,
}

impl From<&Entry> for RecipeRef {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.recipe.name.clone(),
            origin: entry.origin,
            path: entry.path.as_ref().map(|p| p.display().to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct LocalError {
    pub path: String,
    pub error: String,
}

/// Feedback after saving a local recipe.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct RecipeNotice {
    /// The file name.
    pub file: String,
    pub ok: bool,
    pub message: String,
}

/// What a test reading of a recipe found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct RecipeTest {
    pub recipe: Option<RecipeRef>,
    pub local_error: Option<LocalError>,
    /// The page that was read.
    pub url: Option<String>,
    pub track: Option<TrackInfo>,
    pub next: Option<TrackInfo>,
    pub show: Option<String>,
    /// The station's UTC offset, in minutes, when the page gives local times.
    pub utc_offset_minutes: Option<i32>,
    /// What went wrong, step by step for the user.
    pub error: Option<String>,
}

/// Status changes and notices for the window.
pub enum WebEvent {
    Status(TitleStatus),
    Notice(RecipeNotice),
}

pub type WebListener = Box<dyn Fn(WebEvent) + Send + Sync>;
