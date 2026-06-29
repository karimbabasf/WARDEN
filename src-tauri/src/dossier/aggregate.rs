//! Windowed deterministic aggregation for DOSSIER.
//!
//! Rolls the in-window session corpus up three ways — by project, by ISO week,
//! and by anti-pattern frequency — plus a window-wide mean outcome. Everything
//! here is pure arithmetic over already-computed signals (features, the outcome
//! composite, and the windowed detector findings); NO LLM, NO writes. It is the
//! deterministic substrate the Phase-2 efficiency score and Phase-5 synthesis sit
//! on top of.
//!
//! Windowing is the project convention: a [`Window`] resolves to a `since`
//! cutoff, and every input is pulled through that same cutoff
//! (`features_since(since)`, `scoped_sessions`, `nominate_windowed(.., since)`),
//! so the rollups, the outcome mean, and the pattern frequencies all describe the
//! identical slice of history.

use crate::dossier::outcome::session_outcome;
use crate::dossier::scope::{scoped_sessions, Window};
use chrono::{DateTime, Datelike, Utc};
use std::collections::BTreeMap;

/// Per-project rollup over the window.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectRollup {
    /// Project label (the FeatureVector `project`, or `"unknown"` when undated).
    pub project: String,
    pub sessions: u32,
    pub token_burn: u64,
    /// Mean per-session outcome score for this project, in `0..=1`.
    pub mean_outcome: f64,
}

/// Per-ISO-week rollup over the window.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WeekBin {
    /// ISO week as `YYYY-Www` (e.g. `2026-W25`).
    pub iso_week: String,
    pub sessions: u32,
    pub token_burn: u64,
    pub mean_outcome: f64,
}

/// Frequency + estimated cost of one anti-pattern over the window.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PatternStat {
    pub pattern_id: String,
    /// Number of in-window sessions the pattern fired on.
    pub count: u32,
    /// Summed estimated token cost across those sessions (from the detector).
    pub est_cost_tokens: u64,
}

/// The full deterministic aggregate for one window.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowAggregate {
    pub window: Window,
    /// Count of in-window sessions (the canonical session set, not feature rows).
    pub session_count: u32,
    pub by_project: Vec<ProjectRollup>,
    pub by_week: Vec<WeekBin>,
    pub pattern_freq: Vec<PatternStat>,
    /// Mean outcome score across every in-window feature (0.0 when none).
    pub mean_outcome: f64,
}

/// A running (token_burn, outcome_sum, session_count) accumulator for a rollup key.
#[derive(Default, Clone, Copy)]
struct Acc {
    token_burn: u64,
    outcome_sum: f64,
    sessions: u32,
}
impl Acc {
    fn add(&mut self, token_burn: u64, outcome: f64) {
        self.token_burn += token_burn;
        self.outcome_sum += outcome;
        self.sessions += 1;
    }
    fn mean_outcome(&self) -> f64 {
        if self.sessions == 0 {
            0.0
        } else {
            self.outcome_sum / self.sessions as f64
        }
    }
}

/// Format a UTC instant's ISO week as `YYYY-Www` (zero-padded week).
fn iso_week_label(ts: DateTime<Utc>) -> String {
    let iso = ts.iso_week();
    format!("{}-W{:02}", iso.year(), iso.week())
}

