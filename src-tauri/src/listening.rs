// SPDX-License-Identifier: MPL-2.0
//! The listening history, written as playback goes: the titles heard (`listening_history`) and
//! when each station was playing (`listening_sessions`).
//!
//! The player calls its listeners from the decoder thread, from the web title tasks and from
//! commands holding the engine lock. None of them may wait on the database, which an export or a
//! statistics query can hold for a while: the listeners only post what happened, with the station
//! on air at that moment, to a `listening` thread that does the database work in order. That
//! thread also announces the title to the window once it is in the history (the ♥ looks it up
//! there), and to the covers and the track notifications.

use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::audio::{NowPlaying, PlayerState};
use crate::covers::Covers;
use crate::db::Database;
use crate::events;
use crate::notifications::TrackNotifications;
use crate::player::OnAir;
use crate::tracks;

/// How often an open listening session records that it is still playing.
const SESSION_HEARTBEAT: Duration = Duration::from_secs(60);

/// What the player reported, with the station on air when it did.
enum Job {
    State(PlayerState, Option<OnAir>),
    Title(Box<NowPlaying>, Option<OnAir>),
    Heartbeat,
}

/// The listeners' side: posts to the `listening` thread, never waits.
#[derive(Clone)]
pub(crate) struct Journal {
    jobs: mpsc::Sender<Job>,
    on_air: Arc<Mutex<Option<OnAir>>>,
}

impl Journal {
    /// Starts the `listening` thread and its heartbeat. The thread ends once every `Journal` is
    /// dropped and the jobs sent before are done.
    pub(crate) fn start(
        app: tauri::AppHandle,
        covers: Arc<Covers>,
        on_air: Arc<Mutex<Option<OnAir>>>,
    ) -> std::io::Result<Self> {
        let (jobs, pending) = mpsc::channel::<Job>();
        let mut writer = Writer {
            app,
            covers,
            heard: Heard::default(),
        };
        thread::Builder::new()
            .name("listening".into())
            .spawn(move || {
                for job in pending {
                    writer.run(job);
                }
            })?;
        let journal = Self { jobs, on_air };
        let heartbeat = journal.jobs.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(SESSION_HEARTBEAT).await;
                if heartbeat.send(Job::Heartbeat).is_err() {
                    break;
                }
            }
        });
        Ok(journal)
    }

    /// A new player state.
    pub(crate) fn state(&self, state: &PlayerState) {
        self.send(Job::State(state.clone(), self.on_air()));
    }

    /// A new title on air (or none).
    pub(crate) fn title(&self, now_playing: NowPlaying) {
        self.send(Job::Title(Box::new(now_playing), self.on_air()));
    }

    fn on_air(&self) -> Option<OnAir> {
        self.on_air
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn send(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            tracing::warn!("listening history not saved: the listening thread is gone");
        }
    }
}

/// The `listening` thread's side.
struct Writer {
    app: tauri::AppHandle,
    covers: Arc<Covers>,
    heard: Heard,
}

impl Writer {
    fn run(&mut self, job: Job) {
        let app = self.app.clone();
        let db = app.state::<Database>();
        match job {
            Job::State(state, on_air) => self.heard.state(&db, on_air.as_ref(), &state),
            Job::Title(now_playing, on_air) => self.title(&db, *now_playing, on_air),
            Job::Heartbeat => {
                if let Err(e) = db.touch_session() {
                    tracing::warn!("listening session heartbeat failed: {e}");
                }
            }
        }
    }

    /// Logs the title, then announces it: notification for a new track, covers, window. While
    /// paused, the title is still shown (the stream goes on live) but neither logged nor notified.
    fn title(&mut self, db: &Database, now_playing: NowPlaying, on_air: Option<OnAir>) {
        let new_track = self.heard.title(db, on_air.as_ref(), &now_playing);
        let name = on_air.as_ref().and_then(|s| s.name.clone());
        let label = on_air.and_then(|s| {
            db.station_name(&s.id).unwrap_or_else(|e| {
                tracing::warn!("station name not read: {e}");
                None
            })
        });
        if new_track {
            let notifications = self.app.state::<TrackNotifications>();
            notifications.track(&now_playing, name.as_deref(), label.as_deref());
        }
        self.covers.update(now_playing.clone(), name, label);
        if let Err(e) = self.app.emit(events::NOW_PLAYING, now_playing) {
            tracing::warn!("failed to emit now-playing title: {e}");
        }
    }
}

