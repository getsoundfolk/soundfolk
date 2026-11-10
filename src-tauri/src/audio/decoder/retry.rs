// SPDX-License-Identifier: MPL-2.0
//! Retry policy: reopens the stream after it dropped, says whose fault a failure is, and gives
//! up on a station that keeps failing with the network up.

use std::time::{Duration, Instant};

use super::super::failure::{self, Failure};
use super::{play_once, stream_client, End, Job};
use crate::http::public_net;

/// Longest wait between two attempts while the network is up.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// The station is given up after failing this long **with the network up** (time offline or
/// asleep does not count).
const GIVE_UP_AFTER: Duration = Duration::from_secs(5 * 60);

const RECONNECTING: &str = "Reconnecting…";
const NO_NETWORK: &str = "No network — waiting…";
const NO_INTERNET: &str = "No Internet access — waiting…";
const SIGN_IN: &str = "Sign in to the network (captive portal) — waiting…";

pub(super) fn run(mut job: Job) {
    if job.public_only
        && let Err(message) = public_net::check(&job.url)
    {
        return job.reporter.failed(message);
    }
    let client = match stream_client(job.public_only) {
        Ok(client) => client,
        Err(message) => return job.reporter.failed(message),
    };

    let mut ever_played = false;
    let mut attempts = 0;
    // Since when the station itself fails (time without network or Internet does not count).
    let mut failing_since: Option<Instant> = None;

    loop {
        let mut played = false;
        let end = play_once(&mut job, client, &mut played);
        ever_played |= played;
        if played {
            attempts = 0;
            failing_since = None;
        }

        let failure = match end {
            End::Cancelled => return,
            End::Failed(failure) => Some(failure),
            End::Interrupted(why) => {
                tracing::info!(url = %job.url, "connection dropped: {why}");
                None
            }
        };
        if job.cancel.is_cancelled() {
            return;
        }
        let interrupted = failure.is_none();
        let message = match failure {
            None => RECONNECTING.to_owned(),
            // Retrying would fail the same way.
            Some(Failure::Internal(message)) => return job.reporter.failed(message),
            Some(failure) => {
                tracing::warn!(url = %job.url, ?failure, "playback failed");
                // Right after the sound stopped, the check is shown; later the last reason stays.
                match blame(&job, ever_played && attempts == 0, &failure) {
                    Blame::Connection(message) => {
                        failing_since = None;
                        message.to_owned()
                    }
                    // Nobody can be blamed: what was seen, and no giving up (it may be the
                    // connection, which is waited for as long as it takes).
                    Blame::Unknown => {
                        failing_since = None;
                        format!(
                            "Cannot play the station ({}) — retrying…",
                            failure.symptom()
                        )
                    }
                    Blame::Station => {
                        let reason = failure.reason();
                        // Never played: dead URL, not audio… the user must be told right away.
                        if !ever_played {
                            return job
                                .reporter
                                .failed(format!("The station is not broadcasting: {reason}"));
                        }
                        if failing_since.get_or_insert_with(Instant::now).elapsed() > GIVE_UP_AFTER
                        {
                            return job
                                .reporter
                                .failed(format!("The station stopped broadcasting: {reason}"));
                        }
                        format!("The station is not broadcasting ({reason}) — retrying…")
                    }
                }
            }
        };

        attempts += 1;
        // A connection dropped on purpose is reopened at once; then exponential backoff
        // (1 s, 2 s, 4 s… capped), cut short when the network comes back or changes.
        let delay = match (interrupted, attempts) {
            (true, 1) => Duration::ZERO,
            _ => Duration::from_secs(1u64 << (attempts - 1).min(5)).min(MAX_BACKOFF),
        };
        if !wait_before_retry(&job, delay, &message) {
            return;
        }
    }
}

/// Whose fault a failure is, on proof only (see [`failure::diagnose`]).
enum Blame {
    /// No network, no Internet access, or a captive portal: waited for as long as it takes.
    Connection(&'static str),
    /// Internet works: the station is at fault.
    Station,
    /// Cannot be told: nobody is named.
    Unknown,
}

fn blame(job: &Job, sound_just_stopped: bool, failure: &Failure) -> Blame {
    if !job.net.borrow().online {
        return Blame::Connection(NO_NETWORK);
    }
    if sound_just_stopped {
        // The check takes a moment: the player must not look like it still plays.
        job.reporter.reconnecting(RECONNECTING);
    }
    match failure::diagnose(failure, &job.runtime, &job.cancel) {
        failure::Verdict::Station => Blame::Station,
        failure::Verdict::NoInternet => Blame::Connection(NO_INTERNET),
        failure::Verdict::SignIn => Blame::Connection(SIGN_IN),
        failure::Verdict::Unknown => Blame::Unknown,
    }
}

/// Waits `delay` showing `message`, or for the network if it is down; returns at once when the
/// network comes back or changes. `false` if the playback was stopped meanwhile.
fn wait_before_retry(job: &Job, delay: Duration, message: &str) -> bool {
    let mut net = job.net.clone();
    net.mark_unchanged();
    let deadline = tokio::time::Instant::now() + delay;
    // Without network news (the watch thread is gone), only the delay and the stop are waited for.
    let mut watched = true;
    loop {
        if job.cancel.is_cancelled() {
            return false;
        }
        let state = *net.borrow_and_update();
        job.reporter
            .reconnecting(if state.online { message } else { NO_NETWORK });
        if state.online && tokio::time::Instant::now() >= deadline {
            return true;
        }
        // Offline, the delay does not count: only the network coming back ends the wait.
        let changed = job.runtime.block_on(async {
            tokio::select! {
                () = job.cancel.cancelled() => false,
                news = net.changed(), if watched => {
                    watched = news.is_ok();
                    watched
                }
                () = tokio::time::sleep_until(deadline), if state.online => false,
            }
        });
        if changed && net.borrow().online {
            tracing::info!("network back or changed: reconnecting now");
            return !job.cancel.is_cancelled();
        }
    }
}