/// Aggregate the window's sessions into project / week / pattern rollups.
///
/// `now` is injected (never read from the clock) so the window is deterministic.
/// Drives the rollups off `features_since(since)` (the same feature slice
/// `nominate_windowed` scores over), and the canonical `session_count` off
/// `scoped_sessions`. Undated features (no `started_at`) survive only the
/// all-time window and are excluded from `by_week` (they have no week), but still
/// count in `by_project` and `mean_outcome`.
pub fn aggregate_window(
    store: &crate::store::Store,
    window: Window,
    now: DateTime<Utc>,
) -> anyhow::Result<WindowAggregate> {
    let since = window.cutoff(now);
    let features = store.features_since(since)?;
    let session_count = scoped_sessions(store, window, now)?.len() as u32;

    let mut by_project: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_week: BTreeMap<String, Acc> = BTreeMap::new();
    let mut outcome_sum = 0.0_f64;

    for fv in &features {
        let score = session_outcome(fv).score;
        outcome_sum += score;

        let project = fv.project.clone().unwrap_or_else(|| "unknown".to_string());
        by_project
            .entry(project)
            .or_default()
            .add(fv.token_burn_total, score);

        if let Some(ts) = fv.started_at {
            by_week
                .entry(iso_week_label(ts))
                .or_default()
                .add(fv.token_burn_total, score);
        }
    }

    let mean_outcome = if features.is_empty() {
        0.0
    } else {
        outcome_sum / features.len() as f64
    };

    let by_project = by_project
        .into_iter()
        .map(|(project, a)| ProjectRollup {
            project,
            sessions: a.sessions,
            token_burn: a.token_burn,
            mean_outcome: a.mean_outcome(),
        })
        .collect();

    let by_week = by_week
        .into_iter()
        .map(|(iso_week, a)| WeekBin {
            iso_week,
            sessions: a.sessions,
            token_burn: a.token_burn,
            mean_outcome: a.mean_outcome(),
        })
        .collect();

    // Pattern frequencies from the SAME windowed detector pass the rest of WARDEN
    // uses. `frequency` is the exact uncapped affected/total ratio, so recover the
    // affected count as `round(frequency * total)` (evidence is capped at 12 and
    // would understate it). `est_cost_tokens` is the detector's uncapped sum.
    let profile = store.profile()?;
    let total = features.len().max(1) as f64;
    let mut pattern_freq: Vec<PatternStat> =
        crate::detectors::nominate_windowed(store, &profile, since)?
            .into_iter()
            .map(|f| PatternStat {
                count: (f.frequency * total).round() as u32,
                pattern_id: f.pattern_id,
                est_cost_tokens: f.est_cost_tokens,
            })
            .collect();
    pattern_freq.sort_by(|a, b| a.pattern_id.cmp(&b.pattern_id));

    Ok(WindowAggregate {
        window,
        session_count,
        by_project,
        by_week,
        pattern_freq,
        mean_outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FeatureVector, Harness, Session};
    use crate::store::Store;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap()
    }

    /// Seed a session row + its FeatureVector via the real write path. `project`
    /// is stored on the feature (the field the rollup keys on); `clean=false`
    /// makes the session trip UNVERIFIED_COMPLETION (`tool_call_count>=4` & no
    /// verification) so a pattern frequency is exercised.
    #[allow(clippy::too_many_arguments)]
    fn seed(
        store: &Store,
        id: &str,
        started_at: DateTime<Utc>,
        project: &str,
        burn: u64,
        clean: bool,
    ) {
        let session = Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
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
                    project: Some(project.into()),
                    token_burn_total: burn,
                    tool_call_count: 6,
                    verification_present: clean,
                    cache_read_ratio: if clean { 0.8 } else { 0.0 },
                    ..Default::default()
                },
                "test",
            )
            .unwrap();
    }

    /// Golden: 4 sessions across 2 projects and 3 ISO weeks. Asserts the
    /// per-project and per-week rollups, the mean-outcome arithmetic, and the
    /// pattern frequency (count + summed cost) from `nominate_windowed`.
    #[test]
    fn aggregates_projects_weeks_and_patterns() {
        let store = Store::memory().unwrap();
        let now = fixed_now();

        // Three distinct ISO weeks in June 2026:
        //   2026-06-08 → W24, 2026-06-15 → W25, 2026-06-22 → W26.
        let w24 = Utc.with_ymd_and_hms(2026, 6, 8, 10, 0, 0).unwrap();
        let w25 = Utc.with_ymd_and_hms(2026, 6, 15, 10, 0, 0).unwrap();
        let w26a = Utc.with_ymd_and_hms(2026, 6, 22, 10, 0, 0).unwrap();
        let w26b = Utc.with_ymd_and_hms(2026, 6, 23, 10, 0, 0).unwrap();

        // alpha: one clean (W24, 1000) + one dirty (W26, 3000, trips UNVERIFIED).
        seed(&store, "a1", w24, "alpha", 1000, true);
        seed(&store, "a2", w26a, "alpha", 3000, false);
        // beta: one clean (W25, 2000) + one dirty (W26, 4000, trips UNVERIFIED).
        seed(&store, "b1", w25, "beta", 2000, true);
        seed(&store, "b2", w26b, "beta", 4000, false);

        let agg = aggregate_window(&store, Window::AllTime, now).unwrap();

        assert_eq!(agg.window, Window::AllTime);
        assert_eq!(agg.session_count, 4);

        // --- by_project (sorted: alpha < beta) ---
        assert_eq!(agg.by_project.len(), 2);
        let alpha = &agg.by_project[0];
        let beta = &agg.by_project[1];
        assert_eq!(alpha.project, "alpha");
        assert_eq!(alpha.sessions, 2);
        assert_eq!(alpha.token_burn, 4000); // 1000 + 3000
        assert_eq!(beta.project, "beta");
        assert_eq!(beta.sessions, 2);
        assert_eq!(beta.token_burn, 6000); // 2000 + 4000

        // mean_outcome per project = mean of the two sessions' outcome scores.
        let a1 = session_outcome(&feat("a1", "alpha", 1000, true)).score;
        let a2 = session_outcome(&feat("a2", "alpha", 3000, false)).score;
        assert!((alpha.mean_outcome - (a1 + a2) / 2.0).abs() < 1e-9);

        // --- by_week (sorted: W24 < W25 < W26) ---
        assert_eq!(agg.by_week.len(), 3);
        assert_eq!(agg.by_week[0].iso_week, "2026-W24");
        assert_eq!(agg.by_week[0].sessions, 1);
        assert_eq!(agg.by_week[0].token_burn, 1000);
        assert_eq!(agg.by_week[1].iso_week, "2026-W25");
        assert_eq!(agg.by_week[1].token_burn, 2000);
        assert_eq!(agg.by_week[2].iso_week, "2026-W26");
        assert_eq!(agg.by_week[2].sessions, 2); // a2 + b2 both in W26
        assert_eq!(agg.by_week[2].token_burn, 7000); // 3000 + 4000

        // --- window mean_outcome = mean across all 4 sessions ---
        let b1 = session_outcome(&feat("b1", "beta", 2000, true)).score;
        let b2 = session_outcome(&feat("b2", "beta", 4000, false)).score;
        assert!((agg.mean_outcome - (a1 + a2 + b1 + b2) / 4.0).abs() < 1e-9);

        // --- pattern_freq: the two dirty sessions trip UNVERIFIED_COMPLETION ---
        let unv = agg
            .pattern_freq
            .iter()
            .find(|p| p.pattern_id == "UNVERIFIED_COMPLETION")
            .expect("UNVERIFIED_COMPLETION should fire on the two dirty sessions");
        assert_eq!(unv.count, 2, "two sessions tripped the pattern");
        assert_eq!(
            unv.est_cost_tokens, 10_000,
            "UNVERIFIED_COMPLETION cost is a flat 5000 per session × 2"
        );
    }

    /// Mirror of `seed`'s FeatureVector so the test can recompute expected
    /// per-session outcome scores independently of the store.
    fn feat(id: &str, project: &str, burn: u64, clean: bool) -> FeatureVector {
        FeatureVector {
            session_id: id.into(),
            started_at: None,
            project: Some(project.into()),
            token_burn_total: burn,
            tool_call_count: 6,
            verification_present: clean,
            cache_read_ratio: if clean { 0.8 } else { 0.0 },
            ..Default::default()
        }
    }

    /// The bounded window excludes out-of-window sessions from every rollup.
    #[test]
    fn bounded_window_excludes_old_sessions() {
        use chrono::Duration;
        let store = Store::memory().unwrap();
        let now = fixed_now();
        seed(&store, "old", now - Duration::days(40), "alpha", 999, true);
        seed(&store, "recent", now - Duration::days(3), "beta", 100, true);

        let agg = aggregate_window(&store, Window::ThirtyDays, now).unwrap();
        assert_eq!(agg.session_count, 1);
        assert_eq!(agg.by_project.len(), 1);
        assert_eq!(agg.by_project[0].project, "beta");
        assert_eq!(agg.by_project[0].token_burn, 100);
    }

    /// An empty store yields a well-formed, zeroed aggregate (no panics, no NaN).
    #[test]
    fn empty_store_yields_zeroed_aggregate() {
        let store = Store::memory().unwrap();
        let agg = aggregate_window(&store, Window::AllTime, fixed_now()).unwrap();
        assert_eq!(agg.session_count, 0);
        assert!(agg.by_project.is_empty());
        assert!(agg.by_week.is_empty());
        assert_eq!(agg.mean_outcome, 0.0);
    }
}