/// Adds the title to `listening_history`, without consecutive duplicates: while listening goes on,
/// the same track on the same station several times in a row is one entry (checked here, then by
/// the database, which also catches another spelling of the title). After a stop (`last_logged`
/// cleared, see `track_session`), the same track is heard again and gets its own entry. Some radios
/// send their own name as the title between tracks: that is not a track, so it is skipped.
/// Returns whether a new entry was added: a new track on air.
fn journalize_title(
    db: &Database,
    last_logged: &mut Option<Logged>,
    station: Option<&OnAir>,
    now_playing: &NowPlaying,
) -> bool {
    let Some(title) = now_playing
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    let Some(station) = station else {
        return false;
    };
    let is_station_name =
        |name: Option<&str>| name.is_some_and(|n| n.trim().eq_ignore_ascii_case(title));
    if is_station_name(station.name.as_deref()) || is_station_name(now_playing.station.as_deref()) {
        return false;
    }

    if let Some(logged) = last_logged
        .as_mut()
        .filter(|l| l.station_id == station.id && l.title == title)
    {
        // The same title, now with artist and title apart (a server API confirming the ICY title).
        if let (Some(track), false) = (&now_playing.track, logged.structured) {
            let parsed = tracks::ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            };
            match db.set_track_parts(&station.id, title, &parsed) {
                Ok(()) => logged.structured = true,
                Err(e) => tracing::warn!("listening history not updated: {e}"),
            }
        }
        return false;
    }
    let structured = now_playing.track.is_some();
    let continuing = last_logged.is_some();
    let logged = match &now_playing.track {
        // Artist and title given apart by the source: stored as given, with the real start.
        Some(track) => {
            let parsed = tracks::ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            };
            let played_at = track.started_at.as_deref().and_then(history_time);
            db.log_structured_track(
                &station.id,
                station.name.as_deref(),
                title,
                &parsed,
                played_at.as_deref(),
                continuing,
            )
        }
        None => db.log_track(&station.id, station.name.as_deref(), title, continuing),
    };
    match logged {
        Ok(_) => {
            *last_logged = Some(Logged {
                station_id: station.id.clone(),
                title: title.to_owned(),
                structured,
            });
            true
        }
        Err(e) => {
            tracing::warn!("listening history not saved: {e}");
            false
        }
    }
}

/// What the history knows of the listening going on.
#[derive(Default)]
struct Heard {
    /// The last title added to the history, while listening goes on.
    last_logged: Option<Logged>,
    paused: bool,
    /// The title on air while paused, with its station: the pause mutes a stream that goes on
    /// live, so titles keep coming but are not heard. The one on air when playback resumes is.
    held: Option<(NowPlaying, OnAir)>,
}

impl Heard {
    fn state(&mut self, db: &Database, on_air: Option<&OnAir>, state: &PlayerState) {
        track_session(db, &mut self.last_logged, on_air, state);
        self.paused = match state {
            PlayerState::Paused => true,
            // A reconnection while paused comes back paused.
            PlayerState::Reconnecting(_) => self.paused,
            _ => false,
        };
        match state {
            PlayerState::Paused | PlayerState::Reconnecting(_) => {}
            PlayerState::Playing => {
                if let Some((now_playing, station)) = self.held.take()
                    && on_air.is_some_and(|s| s.id == station.id)
                {
                    journalize_title(db, &mut self.last_logged, Some(&station), &now_playing);
                }
            }
            _ => self.held = None,
        }
    }

    /// Logs the title unless paused. Returns whether it is a new track in the history.
    fn title(&mut self, db: &Database, on_air: Option<&OnAir>, now_playing: &NowPlaying) -> bool {
        if self.paused {
            self.held = on_air.map(|s| (now_playing.clone(), s.clone()));
            return false;
        }
        journalize_title(db, &mut self.last_logged, on_air, now_playing)
    }
}

/// The last title added to the history.
struct Logged {
    station_id: String,
    title: String,
    /// Stored with artist and title as the source gave them.
    structured: bool,
}

/// An RFC 3339 start as stored in the history (`YYYY-MM-DD HH:MM:SS`, UTC), if plausible: not
/// in the future, and less than a day old.
fn history_time(started_at: &str) -> Option<String> {
    let started: jiff::Timestamp = started_at.parse().ok()?;
    let age = jiff::Timestamp::now().duration_since(started);
    (age >= jiff::SignedDuration::ZERO && age < jiff::SignedDuration::from_hours(24))
        .then(|| started.strftime("%Y-%m-%d %H:%M:%S").to_string())
}

