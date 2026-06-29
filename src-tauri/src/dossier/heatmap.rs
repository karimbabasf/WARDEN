//! Token-activity heatmap aggregation for DOSSIER.
//!
//! Buckets the in-window sessions into one [`ActivityCell`] per UTC calendar day,
//! summing each session's `token_burn_total` and splitting the total by harness.
//! This is the raw-activity surface (a GitHub-style contribution grid keyed on
//! tokens, not commits) — it makes NO efficiency judgment: a high-burn clean
//! session and a high-burn thrash session both add the same tokens here. The
//! judgment lives in the efficiency score (Phase 2), never in the heatmap.
//!
//! Data join (see plan Task 2): each scoped [`crate::ir::Session`] is matched to
//! its [`crate::ir::FeatureVector`] by `session_id` for `token_burn_total`; a
//! session with no feature row contributes 0 tokens (still counted as a session).

use crate::dossier::scope::{scoped_sessions, Window};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

/// One UTC calendar day of token activity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActivityCell {
    /// The bucket day as `YYYY-MM-DD` (UTC), derived from each session's `started_at`.
    pub date: String,
    /// Sum of `token_burn_total` across every session that started on `date`.
    pub total_tokens: u64,
    /// Number of sessions that started on `date`.
    pub session_count: u32,
    /// Per-harness split of `total_tokens` / `session_count`, sorted by harness key.
    pub by_harness: Vec<HarnessTokens>,
}

/// One harness's contribution to a single day's [`ActivityCell`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HarnessTokens {
    /// The harness wire key, from [`crate::ir::Harness::as_str`] (e.g. `claude_code`).
    pub harness: String,
    /// Sum of `token_burn_total` for this harness on this day.
    pub tokens: u64,
    /// Number of sessions for this harness on this day.
    pub sessions: u32,
}

