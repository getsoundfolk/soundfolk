// SPDX-License-Identifier: MPL-2.0
//! Listening sessions: when each station was actually playing, for listening time and like rates.

use rusqlite::params;

use super::Database;

/// When a session ends: now, but no later than shortly after its last heartbeat (sent every
/// minute while playing). After a sleep, the time asleep is not listening time.
const ENDED_AT: &str = "MIN(CURRENT_TIMESTAMP, datetime(last_seen_at, '+90 seconds'))";

impl Database {
    /// A station started playing. A session already open on the same station continues (a
    /// reconnection is not a new listen); one open on another station is closed first.
    pub fn start_session(&self, station_id: &str, station_name: Option<&str>) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            &format!(
                "UPDATE listening_sessions SET ended_at = {ENDED_AT}, last_seen_at = {ENDED_AT}
                 WHERE ended_at IS NULL AND station_id <> ?1"
            ),
            params![station_id],
        )?;
        tx.execute(
            "INSERT INTO listening_sessions (station_id, station_name)
             SELECT ?1, ?2
             WHERE NOT EXISTS (SELECT 1 FROM listening_sessions WHERE ended_at IS NULL)",
            params![station_id, station_name],
        )?;
        tx.commit()
    }

    /// Playback stopped (stop, error, app exit).
    pub fn end_session(&self) -> rusqlite::Result<()> {
        self.conn().execute(
            &format!(
                "UPDATE listening_sessions SET ended_at = {ENDED_AT}, last_seen_at = {ENDED_AT} WHERE ended_at IS NULL"
            ),
            [],
        )?;
        Ok(())
    }

    /// Heartbeat while playing: after a crash, the session ends at its last heartbeat.
    pub fn touch_session(&self) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE listening_sessions SET last_seen_at = CURRENT_TIMESTAMP WHERE ended_at IS NULL",
            [],
        )?;
        Ok(())
    }

    /// At startup: a session still open was cut by a crash or a kill; it ends at its last heartbeat.
    pub(super) fn close_abandoned_sessions(&self) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE listening_sessions SET ended_at = last_seen_at WHERE ended_at IS NULL",
            [],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions(db: &Database) -> Vec<(String, bool)> {
        let conn = db.conn();
        let mut stmt =
            conn.prepare("SELECT station_id, ended_at IS NOT NULL FROM listening_sessions ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn sessions_follow_playback() {
        let db = Database::open_in_memory().unwrap();
        db.start_session("a", Some("A")).unwrap();
        db.start_session("a", Some("A")).unwrap(); // reconnection: same session
        db.touch_session().unwrap();
        db.start_session("b", Some("B")).unwrap(); // station change
        assert_eq!(sessions(&db), [("a".into(), true), ("b".into(), false)]);
        db.end_session().unwrap();
        db.end_session().unwrap(); // nothing open: no-op
        assert_eq!(sessions(&db), [("a".into(), true), ("b".into(), true)]);
    }

    #[test]
    fn a_session_ended_after_a_sleep_ends_near_its_last_heartbeat() {
        let db = Database::open_in_memory().unwrap();
        db.start_session("a", None).unwrap();
        db.conn()
            .execute_batch("UPDATE listening_sessions SET last_seen_at = '2026-01-01 10:30:00'")
            .unwrap();
        db.end_session().unwrap();
        let ended: String = db.conn().query_row("SELECT ended_at FROM listening_sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(ended, "2026-01-01 10:31:30");
    }

    #[test]
    fn abandoned_sessions_end_at_the_last_heartbeat() {
        let db = Database::open_in_memory().unwrap();
        db.start_session("a", None).unwrap();
        db.conn()
            .execute_batch("UPDATE listening_sessions SET last_seen_at = '2026-01-01 10:30:00'")
            .unwrap();
        db.close_abandoned_sessions().unwrap();
        let ended: String =
            db.conn().query_row("SELECT ended_at FROM listening_sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(ended, "2026-01-01 10:30:00");
    }
}
