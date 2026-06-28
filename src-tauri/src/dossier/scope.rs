//! Time-window scoping for DOSSIER.
//!
//! Every DOSSIER aggregation is computed over a user-selected time window
//! (all-time / 6mo / 3mo / 30d / 2wk). [`Window`] is the canonical enum the
//! frontend toggles, and [`scoped_sessions`] is the single helper every
//! aggregator uses to pull the in-window slice of sessions from the store.
//!
//! Windowing convention (see plan Global Constraints): a window resolves to an
//! optional `since` cutoff via [`Window::cutoff`]; `AllTime` is `None` (no
//! filter), every other window subtracts a fixed duration from `now`.

use chrono::{DateTime, Duration, Utc};

/// The operator-facing time window every DOSSIER aggregation is scoped to.
///
/// The serde representation is the exact set of strings the frontend toggle
/// emits — kept stable via per-variant `rename` so a UI change can never
/// silently drift the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Window {
    /// No cutoff — every session ever ingested.
    #[serde(rename = "all-time")]
    AllTime,
    /// ~6 months (183 days).
    #[serde(rename = "6mo")]
    SixMonths,
    /// ~3 months (91 days).
    #[serde(rename = "3mo")]
    ThreeMonths,
    /// 30 days.
    #[serde(rename = "30d")]
    ThirtyDays,
    /// 2 weeks (14 days).
    #[serde(rename = "2wk")]
    TwoWeeks,
}

impl Window {
    /// The inclusive `since` instant for this window relative to `now`.
    ///
    /// `AllTime` returns `None` (interpreted downstream as "no lower bound");
    /// every bounded window returns `Some(now - duration)`.
    pub fn cutoff(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Window::AllTime => None,
            Window::SixMonths => Some(now - Duration::days(183)),
            Window::ThreeMonths => Some(now - Duration::days(91)),
            Window::ThirtyDays => Some(now - Duration::days(30)),
            Window::TwoWeeks => Some(now - Duration::days(14)),
        }
    }
}

impl std::str::FromStr for Window {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all-time" => Ok(Window::AllTime),
            "6mo" => Ok(Window::SixMonths),
            "3mo" => Ok(Window::ThreeMonths),
            "30d" => Ok(Window::ThirtyDays),
            "2wk" => Ok(Window::TwoWeeks),
            other => Err(anyhow::anyhow!("unknown dossier window: {other:?}")),
        }
    }
}

/// All sessions whose `started_at` falls inside `window` relative to `now`.
///
/// Reads every session via [`crate::store::Store::sessions`] (newest-first) and
/// retains those at or after the window's [`Window::cutoff`]; `AllTime` retains
/// all. Read-only — the caller owns any further per-session enrichment.
pub fn scoped_sessions(
    store: &crate::store::Store,
    window: Window,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<crate::ir::Session>> {
    let cutoff = window.cutoff(now);
    Ok(store
        .sessions()?
        .into_iter()
        .filter(|s| cutoff.map_or(true, |c| s.started_at >= c))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Harness;
    use crate::store::Store;
    use chrono::{Duration, TimeZone, Utc};

    /// A fixed reference instant so window math is reproducible — never `Utc::now()`.
    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap()
    }

    /// Insert a bare session with a controllable `started_at` via the real upsert
    /// path (no turns/events needed — `scoped_sessions` reads only the sessions row).
    fn seed_session_at(store: &Store, id: &str, started_at: chrono::DateTime<Utc>) {
        let session = crate::ir::Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at,
            ended_at: None,
            source_path: std::path::PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: started_at,
            meta: serde_json::json!({}),
        };
        store
            .upsert_session_batch(&session, &[], &[], 0)
            .unwrap();
    }

    #[test]
    fn cutoff_all_time_is_none() {
        assert_eq!(Window::AllTime.cutoff(fixed_now()), None);
    }

    #[test]
    fn cutoff_30d_subtracts() {
        let now = fixed_now();
        assert_eq!(
            Window::ThirtyDays.cutoff(now),
            Some(now - Duration::days(30))
        );
    }

    #[test]
    fn window_parses_frontend_strings() {
        assert_eq!("all-time".parse::<Window>().unwrap(), Window::AllTime);
        assert_eq!("6mo".parse::<Window>().unwrap(), Window::SixMonths);
        assert_eq!("3mo".parse::<Window>().unwrap(), Window::ThreeMonths);
        assert_eq!("30d".parse::<Window>().unwrap(), Window::ThirtyDays);
        assert_eq!("2wk".parse::<Window>().unwrap(), Window::TwoWeeks);
        assert!("bogus".parse::<Window>().is_err());
    }

    #[test]
    fn scoped_sessions_filters_by_cutoff() {
        let store = Store::memory().unwrap();
        let now = fixed_now();
        seed_session_at(&store, "old", now - Duration::days(40));
        seed_session_at(&store, "recent", now - Duration::days(5));

        let within_30d = scoped_sessions(&store, Window::ThirtyDays, now).unwrap();
        let ids: Vec<&str> = within_30d.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["recent"]);

        let all = scoped_sessions(&store, Window::AllTime, now).unwrap();
        assert_eq!(all.len(), 2);
    }
}
