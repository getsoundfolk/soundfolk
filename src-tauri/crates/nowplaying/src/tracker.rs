// SPDX-License-Identifier: MPL-2.0
//! What to show, when, and when to read the page again.
//!
//! The host fetches the page whenever [`Plan::next_fetch`] is due and gives the result to the
//! tracker, which answers with a new [`Plan`]. The request rate is decided here, never by the
//! recipe: a minimum interval, an hourly cap and random jitter always apply.
//!
//! * The title changes when the listener **hears** the new track: at its start plus the audio
//!   latency. Until then the previous title stays (gaps between tracks included).
//! * When the end of the track is known (start of the next one, or start + duration), the next
//!   request is just after it, and the following changes are scheduled ahead without requests.
//! * Otherwise the page is read at a steady rate that slows down while nothing changes.

use std::collections::VecDeque;

use crate::clock::{self, Offset};
use crate::{RawTrack, SignedDuration, Snapshot, Start, Timestamp, Track};

/// Limits and delays. The host may raise the minimums, not lower them (see [`Policy::clamp`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// Minimum time between two requests.
    pub floor: SignedDuration,
    /// Rate when the end of the track is unknown and the recipe gives no hint.
    pub default_every: SignedDuration,
    /// Slowest rate while nothing changes (end unknown).
    pub max_idle: SignedDuration,
    /// Maximum requests per hour.
    pub hourly_cap: usize,
    /// Random delay added to every request, so listeners do not all hit the site together.
    pub jitter: SignedDuration,
    /// Delay after an expected change before checking it.
    pub confirm_delay: SignedDuration,
    /// Longest wait between requests, even when the next change is far away.
    pub max_wait: SignedDuration,
    /// Delay between the broadcast and what the listener hears (network and audio buffers).
    pub latency: SignedDuration,
    /// Clock differences tolerated when resolving local times.
    pub tolerance: SignedDuration,
    /// Longest wait after repeated failures.
    pub max_failure_backoff: SignedDuration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            floor: SignedDuration::from_secs(15),
            default_every: SignedDuration::from_secs(30),
            max_idle: SignedDuration::from_secs(60),
            hourly_cap: 120,
            jitter: SignedDuration::from_secs(5),
            confirm_delay: SignedDuration::from_secs(5),
            max_wait: SignedDuration::from_mins(10),
            latency: SignedDuration::ZERO,
            tolerance: SignedDuration::from_secs(120),
            max_failure_backoff: SignedDuration::from_mins(5),
        }
    }
}

impl Policy {
    /// The policy with its limits brought back to at least the defaults.
    pub fn clamp(self) -> Self {
        let d = Self::default();
        Self {
            floor: self.floor.max(d.floor),
            hourly_cap: self.hourly_cap.min(d.hourly_cap),
            jitter: self.jitter.max(SignedDuration::ZERO),
            latency: self.latency.max(SignedDuration::ZERO),
            ..self
        }
    }
}

/// What to show at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct OnAir {
    pub current: Track,
    pub next: Option<Track>,
    pub show: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scheduled {
    /// When to show it (now or earlier = right away).
    pub at: Timestamp,
    pub on_air: OnAir,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Replaces any previously scheduled change. In time order.
    pub publish: Vec<Scheduled>,
    pub next_fetch: Timestamp,
}

/// Number of upcoming changes scheduled ahead.
const MAX_SCHEDULED: usize = 5;
/// Without an end time, a start older than this is not trusted to validate a learned offset.
const MAX_UNBOUNDED_TRACK: SignedDuration = SignedDuration::from_hours(6);

pub struct Tracker {
    policy: Policy,
    every: SignedDuration,
    offset: Option<Offset>,
    last_key: Option<String>,
    last_fetch: Option<Timestamp>,
    idle_steps: u32,
    overrun_steps: u32,
    failures: u32,
    recent: VecDeque<Timestamp>,
    rng: fastrand::Rng,
}

