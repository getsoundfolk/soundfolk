// SPDX-License-Identifier: MPL-2.0
//! The commands the window can call, and the TypeScript bindings generated from them
//! (`src/lib/bindings.ts`): the types the front end uses are the Rust ones, never written by hand.
//!
//! The `bindings` test writes the file again; it fails when the file changed, so a stale copy does
//! not get past the CI. Run `cargo test bindings` after changing a command or a type it carries.

use serde::Serialize;
use specta::Type;
use tauri_specta::{collect_commands, Builder, ErrorHandlingMode};

use crate::*;

/// Every command, for the invoke handler and the bindings. `collect_commands!` takes no `#[cfg]`:
/// the desktop-only ones come as extra tokens.
macro_rules! commands {
    ($($extra:tt)*) => {
        collect_commands![
            commands::play_stream,
            commands::stop_stream,
            commands::pause_stream,
            commands::resume_stream,
            volume::set_volume,
            volume::get_volume,
            volume::save_volume,
            commands::get_player_state,
            commands::get_now_playing,
            commands::get_current_station,
            commands::add_favorite,
            commands::remove_favorite,
            commands::list_favorites,
            commands::reorder_favorites,
            commands::get_listening_history,
            commands::get_history_summary,
            commands::clear_listening_history,
            commands::export_history,
            commands::like_track,
            commands::unlike_track,
            commands::list_likes,
            commands::get_stats,
            commands::get_track_identification,
            commands::get_cover_sources,
            commands::set_cover_sources,
            commands::get_cover,
            commands::set_track_identification,
            commands::search_stations,
            commands::get_station,
            commands::surprise_candidates,
            commands::resolve_station,
            commands::get_click_counting,
            commands::set_click_counting,
            commands::get_web_titles,
            commands::set_web_titles,
            commands::get_autostart,
            commands::set_autostart,
            notifications::get_track_notifications,
            notifications::set_track_notifications,
            volume::get_constant_volume,
            volume::set_constant_volume,
            volume::get_level_adjustment,
            volume::get_muted,
            volume::set_muted,
            commands::get_title_status,
            commands::reload_recipes,
            commands::test_recipe,
            commands::edit_recipe,
            commands::share_recipe,
            commands::get_local_recipes,
            commands::open_recipes_folder,
            commands::get_track_services,
            commands::set_track_services,
            commands::get_custom_track_services,
            commands::set_custom_track_services,
            commands::add_personal_station,
            commands::list_personal_stations,
            commands::remove_personal_station,
            commands::get_station_names,
            commands::set_station_name,
            donations::get_donation_link,
            curated_lists::get_curated_lists,
            curated_lists::get_lists_for_station,
            curated_lists::get_curated_list_stations,
            curated_lists::export_curated_list,
            curated_lists::get_favorite_lists,
            curated_lists::set_favorite_list,
            curated_lists::export_my_list,
            curated_lists::refresh_curated_lists,
            share::read_share_link,
            share::make_share_link,
            share::get_share_name,
            share::set_share_name,
            share::take_opened_link,
            share::catalog_station_for_stream,
            mini::open_mini_player,
            mini::close_mini_player,
            mini::set_mini_compact,
            mini::switch_favorite,
            $($extra)*
        ]
    };
}

pub fn builder() -> Builder<tauri::Wry> {
    #[cfg(desktop)]
    let commands = commands!(
        updates::get_update_check,
        updates::set_update_check,
        updates::get_update,
        updates::skip_update
    );
    #[cfg(not(desktop))]
    let commands = commands!();
    Builder::new()
        .commands(commands)
        // A failed command rejects its promise with an `AppError`, as `invoke` does.
        .error_handling(ErrorHandlingMode::Throw)
        // Database ids and counts are `i64` and `usize`: far below 2^53, a JavaScript number holds
        // them exactly.
        .dangerously_cast_bigints_to_number()
        // One type per Rust type, not one per direction.
        .disable_serde_phases()
        .typ::<EventPayloads>()
        .typ::<error::AppError>()
}

/// A float the window receives as a plain `number`. Specta types every float as `number | null`
/// (serde writes NaN and the infinities as `null`): `#[specta(type = Number)]` on one that is never
/// NaN or infinite.
pub(crate) type Number = i32;

/// The events sent to the windows, by name, with their payload: the front end listens to them
/// through this map (`listen` in `api.ts`). `null` is an event without payload.
#[derive(Serialize, Type)]
#[allow(dead_code)] // Only its type is used.
struct EventPayloads {
    #[serde(rename = "player:state")]
    player_state: audio::PlayerState,
    #[serde(rename = "player:now-playing")]
    player_now_playing: audio::NowPlaying,
    #[serde(rename = "player:cover")]
    player_cover: covers::Cover,
    #[serde(rename = "player:muted")]
    player_muted: bool,
    #[serde(rename = "player:title-status")]
    player_title_status: audio::TitleStatus,
    #[serde(rename = "player:station")]
    player_station: media_controls::StationChange,
    #[serde(rename = "recipes:notice")]
    recipes_notice: audio::RecipeNotice,
    #[serde(rename = "favorite:gone")]
    favorite_gone: String,
    #[serde(rename = "favorites:changed")]
    favorites_changed: (),
    #[serde(rename = "donations:updated")]
    donations_updated: (),
    #[serde(rename = "lists:updated")]
    lists_updated: (),
    #[serde(rename = "share:opened")]
    share_opened: (),
    #[serde(rename = "updates:available")]
    updates_available: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

    /// The bindings as generated now.
    fn generated(name: &str) -> String {
        let path = std::env::temp_dir().join(format!("soundfolk-{name}-{}.ts", std::process::id()));
        builder()
            .export(
                specta_typescript::Typescript::default().header(
                    "// SPDX-License-Identifier: MPL-2.0\n\
                     // Generated by `cargo test bindings` (src-tauri/src/bindings.rs): do not edit.",
                ),
                &path,
            )
            .expect("TypeScript bindings not written");
        let text = std::fs::read_to_string(&path).expect("bindings not read back");
        let _ = std::fs::remove_file(&path);
        text
    }

    /// Writes `src/lib/bindings.ts` when it is not up to date, and fails then.
    #[test]
    fn bindings() {
        let now = generated("bindings");
        let committed = std::fs::read_to_string(PATH).unwrap_or_default();
        // Git may check the file out with CRLF line ends on Windows.
        if committed.replace("\r\n", "\n") != now {
            std::fs::write(PATH, &now).expect("bindings.ts not written");
            panic!("src/lib/bindings.ts was out of date: written again, commit it");
        }
    }

    /// Each name in the map is one the backend sends.
    #[test]
    fn event_names() {
        let names = [
            PLAYER_STATE_EVENT,
            TITLE_STATUS_EVENT,
            RECIPE_NOTICE_EVENT,
            DONATIONS_EVENT,
            LISTS_EVENT,
            share::OPENED_EVENT,
            favorite_repair::FAVORITES_EVENT,
            favorite_repair::GONE_EVENT,
            media_controls::STATION_EVENT,
            updates::UPDATE_EVENT,
            listening::NOW_PLAYING_EVENT,
            covers::COVER_EVENT,
            volume::MUTED_EVENT,
        ];
        let bindings = generated("events");
        let map = &bindings[bindings
            .find("export type EventPayloads")
            .expect("no EventPayloads")..];
        let map = &map[..map.find('}').expect("EventPayloads not closed")];
        assert_eq!(map.matches(": ").count(), names.len(), "{map}");
        for name in names {
            assert!(
                map.contains(&format!("\"{name}\"")),
                "{name} missing from EventPayloads"
            );
        }
    }
}