/// Opens a listening session when a station starts playing and closes it when playback stops. A
/// stop or an error also ends the run of titles `journalize_title` merges; a pause or a
/// reconnection does not, so a track paused and resumed is one entry.
fn track_session(
    db: &Database,
    last_logged: &mut Option<Logged>,
    station: Option<&OnAir>,
    state: &PlayerState,
) {
    if matches!(state, PlayerState::Idle | PlayerState::Error(_)) {
        *last_logged = None;
    }
    let result = match state {
        PlayerState::Playing => match station {
            Some(s) => db.start_session(&s.id, s.name.as_deref()),
            None => Ok(()),
        },
        // While reconnecting nothing is heard: the session ends when the sound stopped (a sleep
        // is not listening time), and a new one starts when it comes back.
        PlayerState::Idle
        | PlayerState::Paused
        | PlayerState::Reconnecting(_)
        | PlayerState::Error(_) => db.end_session(),
        PlayerState::Connecting => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!("listening session not saved: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: &str, name: &str) -> OnAir {
        OnAir {
            id: id.into(),
            name: Some(name.into()),
            url: format!("https://example.org/{id}"),
        }
    }

    fn title(text: &str) -> NowPlaying {
        NowPlaying {
            title: Some(text.into()),
            ..NowPlaying::default()
        }
    }

    fn logged(db: &Database) -> Vec<String> {
        let mut titles: Vec<String> = db
            .recent_history(100, None)
            .unwrap()
            .into_iter()
            .map(|e| e.track_title)
            .collect();
        titles.reverse();
        titles
    }

    #[test]
    fn a_title_repeated_while_listening_is_one_entry() {
        let db = Database::open_in_memory().unwrap();
        let fip = station("fip", "FIP");
        let mut last = None;
        assert!(journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("A - One")
        ));
        assert!(!journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("A - One")
        ));
        assert!(journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("B - Two")
        ));
        assert_eq!(logged(&db), ["A - One", "B - Two"]);
    }

    #[test]
    fn after_a_stop_the_same_title_is_heard_again() {
        let db = Database::open_in_memory().unwrap();
        let fip = station("fip", "FIP");
        let mut last = None;
        track_session(&db, &mut last, Some(&fip), &PlayerState::Playing);
        assert!(journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("A - One")
        ));
        // A reconnection keeps the run of titles.
        track_session(
            &db,
            &mut last,
            Some(&fip),
            &PlayerState::Reconnecting("x".into()),
        );
        assert!(!journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("A - One")
        ));
        track_session(&db, &mut last, Some(&fip), &PlayerState::Idle);
        assert!(journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title("A - One")
        ));
        assert_eq!(logged(&db), ["A - One", "A - One"]);
    }

    #[test]
    fn titles_while_paused_wait_for_playback() {
        let db = Database::open_in_memory().unwrap();
        let fip = station("fip", "FIP");
        let mut heard = Heard::default();
        heard.state(&db, Some(&fip), &PlayerState::Playing);
        assert!(heard.title(&db, Some(&fip), &title("A - One")));
        heard.state(&db, Some(&fip), &PlayerState::Paused);
        assert!(!heard.title(&db, Some(&fip), &title("B - Two")));
        assert!(!heard.title(&db, Some(&fip), &title("C - Three")));
        heard.state(&db, Some(&fip), &PlayerState::Reconnecting("x".into()));
        heard.state(&db, Some(&fip), &PlayerState::Paused);
        assert_eq!(logged(&db), ["A - One"]);
        // Resumed: the track on air is heard, the ones that went by while paused were not.
        heard.state(&db, Some(&fip), &PlayerState::Playing);
        assert_eq!(logged(&db), ["A - One", "C - Three"]);
        // Paused and resumed within a track: still one entry.
        heard.state(&db, Some(&fip), &PlayerState::Paused);
        heard.state(&db, Some(&fip), &PlayerState::Playing);
        assert!(!heard.title(&db, Some(&fip), &title("C - Three")));
        assert_eq!(logged(&db), ["A - One", "C - Three"]);
    }

    #[test]
    fn a_title_held_in_pause_is_dropped_for_another_station() {
        let db = Database::open_in_memory().unwrap();
        let (fip, nova) = (station("fip", "FIP"), station("nova", "Nova"));
        let mut heard = Heard::default();
        heard.state(&db, Some(&fip), &PlayerState::Playing);
        heard.state(&db, Some(&fip), &PlayerState::Paused);
        heard.title(&db, Some(&fip), &title("B - Two"));
        heard.state(&db, Some(&nova), &PlayerState::Connecting);
        heard.state(&db, Some(&nova), &PlayerState::Playing);
        assert!(logged(&db).is_empty());
    }

    #[test]
    fn the_station_name_and_no_station_are_not_tracks() {
        let db = Database::open_in_memory().unwrap();
        let fip = station("fip", "FIP");
        let mut last = None;
        assert!(!journalize_title(
            &db,
            &mut last,
            Some(&fip),
            &title(" fip ")
        ));
        assert!(!journalize_title(&db, &mut last, Some(&fip), &title("  ")));
        assert!(!journalize_title(&db, &mut last, None, &title("A - One")));
        assert!(logged(&db).is_empty());
    }

    #[test]
    fn source_start_times_are_kept_only_when_plausible() {
        let recent = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(3);
        assert!(history_time(&recent.to_string()).is_some());
        let future = jiff::Timestamp::now() + jiff::SignedDuration::from_mins(3);
        assert_eq!(history_time(&future.to_string()), None);
        let old = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(25);
        assert_eq!(history_time(&old.to_string()), None);
        assert_eq!(history_time("yesterday"), None);
    }
}
