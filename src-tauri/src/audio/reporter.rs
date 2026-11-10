// SPDX-License-Identifier: MPL-2.0
//! What a playback session tells the rest of the app: its state and title (through the engine's
//! [`Status`]), and what the decoder learned about the stream, for the web title task.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Cancel, Controls, NowPlaying, PlayerState, Status, TitleSource, TrackInfo};

/// Publishes state on behalf of a session. A cancelled session can no longer publish anything,
/// so an old decoder thread cannot overwrite the state of the next session.
#[derive(Clone)]
pub(crate) struct Reporter {
    pub(super) status: Arc<Status>,
    pub(super) controls: Arc<Controls>,
    pub(super) cancel: Arc<Cancel>,
    pub(super) stream: Arc<StreamMeta>,
}

/// What the decoder learned about the stream, for the web title task.
pub(crate) struct StreamMeta {
    pub(crate) created: Instant,
    /// The stream announced a title itself (ICY or tags): web sources stand down.
    inband_title: AtomicBool,
    /// The last ICY `StreamTitle`, for the details a server API gives about it.
    icy_title: Mutex<Option<String>>,
    /// Set once the stream is open.
    opened: Mutex<Option<OpenedInfo>>,
    /// Bumped each time one of the above changes, for the web title task that waits for it.
    changes: tokio::sync::watch::Sender<()>,
}

impl Default for StreamMeta {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            inband_title: AtomicBool::new(false),
            icy_title: Mutex::new(None),
            opened: Mutex::new(None),
            changes: tokio::sync::watch::Sender::new(()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenedInfo {
    pub at: Instant,
    /// The stream carries metadata (ICY `icy-metaint`, or an Ogg container).
    pub inband: bool,
    /// HTTP `Server` header.
    pub server: Option<String>,
}

impl StreamMeta {
    pub(crate) fn has_inband_title(&self) -> bool {
        self.inband_title.load(Ordering::SeqCst)
    }

    pub(crate) fn icy_title(&self) -> Option<String> {
        self.icy_title
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(crate) fn opened(&self) -> Option<OpenedInfo> {
        self.opened
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Sees each change made after this call: subscribe before reading what it guards, so a
    /// change between the reading and the waiting is not lost.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_replace(());
    }
}

impl Reporter {
    fn publish(&self, new: PlayerState) {
        self.publish_with(|| new);
    }

    /// `new` runs under the state lock, so it reads the pause flag as `pause` and `resume`
    /// leave it.
    fn publish_with(&self, new: impl FnOnce() -> PlayerState) {
        let mut state = self.status.lock();
        if self.cancel.is_cancelled() {
            return;
        }
        self.status.replace(&mut state, new());
    }

    pub(crate) fn reconnecting(&self, message: &str) {
        self.publish(PlayerState::Reconnecting(message.to_owned()));
    }

    pub(crate) fn audible(&self) {
        self.publish_with(|| {
            if self.controls.is_paused() {
                PlayerState::Paused
            } else {
                PlayerState::Playing
            }
        });
    }

    /// The playback is given up: the error is the session's last word. Nothing it still reports
    /// (a web title, typically) is published after it, and the engine closes the sound output.
    pub(crate) fn failed(&self, message: String) {
        self.publish(PlayerState::Error(message));
        self.cancel.cancel();
    }

    pub(crate) fn station(&self, name: Option<String>) {
        self.update_now_playing(|np| np.station = name);
    }

    /// ICY `StreamTitle`.
    pub(crate) fn title(&self, title: String) {
        self.stream.inband_title.store(true, Ordering::SeqCst);
        *self
            .stream
            .icy_title
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(title.clone());
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(title),
                source: Some(TitleSource::Icy),
                ..Default::default()
            };
        });
        self.stream.changed();
    }