impl Tracker {
    /// `every`: the recipe's hint. `offset`: a UTC offset learned earlier for this station.
    pub fn new(
        policy: Policy,
        every: Option<SignedDuration>,
        offset: Option<Offset>,
        seed: u64,
    ) -> Self {
        let policy = policy.clamp();
        let every = every.unwrap_or(policy.default_every).max(policy.floor);
        Self {
            policy,
            every,
            offset,
            last_key: None,
            last_fetch: None,
            idle_steps: 0,
            overrun_steps: 0,
            failures: 0,
            recent: VecDeque::new(),
            rng: fastrand::Rng::with_seed(seed),
        }
    }

    /// The station's UTC offset, once known (to remember it for the next session).
    pub fn offset(&self) -> Option<Offset> {
        self.offset
    }

    /// The page was read at `fetched_at`; `server_date` is its HTTP `Date` header.
    pub fn on_snapshot(
        &mut self,
        snapshot: Snapshot,
        fetched_at: Timestamp,
        server_date: Option<Timestamp>,
    ) -> Plan {
        let previous_fetch = self.last_fetch;
        self.note_fetch(fetched_at);
        self.failures = 0;

        let Some(current_raw) = snapshot.current.filter(|t| t.title.is_some()) else {
            // Nothing on air the page will name (talk, ad, ignored jingle): keep the last title.
            self.last_key = None;
            self.overrun_steps = 0;
            let next = self.idle_wait(false);
            return Plan {
                publish: Vec::new(),
                next_fetch: self.limit(fetched_at, fetched_at + next),
            };
        };
        let key = key_of(&current_raw);
        let changed = self.last_key.as_deref() != Some(key.as_str());
        self.last_key = Some(key);

        let seen_change_since = previous_fetch.filter(|_| changed);
        self.resolve_offset(
            &current_raw,
            snapshot.next.first(),
            fetched_at,
            server_date,
            seen_change_since,
        );

        let Some(current) = self.resolve(current_raw, fetched_at, None) else {
            unreachable!("title checked above")
        };
        let mut upcoming: Vec<Track> = Vec::new();
        for raw in snapshot.next.into_iter().take(MAX_SCHEDULED) {
            let previous_end = upcoming.last().unwrap_or(&current).ends_at();
            match self.resolve(raw, fetched_at, previous_end) {
                Some(track) => upcoming.push(track),
                None => break,
            }
        }

        let latency = self.policy.latency;
        let mut publish = vec![Scheduled {
            at: current
                .started_at
                .map_or(fetched_at, |s| (s + latency).max(fetched_at)),
            on_air: OnAir {
                current: current.clone(),
                next: upcoming.first().cloned(),
                show: snapshot.show.clone(),
            },
        }];
        for (i, track) in upcoming.iter().enumerate() {
            let Some(start) = track.started_at else { break };
            let at = start + latency;
            if at <= publish.last().map_or(fetched_at, |p| p.at) {
                continue;
            }
            publish.push(Scheduled {
                at,
                on_air: OnAir {
                    current: track.clone(),
                    next: upcoming.get(i + 1).cloned(),
                    show: snapshot.show.clone(),
                },
            });
        }

        // The moment the listener should hear something else.
        let boundary = upcoming
            .first()
            .and_then(|t| t.started_at)
            .or_else(|| current.ends_at())
            .map(|b| b + latency);
        let wait = match boundary {
            Some(boundary) if boundary > fetched_at => {
                self.overrun_steps = 0;
                self.idle_steps = 0;
                (boundary.duration_since(fetched_at) + self.policy.confirm_delay)
                    .min(self.policy.max_wait)
            }
            Some(_) if !changed => {
                // Should have changed already: the track runs over, or the page is late.
                self.overrun_steps += 1;
                self.policy.floor * (1 << (self.overrun_steps - 1).min(2))
            }
            _ => {
                self.overrun_steps = 0;
                self.idle_wait(changed)
            }
        };
        Plan {
            publish,
            next_fetch: self.limit(fetched_at, fetched_at + wait),
        }
    }

    /// The request at `at` failed: when to try again.
    pub fn on_failure(&mut self, at: Timestamp) -> Timestamp {
        self.note_fetch(at);
        self.failures += 1;
        let backoff = (self.policy.default_every * (1 << (self.failures - 1).min(8)))
            .min(self.policy.max_failure_backoff);
        self.limit(at, at + backoff)
    }

