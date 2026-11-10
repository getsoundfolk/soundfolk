// SPDX-License-Identifier: MPL-2.0
//! How a title task waits: for a time read on the wall clock, or for a change of what it
//! follows.

use std::time::Duration;

use soundfolk_nowplaying::{SignedDuration, Timestamp};
use tokio::sync::watch;

use crate::audio::{Cancel, Reporter};
use crate::recipes::Recipes;

/// Longest wait for a time read on the wall clock: the timers stop while the computer sleeps, the
/// wall clock does not, so a wait counted before a sleep would end late after it.
pub(super) const CLOCK_CHECK: Duration = Duration::from_secs(5);

/// From `now` to `wake` (nothing if already past), at most [`CLOCK_CHECK`].
pub(super) fn until(wake: Timestamp, now: Timestamp) -> Duration {
    wake.duration_since(now)
        .max(SignedDuration::ZERO)
        .unsigned_abs()
        .min(CLOCK_CHECK)
}

/// What a title task waits for besides time: the playback ending (stop, other station), the
/// stream opening or announcing a title, the recipes changing. Create it before reading what it
/// watches: a change made in between then ends the next wait at once instead of being missed.
pub(super) struct Changes<'a> {
    cancel: &'a Cancel,
    stream: watch::Receiver<()>,
    recipes: watch::Receiver<u64>,
}

impl<'a> Changes<'a> {
    pub(super) fn new(recipes: &Recipes, reporter: &Reporter, cancel: &'a Cancel) -> Self {
        Self {
            cancel,
            stream: reporter.stream_meta().changes(),
            recipes: recipes.changes(),
        }
    }

    /// Returns on the next change, or once `timeout` has passed (`None`: no limit). The caller
    /// checks again what it waits for: a change may concern something else.
    pub(super) async fn next(&mut self, timeout: Option<Duration>) {
        let timeout = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        // Both senders outlive the task (the reporter and the recipes it holds): `changed` does
        // not fail.
        tokio::select! {
            () = self.cancel.cancelled() => {}
            _ = self.stream.changed() => {}
            _ = self.recipes.changed() => {}
            () = timeout => {}
        }
    }
}
