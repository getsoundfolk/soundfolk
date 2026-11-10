// SPDX-License-Identifier: MPL-2.0
//! Track notifications: when a new track goes on air while Soundfolk is in the background (its
//! window closed, minimized or behind another one), the desktop shows it. Off by default.
//!
//! A track is the one `journalize_title` adds to the history (`listening.rs`): a station
//! repeating the same title, or sending its own name between tracks, notifies nothing. On Linux,
//! each notification replaces the previous one and is marked transient, so a long listen does not
//! fill the notification list.
//!
//! The notification shows the track's cover, else the station's logo. It waits for them
//! (`covers.rs`), at most [`PICTURE_WAIT`]: a cover already known comes at once, one looked up
//! online after a moment.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, State};

use crate::audio::NowPlaying;
use crate::db::{settings, Database};
use crate::error::AppResult;
use crate::state::with_db;
use crate::tracks::ParsedTitle;

/// A notification waits this long for its picture at most, then goes without it.
const PICTURE_WAIT: Duration = Duration::from_secs(4);

/// Whether the main window has the focus: no notification then, the player shows the track.
/// Followed from the window's events (`lib.rs`), never asked from here: the title arrives on the
/// audio thread, which must not wait for the main thread.
static FOREGROUND: AtomicBool = AtomicBool::new(false);

pub fn set_foreground(focused: bool) {
    FOREGROUND.store(focused, Ordering::Relaxed);
}

pub struct TrackNotifications {
    enabled: AtomicBool,
    /// Address of the app icon, shown with the notification.
    icon: Option<String>,
    /// The app's identifier (`tauri.conf.json`), its identity on Windows.
    identifier: String,
    /// The last notification shown (its id on Linux, to replace it). Also keeps them in order.
    last: Arc<Mutex<Option<u32>>>,
    /// The notification waiting for its picture.
    pending: Arc<Mutex<Option<Pending>>>,
    /// Numbers the notifications, so that a late timer leaves a newer one alone.
    count: Arc<AtomicU64>,
}

struct Pending {
    number: u64,
    /// The title on air (`NowPlaying.title`), as the covers name it.
    title: Option<String>,
    summary: String,
    body: String,
}

impl TrackNotifications {
    pub fn new(app: &AppHandle, db: &Database, icon: Option<String>) -> Self {
        let enabled = db.flag(settings::TRACK_NOTIFICATIONS).unwrap_or_else(|e| {
            tracing::warn!("track notifications setting not read: {e}");
            false
        });
        let identifier = app.config().identifier.clone();
        init(&identifier);
        Self {
            enabled: AtomicBool::new(enabled),
            icon,
            identifier,
            last: Arc::default(),
            pending: Arc::default(),
            count: Arc::default(),
        }
    }

    /// A new track is on air. `station`: the station's own name, which finds its signature in the
    /// title; `label`: the name the user gave it, shown instead.
    pub fn track(&self, now_playing: &NowPlaying, station: Option<&str>, label: Option<&str>) {
        // The mini player shows the title already, over every other window.
        let shown = FOREGROUND.load(Ordering::Relaxed) || crate::mini::is_open();
        if !self.enabled.load(Ordering::Relaxed) || shown {
            return;
        }
        let station = station
            .or(now_playing.station.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(raw) = now_playing
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        let parsed = now_playing.parsed(raw, station);
        let label = label.map(str::trim).filter(|l| !l.is_empty());
        let (summary, body) = text(&parsed, label.or(station));
        let number = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        *self.lock_pending() = Some(Pending {
            number,
            title: now_playing.title.clone(),
            summary,
            body,
        });
        // Without a picture in time, the notification goes without it.
        let (pending, shower) = (self.pending.clone(), self.shower());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(PICTURE_WAIT).await;
            let late = {
                let mut pending = pending.lock().unwrap_or_else(|p| p.into_inner());
                pending.take_if(|p| p.number == number)
            };
            if let Some(late) = late {
                shower.show(late, None);
            }
        });
    }

    /// Whether a notification waits for the picture of this title.
    pub fn waits_for(&self, title: Option<&str>) -> bool {
        self.lock_pending()
            .as_ref()
            .is_some_and(|p| p.title.as_deref() == title)
    }

    /// The picture of this title is known (`None`: it has none): its notification shows.
    pub fn picture(&self, title: Option<&str>, picture: Option<PathBuf>) {
        let pending = self.lock_pending().take_if(|p| p.title.as_deref() == title);
        if let Some(pending) = pending {
            self.shower().show(pending, picture);
        }
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn shower(&self) -> Shower {
        Shower {
            icon: self.icon.clone(),
            identifier: self.identifier.clone(),
            last: self.last.clone(),
        }
    }
}

