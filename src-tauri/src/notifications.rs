// SPDX-License-Identifier: MPL-2.0
//! Track notifications: when a new track goes on air while Soundfolk is in the background (its
//! window closed, minimized or behind another one), the desktop shows it. Off by default.
//!
//! A track is the one `journalize_title` adds to the history (`lib.rs`): a station repeating the
//! same title, or sending its own name between tracks, notifies nothing. On Linux, each
//! notification replaces the previous one and is marked transient, so a long listen does not fill
//! the notification list.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State};

use crate::audio::NowPlaying;
use crate::commands::with_db;
use crate::db::Database;
use crate::error::AppResult;
use crate::tracks::{self, ParsedTitle};

const SETTING_TRACK_NOTIFICATIONS: &str = "track_notifications";

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
}

impl TrackNotifications {
    pub fn new(app: &AppHandle, db: &Database, icon: Option<String>) -> Self {
        let enabled = db
            .get_bool(SETTING_TRACK_NOTIFICATIONS, false)
            .unwrap_or_else(|e| {
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
        }
    }

    /// A new track is on air (`station`: the name the user knows it by).
    pub fn track(&self, now_playing: &NowPlaying, station: Option<&str>) {
        if !self.enabled.load(Ordering::Relaxed) || FOREGROUND.load(Ordering::Relaxed) {
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
        let parsed = match &now_playing.track {
            Some(track) => ParsedTitle {
                artist: track.artist.clone(),
                title: track.title.clone(),
            },
            None => tracks::parse(raw, station),
        };
        let (summary, body) = text(&parsed, station);
        let (icon, identifier, last) = (
            self.icon.clone(),
            self.identifier.clone(),
            self.last.clone(),
        );
        // Showing waits for the desktop (D-Bus on Linux): not on the audio thread.
        tauri::async_runtime::spawn_blocking(move || {
            let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
            match imp::show(&summary, &body, icon.as_deref(), &identifier, *last) {
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
        identifier: &str,
        _replaces: Option<u32>,
    ) -> Result<Option<u32>, String> {
        let mut notification = Notification::new();
        notification
            .appname("Soundfolk")
            .summary(summary)
            .body(body);
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
    if !tauri::is_dev() {
        if let Err(e) = notify_rust::set_application(identifier) {
            tracing::warn!("notifications: app not set: {e}");
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn init(_identifier: &str) {}

#[tauri::command]
pub async fn get_track_notifications(db: State<'_, Database>) -> AppResult<bool> {
    with_db(&db, |db| {
        Ok(db.get_bool(SETTING_TRACK_NOTIFICATIONS, false)?)
    })
    .await
}

#[tauri::command]
pub async fn set_track_notifications(
    db: State<'_, Database>,
    notifications: State<'_, TrackNotifications>,
    enabled: bool,
) -> AppResult<()> {
    with_db(&db, move |db| {
        Ok(db.set_bool(SETTING_TRACK_NOTIFICATIONS, enabled)?)
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
