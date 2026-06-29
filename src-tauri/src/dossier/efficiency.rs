//! DOSSIER efficiency score — the headline number and its per-family breakdown.
//!
//! Two entry points, both reproducible (same input + same [`RUBRIC_VERSION`] →
//! identical score, no corpus dependency):
//!
//!   * [`session_efficiency`] — score ONE session's [`FeatureVector`]:
//!     `headline = Σ weight * family_sub_score` over the seven
//!     [`rubric::FAMILIES`]; `session_count = 1`.
//!   * [`window_efficiency`] — score a TIME WINDOW: pull every in-window
//!     per-session `FeatureVector` from the store, take the MEAN sub-score per
//!     family across those sessions, then `headline = Σ weight * mean_sub_score`;
//!     `session_count` = the number of sessions in the window. An empty window
//!     yields a `0.0` headline with all families present at `0.0` and
//!     `session_count == 0` (a well-defined "no data" score, not an error).
//!
//! The family sub-scores already encode goodness in `0..1` (see [`rubric`]), so
//! the headline is itself a `0..1` efficiency score where 1.0 is ideal.

use crate::dossier::rubric::{self, RUBRIC_VERSION};
use crate::dossier::scope::Window;
use crate::ir::FeatureVector;
use crate::store::Store;
use chrono::{DateTime, Utc};

/// One family's contribution to the headline: its key, the sub-score (a mean
/// across sessions for a window; the single session's score otherwise), and the
/// rubric weight applied to it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FamilyScore {
    pub key: String,
    pub sub_score: f64,
    pub weight: f64,
}

/// A full efficiency result: the `0..1` headline, the pinned rubric version it
/// was computed under (a score is only comparable to another with the same
/// version), the per-family breakdown, and how many sessions fed it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EfficiencyScore {
    pub headline: f64,
    pub rubric_version: String,
    pub families: Vec<FamilyScore>,
    pub session_count: u32,
}

/// Look up a family's weight by key from the pinned [`rubric::FAMILIES`] table.
/// The keys come from `rubric::family_scores`, which is generated from the same
/// table, so this always resolves; `0.0` is a defensive fallback only.
fn weight_for(key: &str) -> f64 {
    rubric::FAMILIES
        .iter()
        .find(|fam| fam.key == key)
        .map(|fam| fam.weight)
        .unwrap_or(0.0)
}

/// Score a single session's [`FeatureVector`]. `headline = Σ weight * sub_score`
/// over the seven families; `session_count = 1`.
pub fn session_efficiency(fv: &FeatureVector) -> EfficiencyScore {
    let families: Vec<FamilyScore> = rubric::family_scores(fv)
        .into_iter()
        .map(|(key, sub_score)| FamilyScore {
            key: key.to_string(),
            sub_score,
            weight: weight_for(key),
        })
        .collect();

    let headline = families
        .iter()
        .map(|f| f.weight * f.sub_score)
        .sum::<f64>();

    EfficiencyScore {
        headline,
        rubric_version: RUBRIC_VERSION.to_string(),
        families,
        session_count: 1,
    }
}

/// Score a time `window`: mean each family's sub-score across every in-window
/// per-session `FeatureVector`, then `headline = Σ weight * mean_sub_score`.
///
/// The window's sessions come from [`Store::features_since`] over the window's
/// [`Window::cutoff`] (the same path Phase 1's aggregators use). `now` is
/// injected rather than read from the clock so the result is deterministic and
/// unit-testable. An empty window returns a `0.0` headline with every family
/// present at `0.0` and `session_count == 0`.
pub fn window_efficiency(
    store: &Store,
    window: Window,
    now: DateTime<Utc>,
) -> anyhow::Result<EfficiencyScore> {
    let features = store.features_since(window.cutoff(now))?;
    Ok(efficiency_over(&features))
}

