//! Time-windowing ("Time Machine") for habits/findings queries.
//!
//! A `Window` is a rolling lookback range used to compute habits/findings over a
//! recent slice of history instead of all-time. `since(now)` turns the window into
//! an inclusive lower-bound cutoff (`None` = no filter = all-time); every windowed
//! store query keeps a row iff its timestamp is `>= since`. The string mapping is
//! the FACE contract: the frontend sends `"today"|"7d"|"30d"|"6mo"|"all"` and an
//! unknown value degrades to `AllTime` rather than panicking.

use chrono::{DateTime, Duration, Utc};

/// A rolling lookback window over the ingested history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Last 24 hours.
    Today,
    /// Last 7 days.
    D7,
    /// Last 30 days.
    D30,
    /// Last 180 days (~6 months).
    M6,
    /// No lower bound — every row, all-time.
    AllTime,
}

impl Window {
    /// The inclusive lower-bound cutoff for this window relative to `now`.
    ///
    /// `Some(cutoff)` means "keep rows with timestamp `>= cutoff`"; `None`
    /// (only for `AllTime`) means "no time filter". `now` is passed in (not read
    /// from the clock here) so the math is deterministic and testable.
    pub fn since(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Window::Today => Some(now - Duration::hours(24)),
            Window::D7 => Some(now - Duration::days(7)),
            Window::D30 => Some(now - Duration::days(30)),
            Window::M6 => Some(now - Duration::days(180)),
            Window::AllTime => None,
        }
    }

    /// Parse the FACE wire string into a `Window`. Unknown/empty inputs degrade
    /// to `AllTime` (never panics) so a stale or malformed frontend value can
    /// only ever widen the window, never crash the command.
    pub fn from_str(s: &str) -> Window {
        match s {
            "today" => Window::Today,
            "7d" => Window::D7,
            "30d" => Window::D30,
            "6mo" => Window::M6,
            "all" => Window::AllTime,
            _ => Window::AllTime,
        }
    }

    /// The FACE wire string for this window — the inverse of `from_str` for the
    /// five known variants.
    pub fn as_str(&self) -> &'static str {
        match self {
            Window::Today => "today",
            Window::D7 => "7d",
            Window::D30 => "30d",
            Window::M6 => "6mo",
            Window::AllTime => "all",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A fixed reference instant so the cutoff math is deterministic.
    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap()
    }

    #[test]
    fn since_computes_rolling_cutoffs_from_fixed_now() {
        let now = fixed_now();
        assert_eq!(Window::Today.since(now), Some(now - Duration::hours(24)));
        assert_eq!(Window::D7.since(now), Some(now - Duration::days(7)));
        assert_eq!(Window::D30.since(now), Some(now - Duration::days(30)));
        assert_eq!(Window::M6.since(now), Some(now - Duration::days(180)));
        // All-time has no lower bound.
        assert_eq!(Window::AllTime.since(now), None);
    }

    #[test]
    fn since_today_is_exactly_24h_before_now() {
        let now = fixed_now();
        let cutoff = Window::Today.since(now).unwrap();
        assert_eq!(now - cutoff, Duration::hours(24));
    }

    #[test]
    fn from_str_maps_known_wire_strings() {
        assert_eq!(Window::from_str("today"), Window::Today);
        assert_eq!(Window::from_str("7d"), Window::D7);
        assert_eq!(Window::from_str("30d"), Window::D30);
        assert_eq!(Window::from_str("6mo"), Window::M6);
        assert_eq!(Window::from_str("all"), Window::AllTime);
    }

    #[test]
    fn from_str_unknown_or_empty_degrades_to_all_time() {
        assert_eq!(Window::from_str(""), Window::AllTime);
        assert_eq!(Window::from_str("garbage"), Window::AllTime);
        assert_eq!(Window::from_str("90d"), Window::AllTime);
        assert_eq!(Window::from_str("TODAY"), Window::AllTime); // case-sensitive
    }

    #[test]
    fn as_str_round_trips_for_known_variants() {
        for w in [
            Window::Today,
            Window::D7,
            Window::D30,
            Window::M6,
            Window::AllTime,
        ] {
            assert_eq!(Window::from_str(w.as_str()), w);
        }
    }
}