/// Per-day token activity for every session inside `window` relative to `now`.
///
/// Joins the scoped sessions to their `token_burn_total` (0 when a session has no
/// feature row), buckets by the session's `started_at` UTC date, sums totals, and
/// splits each day by `harness.as_str()`. Cells are returned sorted by date
/// ascending; within a cell, `by_harness` is sorted by harness key ascending.
pub fn activity_heatmap(
    store: &crate::store::Store,
    window: Window,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<ActivityCell>> {
    // session_id -> token_burn_total. Sessions without a feature row are absent
    // here and resolve to 0 tokens at join time (never dropped).
    let burn_by_session: std::collections::HashMap<String, u64> = store
        .all_features()?
        .into_iter()
        .map(|f| (f.session_id, f.token_burn_total))
        .collect();

    // date -> harness_key -> (tokens, sessions). BTreeMaps keep both the day axis
    // and the per-day harness axis deterministically sorted ascending.
    let mut by_day: BTreeMap<String, BTreeMap<String, (u64, u32)>> = BTreeMap::new();

    for s in scoped_sessions(store, window, now)? {
        let date = s.started_at.format("%Y-%m-%d").to_string();
        let tokens = burn_by_session.get(&s.id).copied().unwrap_or(0);
        let entry = by_day
            .entry(date)
            .or_default()
            .entry(s.harness.as_str().to_string())
            .or_insert((0, 0));
        entry.0 += tokens;
        entry.1 += 1;
    }

    Ok(by_day
        .into_iter()
        .map(|(date, harnesses)| {
            let mut total_tokens = 0u64;
            let mut session_count = 0u32;
            let by_harness = harnesses
                .into_iter()
                .map(|(harness, (tokens, sessions))| {
                    total_tokens += tokens;
                    session_count += sessions;
                    HarnessTokens {
                        harness,
                        tokens,
                        sessions,
                    }
                })
                .collect();
            ActivityCell {
                date,
                total_tokens,
                session_count,
                by_harness,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FeatureVector, Harness, Session};
    use crate::store::Store;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    /// A fixed reference instant so window math is reproducible — never `Utc::now()`.
    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap()
    }

    /// Seed a session (no events) + its FeatureVector via the real write path, so
    /// the heatmap join sees a real `token_burn_total`.
    fn seed(store: &Store, id: &str, harness: Harness, started_at: DateTime<Utc>, burn: u64) {
        let session = Session {
            id: id.into(),
            harness,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: started_at,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&session, &[], &[], 0).unwrap();
        store
            .save_feature(
                &FeatureVector {
                    session_id: id.into(),
                    started_at: Some(started_at),
                    token_burn_total: burn,
                    ..Default::default()
                },
                "test",
            )
            .unwrap();
    }

    /// Golden test: 3 sessions across 2 days and 2 harnesses with known burns.
    /// Asserts cell count, per-day totals, and the exact `by_harness` split.
    /// `high_burn_clean_session_still_counts`: the heatmap is raw tokens — it
    /// applies no judgment (that lives in the efficiency score), so a big-burn
    /// session simply adds its tokens like any other.
    #[test]
    fn golden_two_days_two_harnesses() {
        let store = Store::memory().unwrap();
        let now = fixed_now();
        let day1 = Utc.with_ymd_and_hms(2026, 6, 20, 9, 0, 0).unwrap();
        let day1_late = Utc.with_ymd_and_hms(2026, 6, 20, 22, 0, 0).unwrap();
        let day2 = Utc.with_ymd_and_hms(2026, 6, 21, 10, 0, 0).unwrap();

        // Day 1: Claude 1000 + Codex 500. Day 2: Claude 2000 (a big-burn session).
        seed(&store, "c1", Harness::ClaudeCode, day1, 1000);
        seed(&store, "x1", Harness::Codex, day1_late, 500);
        seed(&store, "c2", Harness::ClaudeCode, day2, 2000);

        let cells = activity_heatmap(&store, Window::AllTime, now).unwrap();

        // Two day-buckets, sorted ascending.
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].date, "2026-06-20");
        assert_eq!(cells[1].date, "2026-06-21");

        // Day 1 totals + per-harness split (harness key sorted: claude_code < codex).
        assert_eq!(cells[0].total_tokens, 1500);
        assert_eq!(cells[0].session_count, 2);
        assert_eq!(
            cells[0].by_harness,
            vec![
                HarnessTokens {
                    harness: "claude_code".into(),
                    tokens: 1000,
                    sessions: 1,
                },
                HarnessTokens {
                    harness: "codex".into(),
                    tokens: 500,
                    sessions: 1,
                },
            ]
        );

        // Day 2: single Claude session, the big-burn one counts at full value.
        assert_eq!(cells[1].total_tokens, 2000);
        assert_eq!(cells[1].session_count, 1);
        assert_eq!(
            cells[1].by_harness,
            vec![HarnessTokens {
                harness: "claude_code".into(),
                tokens: 2000,
                sessions: 1,
            }]
        );
    }

    /// A session with no feature row contributes 0 tokens but is still counted as
    /// a session (the join must never drop it).
    #[test]
    fn session_without_feature_row_counts_zero_tokens() {
        let store = Store::memory().unwrap();
        let now = fixed_now();
        let day = Utc.with_ymd_and_hms(2026, 6, 22, 8, 0, 0).unwrap();

        // Seed a session row directly WITHOUT a feature row.
        let session = Session {
            id: "nofeat".into(),
            harness: Harness::ClaudeCode,
            external_id: "nofeat".into(),
            project: None,
            model_ids: vec![],
            started_at: day,
            ended_at: None,
            source_path: PathBuf::from("/tmp/nofeat.jsonl"),
            raw_hash: 0,
            ingested_at: day,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&session, &[], &[], 0).unwrap();

        let cells = activity_heatmap(&store, Window::AllTime, now).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].total_tokens, 0);
        assert_eq!(cells[0].session_count, 1);
    }

    /// The window cutoff is honored: a session outside the window is excluded
    /// from the heatmap entirely.
    #[test]
    fn window_excludes_out_of_window_sessions() {
        use chrono::Duration;
        let store = Store::memory().unwrap();
        let now = fixed_now();
        seed(
            &store,
            "old",
            Harness::ClaudeCode,
            now - Duration::days(40),
            777,
        );
        seed(
            &store,
            "recent",
            Harness::ClaudeCode,
            now - Duration::days(5),
            123,
        );

        let cells = activity_heatmap(&store, Window::ThirtyDays, now).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].total_tokens, 123);
    }
}
