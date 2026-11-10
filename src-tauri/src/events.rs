// SPDX-License-Identifier: MPL-2.0
//! The events sent to the windows: their names and payloads, in one table. It gives each name its
//! constant, for `emit`, and `EventPayloads`, the map the TypeScript bindings carry (`listen` in
//! `api.ts` is typed by it), so a name exists once and the front end cannot misspell it.

use serde::Serialize;
use specta::Type;

use crate::audio::{NowPlaying, PlayerState};
use crate::covers::Cover;
use crate::media_controls::StationChange;
use crate::now_playing::{RecipeNotice, TitleStatus};

macro_rules! events {
    ($($(#[doc = $doc:literal])* $constant:ident = $name:literal: $payload:ty,)*) => {
        $(
            $(#[doc = $doc])*
            pub(crate) const $constant: &str = $name;
        )*

        /// The events sent to the windows, by name, with their payload: the front end listens to
        /// them through this map (`listen` in `api.ts`). `null` is an event without payload.
        #[derive(Serialize, Type)]
        #[allow(dead_code, non_snake_case)] // Only its type is used.
        pub(crate) struct EventPayloads {
            $(
                #[serde(rename = $name)]
                $constant: $payload,
            )*
        }
    };
}

events! {
    /// Every player state change.
    PLAYER_STATE = "player:state": PlayerState,
    /// The on-air station or title changed.
    NOW_PLAYING = "player:now-playing": NowPlaying,
    /// The cover of the track on air changed.
    COVER = "player:cover": Cover,
    /// The sound is cut or comes back (payload: whether it is cut).
    MUTED = "player:muted": bool,
    /// The source of the title changed.
    TITLE_STATUS = "player:title-status": TitleStatus,
    /// The media controls or a favorite's repair switched station, so that the window shows the
    /// station on air.
    STATION = "player:station": StationChange,
    /// A local recipe file was saved.
    RECIPE_NOTICE = "recipes:notice": RecipeNotice,
    /// The favorite on air is no longer in the catalog (payload: its ID).
    FAVORITE_GONE = "favorite:gone": String,
    /// The window should read the favorites again (one of them has a new address).
    FAVORITES_CHANGED = "favorites:changed": (),
    /// A new list of donation links was downloaded.
    DONATIONS_UPDATED = "donations:updated": (),
    /// New curated lists were downloaded.
    LISTS_UPDATED = "lists:updated": (),
    /// A `soundfolk://` link was opened: the window takes it with `take_opened_link`, also when it
    /// opens later.
    SHARE_OPENED = "share:opened": (),
    /// A newer version was found: the window asks `get_update`.
    UPDATE_AVAILABLE = "updates:available": (),
}