    fn note_fetch(&mut self, at: Timestamp) {
        self.last_fetch = Some(at);
        self.recent.push_back(at);
        while self
            .recent
            .front()
            .is_some_and(|&t| at.duration_since(t) >= SignedDuration::from_hours(1))
        {
            self.recent.pop_front();
        }
    }

    /// Steady rate, slowing down by half each time nothing changed.
    fn idle_wait(&mut self, changed: bool) -> SignedDuration {
        self.idle_steps = if changed { 0 } else { self.idle_steps + 1 };
        let slowest = self.policy.max_idle.max(self.every);
        let mut wait = self.every;
        for _ in 0..self.idle_steps.min(8) {
            wait = (wait * 3 / 2).min(slowest);
        }
        wait
    }

    /// Applies the minimum interval, the hourly cap and the jitter.
    fn limit(&mut self, now: Timestamp, wanted: Timestamp) -> Timestamp {
        let mut at = wanted.max(now + self.policy.floor);
        let cap = self.policy.hourly_cap.max(1);
        if self.recent.len() >= cap {
            let oldest_counted = self.recent[self.recent.len() - cap];
            at = at.max(oldest_counted + SignedDuration::from_hours(1));
        }
        let jitter_ms = self.policy.jitter.as_millis().max(0) as u64;
        at + SignedDuration::from_millis(self.rng.u64(0..=jitter_ms) as i64)
    }

    /// Learns or checks the station's UTC offset from the current track's local start time.
    fn resolve_offset(
        &mut self,
        current: &RawTrack,
        next: Option<&RawTrack>,
        fetched_at: Timestamp,
        server_date: Option<Timestamp>,
        change_seen_since: Option<Timestamp>,
    ) {
        let Some(Start::Local(start)) = current.start else {
            return;
        };
        let tolerance = self.policy.tolerance;
        // How long the track lasts, from the page.
        let length = match next.and_then(|n| n.start) {
            Some(Start::Local(next_start)) => Some(clock::forward_gap(start, next_start)),
            _ => current.duration,
        };

        if let Some(offset) = self.offset {
            let started = clock::to_instant(start, offset, fetched_at);
            let plausible = started <= fetched_at + tolerance
                && match length {
                    Some(length) => started + length >= fetched_at - tolerance,
                    None => fetched_at.duration_since(started) <= MAX_UNBOUNDED_TRACK,
                };
            if plausible {
                return;
            }
            // Daylight saving time, or the offset was wrong: learn it again.
            self.offset = None;
        }

        let mut candidates: Option<Vec<Offset>> = None;
        let mut narrow = |found: Vec<Offset>| {
            candidates = Some(match candidates.take() {
                Some(previous) => previous.into_iter().filter(|o| found.contains(o)).collect(),
                None => found,
            });
        };
        if let Some(length) = length {
            narrow(clock::candidate_offsets(
                start,
                server_date.unwrap_or(fetched_at),
                length,
                tolerance,
            ));
        }
        if let Some(since) = change_seen_since {
            narrow(clock::candidate_offsets(
                start,
                fetched_at,
                fetched_at.duration_since(since),
                tolerance,
            ));
        }
        if let Some([only]) = candidates.as_deref() {
            self.offset = Some(*only);
        }
    }

    /// A track with its start as an instant. `previous_end` stands in for a missing start.
    fn resolve(
        &self,
        raw: RawTrack,
        reference: Timestamp,
        previous_end: Option<Timestamp>,
    ) -> Option<Track> {
        let title = raw.title?;
        let started_at = match raw.start {
            Some(Start::At(at)) => Some(at),
            Some(Start::Local(local)) => {
                self.offset.map(|o| clock::to_instant(local, o, reference))
            }
            None => None,
        }
        .or(previous_end);
        Some(Track {
            artist: raw.artist,
            title,
            started_at,
            duration: raw.duration,
            cover: raw.cover,
        })
    }
}