/// What showing a notification needs, away from [`TrackNotifications`].
struct Shower {
    icon: Option<String>,
    identifier: String,
    last: Arc<Mutex<Option<u32>>>,
}

impl Shower {
    fn show(self, pending: Pending, picture: Option<PathBuf>) {
        // Showing waits for the desktop (D-Bus on Linux): not on the audio thread.
        crate::state::spawn_blocking(move || {
            let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
            let picture = picture.as_deref().and_then(|p| p.to_str());
            let shown = imp::show(
                &pending.summary,
                &pending.body,
                self.icon.as_deref(),
                picture,
                &self.identifier,
                *last,
            );
            match shown {
                Ok(id) => *last = id,
                Err(e) => tracing::warn!("track notification not shown: {e}"),
            }
        });
    }
}

/// The title on the first line; artist and station below, as in the history.
fn text(parsed: &ParsedTitle, station: Option<&str>) -> (String, String) {
    let body = [parsed.artist.as_deref(), station]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    (parsed.title.clone(), body)
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use notify_rust::{Hint, Notification};

    pub fn show(
        summary: &str,
        body: &str,
        icon: Option<&str>,
        picture: Option<&str>,
        _identifier: &str,
        replaces: Option<u32>,
    ) -> Result<Option<u32>, String> {
        let mut notification = Notification::new();
        notification
            .appname("Soundfolk")
            .summary(summary)
            .body(body)
            .hint(Hint::Transient(true));
        if let Some(icon) = icon {
            notification.icon(icon);
        }
        if let Some(picture) = picture {
            notification.image_path(picture);
        }
        if let Some(id) = replaces {
            notification.id(id);
        }
        notification
            .show()
            .map(|shown| Some(shown.id()))
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
mod imp {
    use notify_rust::Notification;

    /// The icon is the app's own there (Windows takes it from the installed shortcut).
    pub fn show(
        summary: &str,
        body: &str,
        _icon: Option<&str>,
        picture: Option<&str>,
        identifier: &str,
        _replaces: Option<u32>,
    ) -> Result<Option<u32>, String> {
        let mut notification = Notification::new();
        notification
            .appname("Soundfolk")
            .summary(summary)
            .body(body);
        if let Some(picture) = picture {
            notification.image_path(picture);
        }
        // Installed, the app has its own identity; a development build borrows PowerShell's
        // (Windows) or the Terminal's (macOS), which every system knows.
        #[cfg(windows)]
        if !tauri::is_dev() {
            notification.app_id(identifier);
        }
        #[cfg(not(windows))]
        let _ = identifier;
        notification.show().map(|_| None).map_err(|e| e.to_string())
    }
}

/// Sets the app the notifications come from on macOS, once, before the first one.
#[cfg(target_os = "macos")]
fn init(identifier: &str) {
    if !tauri::is_dev()
        && let Err(e) = notify_rust::set_application(identifier)
    {
        tracing::warn!("notifications: app not set: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn init(_identifier: &str) {}

#[tauri::command]
#[specta::specta]
pub async fn get_track_notifications(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| Ok(db.flag(settings::TRACK_NOTIFICATIONS)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_track_notifications(
    db: State<'_, Database>,
    notifications: State<'_, TrackNotifications>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| {
        Ok(db.set_flag(settings::TRACK_NOTIFICATIONS, enabled)?)
    })
    .await?;
    notifications.enabled.store(enabled, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(artist: Option<&str>, title: &str) -> ParsedTitle {
        ParsedTitle {
            artist: artist.map(str::to_owned),
            title: title.to_owned(),
        }
    }

    #[test]
    fn title_first_then_artist_and_station() {
        let (summary, body) = text(
            &parsed(Some("Ben E. King"), "Stand By Me"),
            Some("Radio Swiss Jazz"),
        );
        assert_eq!(summary, "Stand By Me");
        assert_eq!(body, "Ben E. King · Radio Swiss Jazz");
    }

    #[test]
    fn missing_parts_leave_no_separator() {
        assert_eq!(text(&parsed(None, "Morning show"), Some("FIP")).1, "FIP");
        assert_eq!(
            text(&parsed(Some("Nina Simone"), "Feeling Good"), None).1,
            "Nina Simone"
        );
        assert_eq!(text(&parsed(None, "Jingle"), None).1, "");
    }
}
