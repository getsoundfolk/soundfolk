// SPDX-License-Identifier: MPL-2.0
//! Every setting the app keeps in the `settings` table, with its type and default: one list to
//! read them all, and a key can no longer be read as two types or with two defaults.

use rusqlite::{params, OptionalExtension};

use super::Database;

/// An on/off setting.
#[derive(Clone, Copy, Debug)]
pub struct Flag {
    key: &'static str,
    /// The value until the user changes it.
    pub default: bool,
}

/// A text, `None` until first set. Structured values are stored as JSON.
#[derive(Clone, Copy, Debug)]
pub struct Text(&'static str);

/// A list of short ids (no commas), `None` until first set. An empty list is a choice.
#[derive(Clone, Copy, Debug)]
pub struct List(&'static str);

impl Text {
    /// For a write inside another transaction.
    pub(super) fn key(self) -> &'static str {
        self.0
    }
}

// ── playback ─────────────────────────────────────────────────────────────────

/// Report to the catalog that a station is being played.
pub const CLICK_COUNTING: Flag = Flag {
    key: "click_counting",
    default: true,
};
/// True once the click-counting notice has been shown.
pub const CLICK_NOTICE_SEEN: Flag = Flag {
    key: "click_notice_seen",
    default: false,
};
/// Last station played (JSON `OnAir`): the app opens with it in the player, ready to play.
pub const LAST_STATION: Text = Text("last_station");
/// Volume the user last chose (`0.0..=1.0`), restored at the next launch.
pub const VOLUME: Text = Text("volume");
/// Even out the loudness of stations. On by default: it is what nearly every listener wants, and
/// its failure mode (a station a bit quieter than its own mastering) is mild.
pub const CONSTANT_VOLUME: Flag = Flag {
    key: "constant_volume",
    default: true,
};

// ── what is playing ──────────────────────────────────────────────────────────

/// Read the title of streams that do not announce it from the web (station page, server status).
pub const WEB_TITLES: Flag = Flag {
    key: "web_titles",
    default: true,
};
/// Identify liked tracks on MusicBrainz (off by default: titles leave the device).
pub const IDENTIFY_TRACKS: Flag = Flag {
    key: "identify_tracks",
    default: false,
};
/// A system notification for each new track.
pub const TRACK_NOTIFICATIONS: Flag = Flag {
    key: "track_notifications",
    default: false,
};
/// The cover sources in order, with their switch (JSON list of `CoverSource`).
pub const COVER_SOURCES: Text = Text("cover_sources");
/// Services listed under "Find this track on", by id. Unset: the frontend's defaults.
pub const TRACK_SERVICES: List = List("track_services");
/// Sites the user added to that menu (JSON list of `CustomService`).
pub const CUSTOM_TRACK_SERVICES: Text = Text("custom_track_services");

// ── stations ─────────────────────────────────────────────────────────────────

/// The curated lists marked as favorites: their ids, in the order they were marked.
pub const FAVORITE_LISTS: List = List("favorite_lists");
/// The sender's name put in share links (§4.2), empty by default.
pub const SHARE_FROM: Text = Text("share_from");

// ── mini player ──────────────────────────────────────────────────────────────

/// Whether the user left the mini player compact.
pub const MINI_COMPACT: Flag = Flag {
    key: "mini_compact",
    default: false,
};
/// Where the user last left the mini player, as physical `x,y` (displays can disagree on their
/// scale factor).
pub const MINI_POSITION: Text = Text("mini_position");

// ── updates ──────────────────────────────────────────────────────────────────

/// Whether to check for a new version (Settings › Privacy).
pub const UPDATE_CHECK: Flag = Flag {
    key: "update_check",
    default: true,
};
/// The version the listener chose to skip: no notice for it, the next one has one again.
pub const UPDATE_SKIPPED: Text = Text("update_skipped");

// ── internal ─────────────────────────────────────────────────────────────────

/// Version of the title parser the stored `artist` / `title` / `track_key` columns come from.
pub(super) const TITLE_PARSER_VERSION: Text = Text("title_parser_version");

impl Database {
    pub fn flag(&self, flag: Flag) -> rusqlite::Result<bool> {
        Ok(self.get(flag.key)?.map_or(flag.default, |v| v == "true"))
    }

    pub fn set_flag(&self, flag: Flag, value: bool) -> rusqlite::Result<()> {
        self.set_flags(&[(flag, value)])
    }

    /// Several flags that go together, written in one transaction.
    pub fn set_flags(&self, flags: &[(Flag, bool)]) -> rusqlite::Result<()> {
        let values: Vec<_> = flags
            .iter()
            .map(|(flag, value)| (flag.key, if *value { "true" } else { "false" }))
            .collect();
        self.put(&values)
    }

    pub fn text(&self, text: Text) -> rusqlite::Result<Option<String>> {
        self.get(text.0)
    }

    pub fn set_text(&self, text: Text, value: &str) -> rusqlite::Result<()> {
        self.put(&[(text.0, value)])
    }

    pub fn list(&self, list: List) -> rusqlite::Result<Option<Vec<String>>> {
        Ok(self.get(list.0)?.map(|v| {
            v.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        }))
    }

    pub fn set_list(&self, list: List, values: &[String]) -> rusqlite::Result<()> {
        self.put(&[(list.0, &values.join(","))])
    }

    fn get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
    }

    fn put(&self, settings: &[(&str, &str)]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (key, value) in settings {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_use_the_default_until_set() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.flag(CLICK_COUNTING).unwrap());
        assert!(!db.flag(CLICK_NOTICE_SEEN).unwrap());
        db.set_flag(CLICK_COUNTING, false).unwrap();
        db.set_flag(CLICK_COUNTING, false).unwrap(); // idempotent
        assert!(!db.flag(CLICK_COUNTING).unwrap());
    }

    #[test]
    fn lists_tell_unset_from_empty() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.list(TRACK_SERVICES).unwrap(), None);
        db.set_list(TRACK_SERVICES, &[]).unwrap();
        assert_eq!(db.list(TRACK_SERVICES).unwrap(), Some(vec![]));
        db.set_list(TRACK_SERVICES, &["bandcamp".into(), "genius".into()])
            .unwrap();
        assert_eq!(
            db.list(TRACK_SERVICES).unwrap(),
            Some(vec!["bandcamp".into(), "genius".into()])
        );
    }

    #[test]
    fn keys_are_unique() {
        let mut keys = vec![
            CLICK_COUNTING.key,
            CLICK_NOTICE_SEEN.key,
            LAST_STATION.0,
            VOLUME.0,
            CONSTANT_VOLUME.key,
            WEB_TITLES.key,
            IDENTIFY_TRACKS.key,
            TRACK_NOTIFICATIONS.key,
            COVER_SOURCES.0,
            TRACK_SERVICES.0,
            CUSTOM_TRACK_SERVICES.0,
            FAVORITE_LISTS.0,
            SHARE_FROM.0,
            MINI_COMPACT.key,
            MINI_POSITION.0,
            UPDATE_CHECK.key,
            UPDATE_SKIPPED.0,
            TITLE_PARSER_VERSION.0,
        ];
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }
}
