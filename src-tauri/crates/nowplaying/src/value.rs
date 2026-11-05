// SPDX-License-Identifier: MPL-2.0
//! Reading the values sources give: times, durations, text.

use jiff::{civil, SignedDuration, Timestamp};

use crate::Start;

/// Collapses whitespace; `None` when nothing is left.
pub(crate) fn clean_text(text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// A start time: Unix timestamp (seconds or milliseconds), ISO 8601 / RFC 3339 instant,
/// or a local time of day (`22:36`, `22:36:51`, `2026-11-04 22:36:51`).
pub(crate) fn parse_start(text: &str) -> Option<Start> {
    let text = text.trim();
    if let Some(ts) = parse_unix(text) {
        return Some(Start::At(ts));
    }
    if let Ok(ts) = text.parse::<Timestamp>() {
        return Some(Start::At(ts));
    }
    if let Ok(dt) = text.parse::<civil::DateTime>() {
        return Some(Start::Local(dt.time()));
    }
    parse_time_of_day(text).map(Start::Local)
}

/// A start given as a JSON number: Unix seconds, or milliseconds when too large for seconds.
pub(crate) fn start_from_number(n: f64) -> Option<Start> {
    unix(n).map(Start::At)
}

fn parse_unix(text: &str) -> Option<Timestamp> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    unix(text.parse().ok()?)
}

fn unix(n: f64) -> Option<Timestamp> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    // Seconds stay below 1e11 until the year 5138: anything larger is milliseconds.
    let ms = if n >= 1e11 { n } else { n * 1000.0 };
    Timestamp::from_millisecond(ms as i64).ok()
}

/// `HH:MM` or `HH:MM:SS` (also with `h` as in `22h36`).
fn parse_time_of_day(text: &str) -> Option<civil::Time> {
    let mut parts = text.split([':', 'h', 'H']);
    let hour: i8 = parts.next()?.trim().parse().ok()?;
    let minute: i8 = parts.next()?.trim().parse().ok()?;
    let second: i8 = match parts.next() {
        Some(s) if !s.trim().is_empty() => s.trim().parse().ok()?,
        _ => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    civil::Time::new(hour, minute, second, 0).ok()
}

/// A duration: seconds (`276`, `276.4`), `MM:SS`, `HH:MM:SS`, ISO 8601 (`PT4M36S`) or friendly
/// (`4m 36s`).
pub(crate) fn parse_duration(text: &str) -> Option<SignedDuration> {
    let text = text.trim();
    if let Ok(secs) = text.parse::<f64>() {
        return duration_from_secs(secs);
    }
    if text.contains(':') {
        let mut secs = 0i64;
        for part in text.split(':') {
            secs = secs.checked_mul(60)?.checked_add(part.trim().parse::<i64>().ok()?)?;
        }
        return (secs > 0).then(|| SignedDuration::from_secs(secs));
    }
    text.parse::<SignedDuration>().ok().filter(|d| d.is_positive())
}

/// A duration given as a JSON number of seconds.
pub(crate) fn duration_from_secs(secs: f64) -> Option<SignedDuration> {
    (secs.is_finite() && secs > 0.0 && secs < 86_400.0).then(|| SignedDuration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts() {
        let local = |h, m, s| Some(Start::Local(civil::time(h, m, s, 0)));
        assert_eq!(parse_start("22:36:51"), local(22, 36, 51));
        assert_eq!(parse_start(" 9:05 "), local(9, 5, 0));
        assert_eq!(parse_start("22h36"), local(22, 36, 0));
        assert_eq!(parse_start("2026-11-04 22:36:51"), local(22, 36, 51));
        let at = Timestamp::from_second(1_793_828_211).unwrap();
        assert_eq!(parse_start("1793828211"), Some(Start::At(at)));
        assert_eq!(parse_start("1793828211000"), Some(Start::At(at)));
        assert_eq!(parse_start("2026-11-04T21:36:51Z"), Some(Start::At(at)));
        assert_eq!(parse_start("2026-11-04T22:36:51+01:00"), Some(Start::At(at)));
        assert_eq!(parse_start("25:00"), None);
        assert_eq!(parse_start("soon"), None);
    }

    #[test]
    fn durations() {
        let secs = |s| Some(SignedDuration::from_secs(s));
        assert_eq!(parse_duration("276"), secs(276));
        assert_eq!(parse_duration("4:36"), secs(276));
        assert_eq!(parse_duration("0:04:36"), secs(276));
        assert_eq!(parse_duration("PT4M36S"), secs(276));
        assert_eq!(parse_duration("4m 36s"), secs(276));
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("-3"), None);
        assert_eq!(parse_duration("long"), None);
    }

    #[test]
    fn text() {
        assert_eq!(clean_text("  BLACK \n  RAINBOWS "), Some("BLACK RAINBOWS".into()));
        assert_eq!(clean_text(" \n "), None);
    }
}
