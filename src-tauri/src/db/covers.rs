// SPDX-License-Identifier: MPL-2.0
//! Cache of the cover art looked up online, so a track that comes back on air is not looked up again.

use rusqlite::{params, OptionalExtension};

use super::Database;

/// A track without cover art is looked up again after this long (the archive keeps growing).
const RETRY_NOT_FOUND: &str = "-30 days";

impl Database {
    /// `Some(url)` when a lookup is still valid (`url` is `None` when nothing was found), `None`
    /// when the track must be looked up.
    pub fn cached_cover(&self, track_key: &str) -> rusqlite::Result<Option<Option<String>>> {
        self.conn()
            .query_row(
                "SELECT url FROM covers
                 WHERE track_key = ?1 AND (url IS NOT NULL OR looked_up_at >= datetime('now', ?2))",
                params![track_key, RETRY_NOT_FOUND],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn save_cover(&self, track_key: &str, url: Option<&str>) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO covers (track_key, url, looked_up_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(track_key) DO UPDATE SET url = excluded.url, looked_up_at = excluded.looked_up_at",
            params![track_key, url],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_cached_and_misses_expire() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.cached_cover("air|sexy boy").unwrap(), None);
        db.save_cover("air|sexy boy", Some("https://x/1.jpg")).unwrap();
        assert_eq!(db.cached_cover("air|sexy boy").unwrap(), Some(Some("https://x/1.jpg".into())));
        db.save_cover("x|y", None).unwrap();
        assert_eq!(db.cached_cover("x|y").unwrap(), Some(None));
        db.conn().execute("UPDATE covers SET looked_up_at = datetime('now', '-31 days')", []).unwrap();
        assert_eq!(db.cached_cover("x|y").unwrap(), None);
        assert_eq!(db.cached_cover("air|sexy boy").unwrap(), Some(Some("https://x/1.jpg".into())));
    }
}
