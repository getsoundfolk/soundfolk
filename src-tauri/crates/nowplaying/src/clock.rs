// SPDX-License-Identifier: MPL-2.0
//! Turning a station's local `HH:MM:SS` into instants, without knowing its time zone.
//!
//! Every real UTC offset is a multiple of 15 minutes. A track shown as "on air" started at its
//! local start time, at most `window` before a known instant (the page's HTTP `Date`, or when a
//! change was seen). Only the offsets that put the start inside that window are possible: with a
//! window under 15 minutes, one is left.
//!
//! Example: `Date` 21:39:08 UTC, current track from 22:36:51 to 22:41:27 (local). The offset is
//! between +57:43 and +1:02:19, so +1:00 (CET).

use jiff::{civil::Time, SignedDuration, Timestamp};

/// Granularity of real UTC offsets.
const STEP_SECS: i64 = 15 * 60;
/// Real offsets go from UTC-12:00 to UTC+14:00, but two offsets 24 hours apart (Samoa at +13:00,
/// Niue at -11:00) show the same time of day and give the same instants: only one of each pair is
/// tried, in (-12:00, +12:00].
const MIN_OFFSET_SECS: i64 = -12 * 3600 + STEP_SECS;
const MAX_OFFSET_SECS: i64 = 12 * 3600;
const DAY_SECS: i64 = 86_400;

/// A UTC offset in seconds (local = UTC + offset).
pub type Offset = i32;

fn seconds_of_day(time: Time) -> i64 {
    i64::from(time.hour()) * 3600 + i64::from(time.minute()) * 60 + i64::from(time.second())
}

/// The offsets for which `start` (local) is between `window + tolerance` before `reference` and
/// `tolerance` after it (clocks are never perfectly in sync).
pub fn candidate_offsets(
    start: Time,
    reference: Timestamp,
    window: SignedDuration,
    tolerance: SignedDuration,
) -> Vec<Offset> {
    let reference = reference.as_second();
    let start = seconds_of_day(start);
    let (window, tolerance) = (window.as_secs().max(0), tolerance.as_secs().max(0));
    (MIN_OFFSET_SECS / STEP_SECS..=MAX_OFFSET_SECS / STEP_SECS)
        .map(|k| k * STEP_SECS)
        .filter(|offset| {
            let local_now = (reference + offset).rem_euclid(DAY_SECS);
            let elapsed = (local_now - start).rem_euclid(DAY_SECS);
            elapsed <= window + tolerance || DAY_SECS - elapsed <= tolerance
        })
        .map(|offset| offset as Offset)
        .collect()
}

/// The instant of `local` with this offset, on the day that puts it closest to `reference`.
pub fn to_instant(local: Time, offset: Offset, reference: Timestamp) -> Timestamp {
    let reference = reference.as_second();
    let local_now = (reference + i64::from(offset)).rem_euclid(DAY_SECS);
    let mut delta = (seconds_of_day(local) - local_now).rem_euclid(DAY_SECS);
    if delta > DAY_SECS / 2 {
        delta -= DAY_SECS;
    }
    Timestamp::from_second(reference + delta).expect("within range")
}

/// Time from `a` to `b` on a 24-hour clock (`23:58` → `00:03` is 5 minutes).
pub fn forward_gap(a: Time, b: Time) -> SignedDuration {
    SignedDuration::from_secs((seconds_of_day(b) - seconds_of_day(a)).rem_euclid(DAY_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }
    const TOLERANCE: SignedDuration = SignedDuration::from_secs(120);

    #[test]
    fn finds_cet_from_the_frequence7_page() {
        // Page served at 21:39:08 UTC; current track 22:36:51 → 22:41:27 local.
        let start = Time::constant(22, 36, 51, 0);
        let window = forward_gap(start, Time::constant(22, 41, 27, 0));
        let found = candidate_offsets(start, ts("2026-11-04T21:39:08Z"), window, TOLERANCE);
        assert_eq!(found, vec![3600]);
    }

    #[test]
    fn long_windows_are_ambiguous() {
        let start = Time::constant(22, 0, 0, 0);
        let found = candidate_offsets(start, ts("2026-11-04T21:39:08Z"), SignedDuration::from_hours(1), TOLERANCE);
        assert!(found.len() > 1, "{found:?}");
    }

    #[test]
    fn works_across_midnight_and_far_offsets() {
        // 23:58 local in Auckland (+13:00 in summer) when it is 10:59 UTC: +13:00 and -11:00 show
        // the same time of day, and both give 10:58 UTC.
        let start = Time::constant(23, 58, 0, 0);
        let now = ts("2026-11-04T10:59:00Z");
        let found = candidate_offsets(start, now, SignedDuration::from_mins(4), TOLERANCE);
        assert_eq!(found, vec![-11 * 3600]);
        assert_eq!(to_instant(start, found[0], now), ts("2026-11-04T10:58:00Z"));
        // Nepal is +05:45.
        let start = Time::constant(3, 30, 0, 0);
        let found = candidate_offsets(start, ts("2026-11-04T21:46:00Z"), SignedDuration::from_mins(3), TOLERANCE);
        assert_eq!(found, vec![5 * 3600 + 45 * 60]);
    }

    #[test]
    fn instants_land_on_the_nearest_day() {
        let reference = ts("2026-11-04T23:10:00Z");
        // 00:05 local at +1:00 is 23:05 UTC the same evening, not 24 hours later.
        assert_eq!(to_instant(Time::constant(0, 5, 0, 0), 3600, reference), ts("2026-11-04T23:05:00Z"));
        assert_eq!(to_instant(Time::constant(0, 25, 0, 0), 3600, reference), ts("2026-11-04T23:25:00Z"));
        assert_eq!(forward_gap(Time::constant(23, 58, 0, 0), Time::constant(0, 3, 0, 0)), SignedDuration::from_mins(5));
    }
}