fn key_of(track: &RawTrack) -> String {
    format!(
        "{}|{}",
        track.artist.as_deref().unwrap_or(""),
        track.title.as_deref().unwrap_or("")
    )
    .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Time;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }
    fn secs(s: i64) -> SignedDuration {
        SignedDuration::from_secs(s)
    }
    fn policy() -> Policy {
        Policy {
            jitter: SignedDuration::ZERO,
            latency: secs(2),
            ..Policy::default()
        }
    }
    fn local(title: &str, h: i8, m: i8, s: i8, duration: Option<i64>) -> RawTrack {
        RawTrack {
            artist: Some("ARTIST".into()),
            title: Some(title.into()),
            start: Some(Start::Local(Time::constant(h, m, s, 0))),
            duration: duration.map(secs),
            cover: None,
        }
    }
    fn plain(title: &str) -> RawTrack {
        RawTrack {
            title: Some(title.into()),
            ..RawTrack::default()
        }
    }

    /// The Fréquence 7 page as seen at 22:39:08 local (CET).
    fn frequence7() -> Snapshot {
        Snapshot {
            current: Some(local(
                "Children Of Fire And Sacrifices",
                22,
                36,
                51,
                Some(276),
            )),
            next: vec![
                local("Nino", 22, 41, 27, Some(103)),
                local("Abduction", 22, 43, 10, Some(106)),
            ],
            show: None,
        }
    }

    #[test]
    fn schedules_changes_from_the_page_times() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let now = ts("2026-11-04T21:39:08Z");
        let plan = tracker.on_snapshot(frequence7(), now, Some(now));

        assert_eq!(tracker.offset(), Some(3600));
        let titles: Vec<_> = plan
            .publish
            .iter()
            .map(|p| (p.at, p.on_air.current.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            [
                (now, "Children Of Fire And Sacrifices"),
                (ts("2026-11-04T21:41:29Z"), "Nino"),
                (ts("2026-11-04T21:43:12Z"), "Abduction"),
            ]
        );
        assert_eq!(
            plan.publish[0].on_air.current.started_at,
            Some(ts("2026-11-04T21:36:51Z"))
        );
        assert_eq!(
            plan.publish[0]
                .on_air
                .next
                .as_ref()
                .map(|t| t.title.as_str()),
            Some("Nino")
        );
        // Checked just after the next track starts (start + latency + 5 s).
        assert_eq!(plan.next_fetch, ts("2026-11-04T21:41:34Z"));
    }

    #[test]
    fn new_track_waits_until_the_listener_hears_it() {
        let mut tracker = Tracker::new(policy(), None, Some(3600), 1);
        // The page switched one second ago; with 2 s of latency the listener still hears the end
        // of the previous track.
        let now = ts("2026-11-04T21:41:28Z");
        let snapshot = Snapshot {
            current: Some(local("Nino", 22, 41, 27, Some(103))),
            ..Snapshot::default()
        };
        let plan = tracker.on_snapshot(snapshot, now, None);
        assert_eq!(plan.publish[0].at, ts("2026-11-04T21:41:29Z"));
    }

    #[test]
    fn backs_off_when_a_track_runs_over() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let first = ts("2026-11-04T21:39:08Z");
        let plan = tracker.on_snapshot(frequence7(), first, Some(first));
        // Same page after the expected change: 15 s, 30 s, 60 s, 60 s.
        let mut at = plan.next_fetch;
        for expected in [15, 30, 60, 60] {
            let plan = tracker.on_snapshot(frequence7(), at, Some(at));
            assert_eq!(plan.next_fetch.duration_since(at), secs(expected));
            at = plan.next_fetch;
        }
    }

    #[test]
    fn slows_down_when_nothing_is_known() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let mut at = ts("2026-11-04T12:00:00Z");
        let mut waits = Vec::new();
        for _ in 0..4 {
            let plan = tracker.on_snapshot(
                Snapshot {
                    current: Some(plain("Same")),
                    ..Snapshot::default()
                },
                at,
                None,
            );
            waits.push(plan.next_fetch.duration_since(at).as_secs());
            at = plan.next_fetch;
        }
        assert_eq!(waits, [30, 45, 60, 60]);
        let plan = tracker.on_snapshot(
            Snapshot {
                current: Some(plain("Other")),
                ..Snapshot::default()
            },
            at,
            None,
        );
        assert_eq!(plan.next_fetch.duration_since(at), secs(30));
    }

    #[test]
    fn recipe_hint_cannot_go_below_the_floor() {
        let mut tracker = Tracker::new(policy(), Some(secs(1)), None, 1);
        let at = ts("2026-11-04T12:00:00Z");
        let plan = tracker.on_snapshot(
            Snapshot {
                current: Some(plain("A")),
                ..Snapshot::default()
            },
            at,
            None,
        );
        assert_eq!(plan.next_fetch.duration_since(at), secs(15));
        // Nor can the host's policy.
        let lax = Policy {
            floor: secs(1),
            hourly_cap: 10_000,
            ..policy()
        };
        let mut tracker = Tracker::new(lax, Some(secs(1)), None, 1);
        let plan = tracker.on_snapshot(
            Snapshot {
                current: Some(plain("A")),
                ..Snapshot::default()
            },
            at,
            None,
        );
        assert_eq!(plan.next_fetch.duration_since(at), secs(15));
    }

    #[test]
    fn hourly_cap_holds() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let start = ts("2026-11-04T12:00:00Z");
        let mut at = start;
        let mut requests = 0;
        while at < start + SignedDuration::from_hours(1) {
            requests += 1;
            // A page that changes every time: the tracker would like to read it every 30 s.
            let snapshot = Snapshot {
                current: Some(plain(&requests.to_string())),
                ..Snapshot::default()
            };
            at = tracker.on_snapshot(snapshot, at, None).next_fetch;
        }
        assert!(requests <= 120, "{requests}");
    }

    #[test]
    fn failures_back_off_up_to_five_minutes() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let mut at = ts("2026-11-04T12:00:00Z");
        let mut waits = Vec::new();
        for _ in 0..6 {
            let next = tracker.on_failure(at);
            waits.push(next.duration_since(at).as_secs());
            at = next;
        }
        assert_eq!(waits, [30, 60, 120, 240, 300, 300]);
    }

    #[test]
    fn learns_the_offset_from_a_change_then_relearns_it() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        // No duration, no next track: the first page says nothing about the time zone.
        let a = Snapshot {
            current: Some(local("A", 22, 30, 0, None)),
            ..Snapshot::default()
        };
        tracker.on_snapshot(a, ts("2026-11-04T21:30:40Z"), None);
        assert_eq!(tracker.offset(), None);
        // B appears between two requests 30 s apart, announced as started at 22:31:00 local.
        let b = Snapshot {
            current: Some(local("B", 22, 31, 0, None)),
            ..Snapshot::default()
        };
        tracker.on_snapshot(b, ts("2026-11-04T21:31:10Z"), None);
        assert_eq!(tracker.offset(), Some(3600));

        // Next spring the station is at +2:00: the stored +1:00 puts the start an hour in the
        // future, so it is dropped and learned again.
        let mut tracker = Tracker::new(policy(), None, Some(3600), 1);
        let summer = Snapshot {
            current: Some(local("C", 22, 0, 0, Some(200))),
            next: vec![local("D", 22, 3, 20, None)],
            show: None,
        };
        tracker.on_snapshot(summer, ts("2027-04-04T20:01:00Z"), None);
        assert_eq!(tracker.offset(), Some(7200));
    }

    #[test]
    fn nothing_on_air_keeps_the_last_title() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let plan = tracker.on_snapshot(Snapshot::default(), ts("2026-11-04T12:00:00Z"), None);
        assert!(plan.publish.is_empty());
    }

    #[test]
    fn absolute_times_need_no_offset() {
        let mut tracker = Tracker::new(policy(), None, None, 1);
        let now = ts("2026-11-04T12:01:00Z");
        let current = RawTrack {
            title: Some("A".into()),
            start: Some(Start::At(ts("2026-11-04T12:00:00Z"))),
            duration: Some(secs(180)),
            ..RawTrack::default()
        };
        let plan = tracker.on_snapshot(
            Snapshot {
                current: Some(current),
                next: vec![plain("B")],
                show: None,
            },
            now,
            None,
        );
        // B has no start: it follows A's end.
        assert_eq!(plan.publish[1].at, ts("2026-11-04T12:03:02Z"));
        assert_eq!(plan.next_fetch, ts("2026-11-04T12:03:07Z"));
    }
}