    /// Artist and title from the stream's own tags (Vorbis comments).
    pub(crate) fn stream_tags(&self, artist: Option<String>, title: String) {
        self.stream.inband_title.store(true, Ordering::SeqCst);
        let track = TrackInfo {
            artist,
            title,
            started_at: None,
            duration_secs: None,
            cover: None,
        };
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(track.display()),
                track: Some(track),
                source: Some(TitleSource::StreamTags),
                ..Default::default()
            };
        });
        self.stream.changed();
    }

    /// What a web source says is on air. Ignored once the stream announced a title itself.
    pub(crate) fn web_title(
        &self,
        on_air: &soundfolk_nowplaying::tracker::OnAir,
        source: TitleSource,
    ) {
        if self.stream.has_inband_title() {
            return;
        }
        let track = TrackInfo::from(&on_air.current);
        self.update_now_playing(|np| {
            *np = NowPlaying {
                station: np.station.take(),
                title: Some(track.display()),
                track: Some(track),
                next: on_air.next.as_ref().map(TrackInfo::from),
                show: on_air.show.clone(),
                source: Some(source),
            };
        });
    }

    /// Details a server API gives about the ICY title `heard` (artist and title apart, start,
    /// length, cover, next track, show). The title shown stays the stream's; ignored once the
    /// stream moved on to another title.
    pub(crate) fn icy_details(
        &self,
        heard: &str,
        track: TrackInfo,
        next: Option<TrackInfo>,
        show: Option<String>,
    ) {
        self.update_now_playing(|np| {
            if np.source == Some(TitleSource::Icy) && np.title.as_deref() == Some(heard) {
                np.track = Some(track);
                np.next = next;
                np.show = show;
            }
        });
    }

    /// The stream is open: whether it carries metadata, and which server sends it.
    pub(crate) fn stream_opened(&self, inband: bool, server: Option<String>) {
        *self.stream.opened.lock().unwrap_or_else(|p| p.into_inner()) = Some(OpenedInfo {
            at: Instant::now(),
            inband,
            server,
        });
        self.stream.changed();
    }

    pub(crate) fn stream_meta(&self) -> &StreamMeta {
        &self.stream
    }

    fn update_now_playing(&self, change: impl FnOnce(&mut NowPlaying)) {
        let mut current = self.status.now_playing();
        if self.cancel.is_cancelled() {
            return;
        }
        let mut next = current.clone();
        change(&mut next);
        if next != *current {
            *current = next.clone();
            (self.status.now_playing_listener)(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_reporter_cannot_publish() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(move |s| sink.lock().unwrap().push(s)),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(|_| {}),
        });
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status,
            controls: Arc::new(Controls::new()),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };

        reporter.reconnecting("Reconnecting…");
        reporter.reconnecting("Reconnecting…"); // repeated while waiting: published once
        cancel.cancel();
        reporter.audible();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![PlayerState::Reconnecting("Reconnecting…".into())]
        );
    }

    /// A playback given up says nothing more: a web title still on its way is not published.
    #[test]
    fn a_failed_session_publishes_nothing_after_its_error() {
        let states = Arc::new(Mutex::new(Vec::new()));
        let titles = Arc::new(Mutex::new(Vec::new()));
        let (state_sink, title_sink) = (states.clone(), titles.clone());
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(move |s| state_sink.lock().unwrap().push(s)),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(move |np| title_sink.lock().unwrap().push(np)),
        });
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status,
            controls: Arc::new(Controls::new()),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };
        let on_air = |artist: Option<&str>, title: &str| soundfolk_nowplaying::tracker::OnAir {
            current: soundfolk_nowplaying::Track {
                artist: artist.map(str::to_owned),
                title: title.to_owned(),
                started_at: None,
                duration: None,
                cover: None,
            },
            next: None,
            show: None,
        };

        reporter.web_title(&on_air(Some("Air"), "Sexy Boy"), TitleSource::Recipe);
        reporter.failed("The station stopped broadcasting: HTTP 404".into());
        assert!(cancel.is_cancelled(), "the session goes on after its error");
        reporter.web_title(&on_air(None, "Nobody heard this"), TitleSource::Recipe);
        reporter.audible();
        assert_eq!(titles.lock().unwrap().len(), 1);
        assert_eq!(
            *states.lock().unwrap(),
            [PlayerState::Error(
                "The station stopped broadcasting: HTTP 404".into()
            )]
        );
    }

    #[test]
    fn now_playing_is_deduplicated_and_gated_by_cancel() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(|_| {}),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(move |np| sink.lock().unwrap().push(np)),
        });
        let cancel = Arc::new(Cancel::default());
        let reporter = Reporter {
            status: status.clone(),
            controls: Arc::new(Controls::new()),
            cancel: cancel.clone(),
            stream: Arc::new(StreamMeta::default()),
        };

        reporter.station(Some("Radio".into()));
        reporter.title("A - B".into());
        reporter.title("A - B".into()); // duplicate (e.g. after a reconnect): ignored
        cancel.cancel();
        reporter.title("late".into()); // cancelled session: ignored

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[1],
            NowPlaying {
                station: Some("Radio".into()),
                title: Some("A - B".into()),
                source: Some(TitleSource::Icy),
                ..Default::default()
            }
        );
        assert!(reporter.stream_meta().has_inband_title());
    }

    #[test]
    fn what_the_stream_learns_is_signalled() {
        let reporter = Reporter {
            status: Arc::new(Status {
                state: Mutex::new(PlayerState::Idle),
                listener: Arc::new(|_| {}),
                now_playing: Mutex::new(NowPlaying::default()),
                now_playing_listener: Arc::new(|_| {}),
            }),
            controls: Arc::new(Controls::new()),
            cancel: Arc::new(Cancel::default()),
            stream: Arc::new(StreamMeta::default()),
        };
        let mut changes = reporter.stream_meta().changes();
        assert!(!changes.has_changed().unwrap());
        reporter.stream_opened(true, None);
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        reporter.title("A - B".into());
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        reporter.stream_tags(None, "C".into());
        assert!(changes.has_changed().unwrap());
    }

    #[test]
    fn icy_details_complete_the_title_heard_only() {
        let status = Arc::new(Status {
            state: Mutex::new(PlayerState::Idle),
            listener: Arc::new(|_| {}),
            now_playing: Mutex::new(NowPlaying::default()),
            now_playing_listener: Arc::new(|_| {}),
        });
        let reporter = Reporter {
            status: status.clone(),
            controls: Arc::new(Controls::new()),
            cancel: Arc::new(Cancel::default()),
            stream: Arc::new(StreamMeta::default()),
        };
        let track = |artist: &str, title: &str| TrackInfo {
            artist: Some(artist.into()),
            title: title.into(),
            started_at: None,
            duration_secs: Some(245),
            cover: None,
        };
        reporter.title("082 - Dru Down - Pimp of the Year".into());
        assert_eq!(
            reporter.stream_meta().icy_title().as_deref(),
            Some("082 - Dru Down - Pimp of the Year")
        );
        reporter.icy_details(
            "082 - Dru Down - Pimp of the Year",
            track("Dru Down", "Pimp of the Year"),
            Some(track("Kurupt", "We Can Freak It")),
            None,
        );
        let np = status.now_playing().clone();
        assert_eq!(
            np.title.as_deref(),
            Some("082 - Dru Down - Pimp of the Year")
        ); // the stream's
        assert_eq!(np.track.unwrap().artist.as_deref(), Some("Dru Down"));
        assert_eq!(np.next.unwrap().title, "We Can Freak It");
        assert_eq!(np.source, Some(TitleSource::Icy));

        // Details arriving after the stream moved on are dropped.
        reporter.title("Air - Sexy Boy".into());
        reporter.icy_details(
            "082 - Dru Down - Pimp of the Year",
            track("Dru Down", "Pimp of the Year"),
            None,
            None,
        );
        let np = status.now_playing().clone();
        assert_eq!(
            (np.title.as_deref(), np.track),
            (Some("Air - Sexy Boy"), None)
        );
    }
}