/// Pure aggregation core shared by [`window_efficiency`] (and directly unit-
/// tested): given the in-window per-session `FeatureVector`s, build the mean-of-
/// sub-scores [`EfficiencyScore`]. Kept separate from the store read so the mean
/// arithmetic can be asserted on fixed fixtures without a DB.
fn efficiency_over(features: &[FeatureVector]) -> EfficiencyScore {
    let session_count = features.len() as u32;

    // Empty window: every family present at 0.0, headline 0.0. This is a defined
    // "no data" score, distinct from an all-1.0 ideal session, so the UI can show
    // a real zero rather than crashing or defaulting to a misleading high score.
    if features.is_empty() {
        let families = rubric::FAMILIES
            .iter()
            .map(|fam| FamilyScore {
                key: fam.key.to_string(),
                sub_score: 0.0,
                weight: fam.weight,
            })
            .collect();
        return EfficiencyScore {
            headline: 0.0,
            rubric_version: RUBRIC_VERSION.to_string(),
            families,
            session_count,
        };
    }

    // Accumulate each family's sub-score across sessions (FAMILIES order is the
    // same as family_scores order, so column index i is family i throughout).
    let mut sums = [0.0_f64; 7];
    for fv in features {
        for (i, (_, sub_score)) in rubric::family_scores(fv).into_iter().enumerate() {
            sums[i] += sub_score;
        }
    }

    let n = features.len() as f64;
    let families: Vec<FamilyScore> = rubric::FAMILIES
        .iter()
        .enumerate()
        .map(|(i, fam)| FamilyScore {
            key: fam.key.to_string(),
            sub_score: sums[i] / n,
            weight: fam.weight,
        })
        .collect();

    let headline = families
        .iter()
        .map(|f| f.weight * f.sub_score)
        .sum::<f64>();

    EfficiencyScore {
        headline,
        rubric_version: RUBRIC_VERSION.to_string(),
        families,
        session_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Harness, Session};
    use std::path::PathBuf;

    /// Insert a parent `sessions` row so a subsequent `save_feature` satisfies the
    /// `features.session_id -> sessions(id)` foreign key (the Phase 1 seed path).
    fn seed_session(store: &Store, id: &str, started_at: DateTime<Utc>) {
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
    }

    fn fv(session_id: &str) -> FeatureVector {
        FeatureVector {
            session_id: session_id.into(),
            ..FeatureVector::default()
        }
    }

    /// A high-quality session: heavy work, verified, low rework, excellent cache,
    /// specific prompt, no over-spawn. Should score a HIGH headline.
    fn clean_session(session_id: &str, burn: u64) -> FeatureVector {
        FeatureVector {
            session_id: session_id.into(),
            token_burn_total: burn,
            context_saturation_peak: 0.3,
            cache_read_ratio: 0.92,
            search_in_main_context: 1,
            subagent_spawn_count: 0,
            subagent_delegation_rate: 0.0,
            tool_call_count: 30,
            tool_error_rate: 0.0,
            ignored_error_count: 0,
            reprompt_count: 0,
            prompt_specificity: 0.9,
            file_churn: 0.0,
            thrash_index: 0.0,
            verification_present: true,
            ..FeatureVector::default()
        }
    }

    /// A thrashy, unverified, vague, cache-cold session. Should score LOW.
    fn thrashy_session(session_id: &str) -> FeatureVector {
        FeatureVector {
            session_id: session_id.into(),
            token_burn_total: 60_000,
            context_saturation_peak: 1.2,
            cache_read_ratio: 0.03,
            search_in_main_context: 9,
            subagent_spawn_count: 0,
            tool_call_count: 30,
            tool_error_rate: 0.6,
            ignored_error_count: 4,
            reprompt_count: 5,
            prompt_specificity: 0.15,
            file_churn: 6.0,
            thrash_index: 3.0,
            verification_present: false,
            ..FeatureVector::default()
        }
    }

    #[test]
    fn session_count_is_one_and_version_pinned() {
        let s = session_efficiency(&fv("s1"));
        assert_eq!(s.session_count, 1);
        assert_eq!(s.rubric_version, "dossier-rubric-v1");
        assert_eq!(s.families.len(), 7, "all seven families present");
    }

    #[test]
    fn headline_equals_weighted_sum_of_families() {
        // The headline must be exactly Σ weight*sub_score over its own families.
        let s = session_efficiency(&clean_session("s1", 50_000));
        let recomputed: f64 = s.families.iter().map(|f| f.weight * f.sub_score).sum();
        assert!(
            (s.headline - recomputed).abs() < 1e-12,
            "headline {} must equal Σ weight*sub_score {}",
            s.headline,
            recomputed
        );
    }

    #[test]
    fn reproducible_same_input_same_score() {
        // Same input twice → byte-identical headline (reproducibility golden).
        let f = clean_session("s1", 50_000);
        let a = session_efficiency(&f);
        let b = session_efficiency(&f);
        assert_eq!(a.headline.to_bits(), b.headline.to_bits());

        // And it equals a hardcoded expected value for this fixed fv. The clean
        // session's family sub-scores are:
        //   delegation_hygiene:        search=1 → 1.0; no deleg → 0.5;
        //                              0.7*1.0 + 0.3*0.5 = 0.85
        //   right_sized_delegation:    spawn==0 → 0.7
        //   context_discipline:        peak 0.3 (<=0.4) → 1.0
        //   cache_efficiency:          real work, ratio 0.92 → 0.92
        //   verification_discipline:   verified → 1.0
        //   rework_and_thrash:         thrash 0, churn 0 → 1.0
        //   prompt_specificity_yield:  spec 0.9, no reprompt → 0.35*0.9+0.65 = 0.965
        // headline = 0.16*0.85 + 0.12*0.7 + 0.12*1.0 + 0.15*0.92
        //          + 0.16*1.0 + 0.11*1.0 + 0.18*0.965
        let expected = 0.16 * 0.85
            + 0.12 * 0.7
            + 0.12 * 1.0
            + 0.15 * 0.92
            + 0.16 * 1.0
            + 0.11 * 1.0
            + 0.18 * 0.965;
        assert!(
            (a.headline - expected).abs() < 1e-9,
            "headline {} != expected {}",
            a.headline,
            expected
        );
    }

    #[test]
    fn high_burn_clean_session_scores_well() {
        // A clean session scores HIGH, and burn is NEVER a term: an identical
        // session with 10x lower burn scores IDENTICALLY.
        let big = session_efficiency(&clean_session("big", 500_000));
        let small = session_efficiency(&clean_session("small", 50_000));
        assert!(big.headline > 0.9, "clean session must score high, got {}", big.headline);
        assert!(
            (big.headline - small.headline).abs() < 1e-12,
            "burn is not a term: {} vs {}",
            big.headline,
            small.headline
        );
    }

    #[test]
    fn thrashy_session_scores_low() {
        let s = session_efficiency(&thrashy_session("bad"));
        assert!(s.headline < 0.35, "thrashy session must score low, got {}", s.headline);
    }

    #[test]
    fn empty_window_is_zero_with_families_present() {
        let s = efficiency_over(&[]);
        assert_eq!(s.session_count, 0);
        assert_eq!(s.headline, 0.0);
        assert_eq!(s.families.len(), 7);
        assert!(s.families.iter().all(|f| f.sub_score == 0.0));
        // Weights are still the real rubric weights even on an empty window.
        let wsum: f64 = s.families.iter().map(|f| f.weight).sum();
        assert!((wsum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn window_mean_is_average_of_per_session_scores() {
        // Two sessions: one clean (high), one thrashy (low). Each family's window
        // sub-score must be the arithmetic mean of the two sessions' sub-scores,
        // and the headline the mean of the two headlines.
        let clean = clean_session("good", 50_000);
        let bad = thrashy_session("bad");
        let agg = efficiency_over(&[clean.clone(), bad.clone()]);
        assert_eq!(agg.session_count, 2);

        let cs = rubric::family_scores(&clean);
        let bs = rubric::family_scores(&bad);
        for (i, famscore) in agg.families.iter().enumerate() {
            let expected_mean = (cs[i].1 + bs[i].1) / 2.0;
            assert!(
                (famscore.sub_score - expected_mean).abs() < 1e-12,
                "{} mean {} != expected {}",
                famscore.key,
                famscore.sub_score,
                expected_mean
            );
        }

        // Because the headline is linear in the sub-scores, mean-of-scores →
        // mean-of-headlines.
        let clean_h = session_efficiency(&clean).headline;
        let bad_h = session_efficiency(&bad).headline;
        assert!(
            (agg.headline - (clean_h + bad_h) / 2.0).abs() < 1e-12,
            "window headline {} != mean of session headlines {}",
            agg.headline,
            (clean_h + bad_h) / 2.0
        );
    }

    #[test]
    fn window_efficiency_reads_store_features_since() {
        // End-to-end through the store: three dated sessions, two inside a 30d
        // window and one far in the past, must average only the two in-window.
        let store = Store::memory().unwrap();
        let now = Utc::now();

        let mut a = clean_session("a", 50_000);
        a.started_at = Some(now - chrono::Duration::days(1));
        let mut b = thrashy_session("b");
        b.started_at = Some(now - chrono::Duration::days(2));
        let mut old = clean_session("old", 50_000);
        old.started_at = Some(now - chrono::Duration::days(200));

        // Seed parent session rows first (features FK -> sessions).
        seed_session(&store, "a", a.started_at.unwrap());
        seed_session(&store, "b", b.started_at.unwrap());
        seed_session(&store, "old", old.started_at.unwrap());

        store.save_feature(&a, "test").unwrap();
        store.save_feature(&b, "test").unwrap();
        store.save_feature(&old, "test").unwrap();

        let s = window_efficiency(&store, Window::ThirtyDays, now).unwrap();
        assert_eq!(s.session_count, 2, "only the two in-window sessions count");

        // Equals the pure aggregate over just the two in-window features.
        let expected = efficiency_over(&[a, b]);
        assert!((s.headline - expected.headline).abs() < 1e-12);

        // All-time picks up all three.
        let all = window_efficiency(&store, Window::AllTime, now).unwrap();
        assert_eq!(all.session_count, 3);
    }
}
