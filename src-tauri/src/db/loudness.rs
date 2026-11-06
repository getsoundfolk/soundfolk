// SPDX-License-Identifier: MPL-2.0
//! Station levels learned by *Constant volume* (`audio::loudness`): one row per station, its
//! loudness rather than a gain, so changing the target never invalidates what was learned. Never
//! leaves the device; cleared with the history.

use rusqlite::{params, OptionalExtension};

use super::Database;
use crate::audio::StoredLevel;

/// Stations to learn before their median is used as the guess for an unknown one.
const MIN_FOR_GUESS: i64 = 20;

impl Database {
    pub fn station_level(&self, station: &str) -> rusqlite::Result<Option<StoredLevel>> {
        self.conn()
            .query_row(
                "SELECT loudness_lufs, peak_dbfs, measured_s FROM station_loudness WHERE station_key = ?1",
                params![station],
                |r| Ok(StoredLevel { loudness_lufs: r.get(0)?, peak_dbfs: r.get(1)?, measured_secs: r.get(2)? }),
            )
            .optional()
    }

    pub fn save_station_level(&self, station: &str, level: &StoredLevel) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO station_loudness (station_key, loudness_lufs, peak_dbfs, measured_s, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())
             ON CONFLICT(station_key) DO UPDATE SET
                 loudness_lufs = excluded.loudness_lufs,
                 peak_dbfs = excluded.peak_dbfs,
                 measured_s = excluded.measured_s,
                 updated_at = excluded.updated_at",
            params![station, level.loudness_lufs, level.peak_dbfs, level.measured_secs],
        )?;
        Ok(())
    }

    /// Median loudness of the stations learned, once there are enough of them.
    pub fn median_station_loudness(&self) -> rusqlite::Result<Option<f64>> {
        let conn = self.conn();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM station_loudness", [], |r| r.get(0))?;
        if count < MIN_FOR_GUESS {
            return Ok(None);
        }
        conn.query_row(
            "SELECT loudness_lufs FROM station_loudness ORDER BY loudness_lufs LIMIT 1 OFFSET ?1",
            params![count / 2],
            |r| r.get(0),
        )
        .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(lufs: f64) -> StoredLevel {
        StoredLevel {
            loudness_lufs: lufs,
            peak_dbfs: -1.5,
            measured_secs: 42.0,
        }
    }

    #[test]
    fn a_level_is_saved_and_updated() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.station_level("a").unwrap(), None);
        db.save_station_level("a", &level(-12.0)).unwrap();
        db.save_station_level("a", &level(-9.0)).unwrap();
        assert_eq!(db.station_level("a").unwrap(), Some(level(-9.0)));
        // Clearing the history forgets the levels too.
        db.clear_history().unwrap();
        assert_eq!(db.station_level("a").unwrap(), None);
    }

    #[test]
    fn the_guess_is_the_median_once_enough_stations_are_known() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..19 {
            db.save_station_level(&i.to_string(), &level(-20.0 + f64::from(i)))
                .unwrap();
        }
        assert_eq!(db.median_station_loudness().unwrap(), None);
        db.save_station_level("19", &level(-1.0)).unwrap();
        // −20 … −2 and −1: the 11th of 20.
        assert_eq!(db.median_station_loudness().unwrap(), Some(-10.0));
    }
}
