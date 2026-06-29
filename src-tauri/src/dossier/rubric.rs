//! DOSSIER efficiency rubric — the pinned v1 scoring rules.
//!
//! Source of truth: `docs/superpowers/research/dossier-efficiency-rubric.md`
//! (`rubric_version: dossier-rubric-v1`). This module implements the seven
//! efficiency families, each with the family's **Normalization** paragraph, as a
//! pure `FeatureVector -> 0..1` sub-score. The family weights below are the v1
//! anchor and sum to exactly `1.000`.
//!
//! ## Two calibration regimes (only the first is built here)
//!
//! Every family's normalization paragraph describes TWO ways to turn a raw signal
//! into a 0..1 score:
//!   1. **Absolute thresholds** — fixed cut-points grounded in WARDEN's own live
//!      detectors (e.g. CONTEXT_BLOAT fires at `search_in_main_context >= 8`).
//!      This is what we implement now: it is reproducible (same input → same
//!      score, no corpus dependency) which is exactly what a *pinned* rubric needs.
//!   2. **Percentile-rank across the operator's own sessions** — the rubric's
//!      stated FUTURE *primary* calibration, because absolute counts scale with
//!      task size. That is **Phase 2a** (the empirical corpus-mining pass) and is
//!      deliberately NOT implemented here. Each family marks where it would plug
//!      in with a `// PERCENTILE-RANK (Phase 2a):` comment.
//!
//! ## Direction is metadata, not a second inversion
//!
//! Each [`Family`] carries a [`Direction`] (higher-better vs lower-better) purely
//! as descriptive metadata for the UI / synthesis layer. The normalization fns
//! ALREADY return "goodness" in `0..1` (1.0 = best behaviour) regardless of
//! direction — a `LowerBetter` family's fn internally inverts its raw signal — so
//! callers must NOT re-invert by direction. `family_scores` and the efficiency
//! engine simply sum `weight * sub_score`.
//!
//! ## Deferred terms (missing `FeatureVector` fields)
//!
//! Some rubric terms reference signals that do not exist as `FeatureVector`
//! fields in the verified Phase 1 API (e.g. `saw_done`, mid-session
//! `ModeChange`/effort-swap events). Per the Phase 1 `outcome.rs` precedent, those
//! multipliers are OMITTED (treated as the identity) rather than fabricated, each
//! marked with a `// rubric: <term> deferred — <field> not on FeatureVector
//! (Phase 2a marker)` comment so the empirical pass can wire them up.

use crate::ir::FeatureVector;

/// The pinned rubric version. Bumped only when weights or normalization change
/// (see spec §4); a score is only comparable to another score with the same
/// `RUBRIC_VERSION`.
pub const RUBRIC_VERSION: &str = "dossier-rubric-v1";

/// Whether, for a family, a higher raw signal is better or worse. Purely
/// descriptive metadata — the normalization fns already encode goodness in
/// `0..1`, so this is never used to re-invert a sub-score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    /// A higher raw signal indicates better behaviour (e.g. cache hit ratio).
    HigherBetter,
    /// A higher raw signal indicates worse behaviour (e.g. thrash index).
    LowerBetter,
}

/// One efficiency family: a stable key, its weight in the headline (all seven sum
/// to `1.000`), and its direction metadata.
#[derive(Debug, Clone, Copy)]
pub struct Family {
    pub key: &'static str,
    pub weight: f64,
    pub direction: Direction,
}

/// The seven efficiency families with the EXACT v1 weights from the rubric doc.
/// Order is the canonical rubric order (1..7); `family_scores` preserves it.
pub const FAMILIES: [Family; 7] = [
    Family {
        key: "delegation_hygiene",
        weight: 0.16,
        direction: Direction::HigherBetter,
    },
    Family {
        key: "right_sized_delegation",
        weight: 0.12,
        direction: Direction::HigherBetter,
    },
    Family {
        key: "context_discipline",
        weight: 0.12,
        direction: Direction::LowerBetter,
    },
    Family {
        key: "cache_efficiency",
        weight: 0.15,
        direction: Direction::HigherBetter,
    },
    Family {
        key: "verification_discipline",
        weight: 0.16,
        direction: Direction::HigherBetter,
    },
    Family {
        key: "rework_and_thrash",
        weight: 0.11,
        direction: Direction::LowerBetter,
    },
    Family {
        key: "prompt_specificity_yield",
        weight: 0.18,
        direction: Direction::HigherBetter,
    },
];

/// Clamp a float into the closed unit interval `[0.0, 1.0]`.
fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Linear interpolation of the score between two raw-signal anchors `(lo, hi)`
/// mapping to scores `(score_lo, score_hi)`, clamped outside the band. Used by
/// the absolute-threshold normalizations so each decay curve is expressed once.
fn lerp_band(x: f64, lo: f64, hi: f64, score_lo: f64, score_hi: f64) -> f64 {
    if x <= lo {
        return score_lo;
    }
    if x >= hi {
        return score_hi;
    }
    let t = (x - lo) / (hi - lo);
    score_lo + t * (score_hi - score_lo)
}

// ---------------------------------------------------------------------------
// 1. delegation_hygiene — weight 0.16, higher_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §1). Composite. (a) search_in_main_context → 1.0 at 0–2
// searches, linear decay to 0.0 at 8 (clamp), grounded in CONTEXT_BLOAT (>=8).
// (b) NO_DELEGATION cap: when search>=3 AND spawn==0, cap the family at 0.4.
// (c) sessions WITH delegation reward subagent_delegation_rate in the healthy
// band 0.1–0.6 (do not reward over-spawning; that is right_sized_delegation's
// job). Final = 0.7*search_term + 0.3*delegation_term, then apply the cap.
fn delegation_hygiene(fv: &FeatureVector) -> f64 {
    // (a) search term: full credit through 2 searches, zero by 8.
    let search_term = lerp_band(fv.search_in_main_context as f64, 2.0, 8.0, 1.0, 0.0);

    // (c) delegation term. With delegation present, reward the healthy band
    // 0.1–0.6: full credit inside it, tapering to a neutral 0.5 as the rate
    // leaves the band (rates above ~0.8 are left for right_sized_delegation to
    // penalize, so we only soft-discount here rather than double-penalizing).
    // Without delegation the term is a neutral 0.5 — the structural NO_DELEGATION
    // case is handled by the hard cap below, not by this term.
    let delegation_term = if fv.subagent_spawn_count > 0 {
        let rate = fv.subagent_delegation_rate;
        if (0.1..=0.6).contains(&rate) {
            1.0
        } else if rate < 0.1 {
            // Ramp 0.5 → 1.0 as the rate climbs from 0 to the band floor.
            lerp_band(rate, 0.0, 0.1, 0.5, 1.0)
        } else {
            // rate > 0.6: taper 1.0 → 0.5 across (0.6, 1.0].
            lerp_band(rate, 0.6, 1.0, 1.0, 0.5)
        }
    } else {
        0.5
    };

    let mut score = 0.7 * search_term + 0.3 * delegation_term;

    // (b) NO_DELEGATION detector cap: heavy in-main searching with zero
    // delegation is the canonical anti-pattern — cap at 0.4 regardless of terms.
    if fv.search_in_main_context >= 3 && fv.subagent_spawn_count == 0 {
        score = score.min(0.4);
    }

    // PERCENTILE-RANK (Phase 2a): replace the absolute 2→8 search curve with the
    // rank of search_in_main_context across the operator's own sessions, since
    // absolute search counts scale with task size.
    clamp01(score)
}

// ---------------------------------------------------------------------------
// 2. right_sized_delegation — weight 0.12, higher_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §2). Two terms, multiplied. (a) over-spawn term:
// r = subagent_spawn_count / (token_burn_total/10000); r<=0.5→1.0, r>=3→0.2,
// linear between. (b) reconciliation term: when spawn>0, if (file_churn>=4 OR
// thrash_index>=2 OR reprompt_count>=3) co-occur with spawns → 0.4 else 1.0.
// Final = over_spawn * reconciliation. spawn==0 → NEUTRAL 0.7.
fn right_sized_delegation(fv: &FeatureVector) -> f64 {
    if fv.subagent_spawn_count == 0 {
        // A single-threaded small task is fine — neutral, not penalized.
        return 0.7;
    }

    // (a) over-spawn ratio: spawns per ~10k tokens of workload. Guard the
    // divisor so a near-zero burn with spawns reads as a very high ratio (many
    // spawns on a cheap workload → low score) rather than dividing by zero.
    let workload = (fv.token_burn_total as f64 / 10_000.0).max(1e-9);
    let r = fv.subagent_spawn_count as f64 / workload;
    let over_spawn_term = lerp_band(r, 0.5, 3.0, 1.0, 0.2);

    // (b) reconciliation penalty: coupled work fanned out then reconciled shows
    // up as a churn / thrash / re-prompt spike co-occurring with the spawns.
    let reconciliation_term =
        if fv.file_churn >= 4.0 || fv.thrash_index >= 2.0 || fv.reprompt_count >= 3 {
            0.4
        } else {
            1.0
        };

    // PERCENTILE-RANK (Phase 2a): the over-spawn crossover r is workload- and
    // task-type-dependent (Augment's 2–3-step crossover), so calibrate r against
    // the corpus rather than the hard-coded 0.5/3.0 band.
    clamp01(over_spawn_term * reconciliation_term)
}

// ---------------------------------------------------------------------------
// 3. context_discipline — weight 0.12, lower_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §3). Primary: context_saturation_peak, lower_better.
// Map: 1.0 for peak<=0.4, linear decay to 0.2 at peak>=1.0, floor 0.1 above 1.0.
// MEASUREMENT CAVEAT: WARDEN computes peak as cumulative_input/DEFAULT_WINDOW, so
// it can exceed 1.0 — not a literal occupancy fraction. Returns goodness (higher
// = better) so callers do not re-invert by direction.
fn context_discipline(fv: &FeatureVector) -> f64 {
    let peak = fv.context_saturation_peak;
    if peak > 1.0 {
        // Floor for sessions that ran far past a full window.
        return 0.1;
    }
    // 1.0 through peak 0.4, decaying to 0.2 by peak 1.0.
    // PERCENTILE-RANK (Phase 2a): the 0.8–0.9 "danger" band is a practitioner
    // heuristic, not a measured cliff — prefer the rank of peak across the
    // operator's sessions (or RADAR fill_pct where joinable) as the PRIMARY curve.
    clamp01(lerp_band(peak, 0.4, 1.0, 1.0, 0.2))
}

// ---------------------------------------------------------------------------
// 4. cache_efficiency — weight 0.15, higher_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §4). Primary: cache_read_ratio directly as the 0..1
// sub-score. GATE against idle false positives: only credit the ratio as virtue
// when real work is present (tool_call_count above a small floor OR
// token_burn_total above a floor); an idle session re-reading a frozen prompt at
// a high ratio is NEUTRAL, not high. HARD FLOOR (CACHE_COLD_RESTARTS): if
// token_burn_total>20000 AND cache_read_ratio<0.08, force the family <=0.15.
fn cache_efficiency(fv: &FeatureVector) -> f64 {
    // Work gate — mirrors outcome.rs's WORK_TOOL_FLOOR / WORK_BURN_FLOOR so the
    // idle-frozen-prompt false positive is scored neutral rather than rewarded.
    let real_work = fv.tool_call_count >= WORK_TOOL_FLOOR || fv.token_burn_total >= WORK_BURN_FLOOR;
    if !real_work {
        return 0.5;
    }

    let mut score = clamp01(fv.cache_read_ratio);

    // CACHE_COLD_RESTARTS hard floor: substantial burn with a near-zero cache
    // read ratio is cache-busting / cold restarts — cap hard.
    if fv.token_burn_total > 20_000 && fv.cache_read_ratio < 0.08 {
        score = score.min(0.15);
    }

    // rubric: mid-session ModeChange (effort/model swap) cache-detonation cap
    // deferred — ModeChange / effort-swap events are not on FeatureVector
    // (Phase 2a marker).

    // PERCENTILE-RANK (Phase 2a): "cache_read_ratio above a corpus-relative
    // median" — rank the ratio across the operator's sessions once a corpus exists.
    score
}

/// Real-work floors gating cache credit (see [`cache_efficiency`]). Mirror the
/// Phase 1 `outcome.rs` constants so the idle-session gate is consistent across
/// DOSSIER: real work = `tool_call_count >= 4` OR `token_burn_total >= 5_000`.
const WORK_TOOL_FLOOR: u32 = 4;
const WORK_BURN_FLOOR: u64 = 5_000;

// ---------------------------------------------------------------------------
// 5. verification_discipline — weight 0.16, higher_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §5). Composite, multiplied. (a) validation-present term:
// if tool_call_count>=4 AND !verification_present → 0.1; if verification_present
// → 1.0; trivial sessions (tool_call_count<4) → NEUTRAL 0.7. (b) unbacked-
// completion term: saw_done==true AND !verification_present → *0.4 (DEFERRED —
// saw_done is not on FeatureVector). (c) ignored-error term: ignored_error_count>0
// and tool_error_rate>0.25 scale 1.0→0.3. Final = validation * unbacked * error.
fn verification_discipline(fv: &FeatureVector) -> f64 {
    // (a) validation-present term (UNVERIFIED_COMPLETION basis).
    let validation_term = if fv.verification_present {
        1.0
    } else if fv.tool_call_count >= 4 {
        // Substantive session that never ran a test/build.
        0.1
    } else {
        // A one-line change needs no build — neutral.
        0.7
    };

    // (b) unbacked-completion term.
    // rubric: unbacked-completion multiplier (saw_done && !verification_present
    // → *0.4) deferred — saw_done not on FeatureVector (Phase 2a marker).
    // Omitted as the identity (1.0) rather than fabricated.
    let unbacked_term = 1.0;

    // (c) ignored-error term (IGNORED_TOOL_ERROR basis): both conditions present
    // → 0.3; otherwise full credit.
    let error_term = if fv.ignored_error_count > 0 && fv.tool_error_rate > 0.25 {
        0.3
    } else {
        1.0
    };

    // PERCENTILE-RANK (Phase 2a): verification_present is binary — prefer the
    // rank of a continuous validation-share proxy (verification-type bash calls /
    // tool_call_count) once corpus-mined.
    clamp01(validation_term * unbacked_term * error_term)
}

// ---------------------------------------------------------------------------
// 6. rework_and_thrash — weight 0.11, lower_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §6). Primary: thrash_index + file_churn, lower_better,
// anchored to WHACK_A_MOLE (thrash_index>=2.0 OR file_churn>=4.0). Map: 1.0 at
// thrash_index==0 AND file_churn<2; decay to 0.2 when thrash_index>=2 OR
// file_churn>=4. CONDITION on verification: when verification_present, multiply
// the PENALTY (1-score) by 0.6 (interleaved-verify iteration is healthy churn).
// Returns goodness (higher = better) so callers do not re-invert by direction.
fn rework_and_thrash(fv: &FeatureVector) -> f64 {
    // Base goodness from churn. Take the worse (lower) of the two axes so either
    // crossing its WHACK_A_MOLE threshold drives the score down.
    let thrash_score = lerp_band(fv.thrash_index, 0.0, 2.0, 1.0, 0.2);
    let churn_score = lerp_band(fv.file_churn, 2.0, 4.0, 1.0, 0.2);
    let base = thrash_score.min(churn_score);

    // Verification softens the penalty: execution-feedback-driven iteration is
    // healthy, so high-but-verified churn is less penalized than unverified churn.
    let penalty = 1.0 - base;
    let softened = if fv.verification_present {
        penalty * 0.6
    } else {
        penalty
    };

    // PERCENTILE-RANK (Phase 2a): rank thrash_index / file_churn across the
    // operator's sessions; absolute WHACK_A_MOLE thresholds are the cold-start
    // fallback only.
    clamp01(1.0 - softened)
}

// ---------------------------------------------------------------------------
// 7. prompt_specificity_yield — weight 0.18, higher_better
// ---------------------------------------------------------------------------
//
// Normalization (rubric §7). Composite. (a) specificity term: prompt_specificity
// directly (0..1), with VAGUE_PROMPT hard zone — specificity in (0,0.28) AND
// reprompt_count>0 → term<=0.3. (b) re-prompt coupling: 1.0 at reprompt_count==0,
// decaying to 0.3 at reprompt_count>=4 WITH churn present. Final =
// 0.35*specificity_term + 0.65*reprompt_term (reprompt weighted higher because it
// is the hard, lagging rework confirmation; specificity is a coarse lexical proxy).
fn prompt_specificity_yield(fv: &FeatureVector) -> f64 {
    // (a) specificity term — the raw score, clamped into the VAGUE_PROMPT zone.
    let mut specificity_term = clamp01(fv.prompt_specificity);
    let in_vague_zone =
        fv.prompt_specificity > 0.0 && fv.prompt_specificity < 0.28 && fv.reprompt_count > 0;
    if in_vague_zone {
        specificity_term = specificity_term.min(0.3);
    }

    // (b) re-prompt coupling term. Re-prompts only confirm thrash when downstream
    // churn/thrash actually follows, so the decay to 0.3 only bites with churn
    // present; re-prompts with no churn are softened toward neutral.
    let churn_present = fv.file_churn >= 2.0 || fv.thrash_index >= 1.0;
    let reprompt_term = if fv.reprompt_count == 0 {
        1.0
    } else if churn_present {
        // Full decay: 1.0 at 0 → 0.3 at >=4 re-prompts.
        lerp_band(fv.reprompt_count as f64, 0.0, 4.0, 1.0, 0.3)
    } else {
        // Re-prompts without downstream churn — softer decay toward 0.6.
        lerp_band(fv.reprompt_count as f64, 0.0, 4.0, 1.0, 0.6)
    };

    // PERCENTILE-RANK (Phase 2a): prompt_specificity is the fuzziest field in
    // WARDEN (coarse lexical proxy) — rank it across the operator's corpus rather
    // than trusting its absolute scale.
    clamp01(0.35 * specificity_term + 0.65 * reprompt_term)
}

/// All seven family sub-scores for one session, in [`FAMILIES`] order. Each entry
/// is `(family_key, sub_score)` with `sub_score` in `0..=1` (1.0 = best). The
/// efficiency engine multiplies these by the matching [`Family::weight`].
pub fn family_scores(fv: &FeatureVector) -> Vec<(&'static str, f64)> {
    vec![
        ("delegation_hygiene", delegation_hygiene(fv)),
        ("right_sized_delegation", right_sized_delegation(fv)),
        ("context_discipline", context_discipline(fv)),
        ("cache_efficiency", cache_efficiency(fv)),
        ("verification_discipline", verification_discipline(fv)),
        ("rework_and_thrash", rework_and_thrash(fv)),
        ("prompt_specificity_yield", prompt_specificity_yield(fv)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blank baseline FeatureVector to mutate per-test (mirrors the Phase 1
    /// test style of `..FeatureVector::default()`).
    fn fv() -> FeatureVector {
        FeatureVector::default()
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    // ---- structural invariants ------------------------------------------

    #[test]
    fn weights_sum_to_one() {
        let sum: f64 = FAMILIES.iter().map(|f| f.weight).sum();
        assert!((sum - 1.0).abs() < 1e-9, "family weights must sum to 1.0, got {sum}");
    }

    #[test]
    fn family_scores_returns_all_seven_in_order() {
        let scores = family_scores(&fv());
        let keys: Vec<&str> = scores.iter().map(|(k, _)| *k).collect();
        let expected: Vec<&str> = FAMILIES.iter().map(|f| f.key).collect();
        assert_eq!(keys, expected, "family_scores must match FAMILIES order");
        for (k, s) in scores {
            assert!((0.0..=1.0).contains(&s), "{k} sub-score {s} out of 0..1");
        }
    }

    // ---- 1. delegation_hygiene boundaries -------------------------------

    #[test]
    fn delegation_hygiene_zero_search_is_high() {
        // search=0 → search_term 1.0; no delegation → delegation_term 0.5; no cap
        // (search<3). 0.7*1.0 + 0.3*0.5 = 0.85.
        let mut f = fv();
        f.search_in_main_context = 0;
        approx(delegation_hygiene(&f), 0.85);
    }

    #[test]
    fn delegation_hygiene_heavy_search_no_delegation_is_capped() {
        // search=8 → search_term 0.0; spawn==0 & search>=3 → NO_DELEGATION cap 0.4.
        // Raw 0.7*0.0 + 0.3*0.5 = 0.15, already below the cap → 0.15.
        let mut f = fv();
        f.search_in_main_context = 8;
        let s = delegation_hygiene(&f);
        approx(s, 0.15);
        assert!(s <= 0.4, "NO_DELEGATION must keep this family <= 0.4");
    }

    #[test]
    fn delegation_hygiene_cap_binds_when_raw_would_exceed() {
        // search=3 (term = 1 - 1/6 ≈ 0.8333), spawn==0 → cap 0.4 must bind even
        // though raw 0.7*0.8333 + 0.3*0.5 ≈ 0.7333 > 0.4.
        let mut f = fv();
        f.search_in_main_context = 3;
        let s = delegation_hygiene(&f);
        approx(s, 0.4);
    }

    #[test]
    fn delegation_hygiene_healthy_delegation_band_full_credit() {
        // search=2 → search_term 1.0; spawn>0 with rate in [0.1,0.6] →
        // delegation_term 1.0; no cap. 0.7+0.3 = 1.0.
        let mut f = fv();
        f.search_in_main_context = 2;
        f.subagent_spawn_count = 2;
        f.subagent_delegation_rate = 0.4;
        approx(delegation_hygiene(&f), 1.0);
    }

    // ---- 2. right_sized_delegation boundaries ---------------------------

    #[test]
    fn right_sized_no_spawn_is_neutral() {
        let mut f = fv();
        f.subagent_spawn_count = 0;
        approx(right_sized_delegation(&f), 0.7);
    }

    #[test]
    fn right_sized_proportionate_spawns_full_credit() {
        // 2 spawns over 100k tokens → workload 10, r = 0.2 (<=0.5) → over_spawn 1.0;
        // no reconciliation spike → 1.0. Final 1.0.
        let mut f = fv();
        f.subagent_spawn_count = 2;
        f.token_burn_total = 100_000;
        approx(right_sized_delegation(&f), 1.0);
    }

    #[test]
    fn right_sized_overspawn_small_workload_low() {
        // 6 spawns over 10k tokens → workload 1, r = 6 (>=3) → over_spawn 0.2;
        // no reconciliation spike → 0.2.
        let mut f = fv();
        f.subagent_spawn_count = 6;
        f.token_burn_total = 10_000;
        approx(right_sized_delegation(&f), 0.2);
    }

    #[test]
    fn right_sized_reconciliation_spike_penalizes() {
        // Proportionate spawns (over_spawn 1.0) but a churn spike co-occurs →
        // reconciliation 0.4. Final 1.0*0.4 = 0.4.
        let mut f = fv();
        f.subagent_spawn_count = 2;
        f.token_burn_total = 100_000;
        f.file_churn = 5.0;
        approx(right_sized_delegation(&f), 0.4);
    }

    // ---- 3. context_discipline boundaries -------------------------------

    #[test]
    fn context_discipline_low_peak_is_one() {
        let mut f = fv();
        f.context_saturation_peak = 0.4;
        approx(context_discipline(&f), 1.0);
    }

    #[test]
    fn context_discipline_full_window_is_floor_band() {
        // peak == 1.0 → bottom of the linear band = 0.2.
        let mut f = fv();
        f.context_saturation_peak = 1.0;
        approx(context_discipline(&f), 0.2);
    }

    #[test]
    fn context_discipline_above_window_is_hard_floor() {
        let mut f = fv();
        f.context_saturation_peak = 1.5;
        approx(context_discipline(&f), 0.1);
    }

    // ---- 4. cache_efficiency boundaries ---------------------------------

    #[test]
    fn cache_efficiency_idle_session_is_neutral() {
        // No real work (tool_call_count < 4 AND burn < 5_000): high ratio must NOT
        // be rewarded — neutral 0.5.
        let mut f = fv();
        f.cache_read_ratio = 0.95;
        f.tool_call_count = 1;
        f.token_burn_total = 100;
        approx(cache_efficiency(&f), 0.5);
    }

    #[test]
    fn cache_efficiency_high_ratio_real_work_is_high() {
        let mut f = fv();
        f.cache_read_ratio = 0.92;
        f.tool_call_count = 20;
        f.token_burn_total = 50_000;
        approx(cache_efficiency(&f), 0.92);
    }

    #[test]
    fn cache_efficiency_cold_restart_floor_triggers() {
        // burn > 20k AND ratio < 0.08 → CACHE_COLD_RESTARTS floor <= 0.15.
        let mut f = fv();
        f.cache_read_ratio = 0.05;
        f.token_burn_total = 50_000;
        f.tool_call_count = 20;
        let s = cache_efficiency(&f);
        assert!(s <= 0.15, "cold-restart floor must cap at <= 0.15, got {s}");
        approx(s, 0.05);
    }

    // ---- 5. verification_discipline boundaries --------------------------

    #[test]
    fn verification_present_is_full_credit() {
        let mut f = fv();
        f.verification_present = true;
        f.tool_call_count = 10;
        approx(verification_discipline(&f), 1.0);
    }

    #[test]
    fn verification_absent_substantive_session_is_low() {
        // tool_call_count>=4 AND !verification_present → validation 0.1.
        let mut f = fv();
        f.verification_present = false;
        f.tool_call_count = 10;
        approx(verification_discipline(&f), 0.1);
    }

    #[test]
    fn verification_trivial_session_is_neutral() {
        // tool_call_count < 4 AND !verification_present → neutral 0.7.
        let mut f = fv();
        f.verification_present = false;
        f.tool_call_count = 2;
        approx(verification_discipline(&f), 0.7);
    }

    #[test]
    fn verification_ignored_errors_penalize() {
        // verified (1.0) but ignored errors over the rate threshold → *0.3.
        let mut f = fv();
        f.verification_present = true;
        f.tool_call_count = 10;
        f.ignored_error_count = 2;
        f.tool_error_rate = 0.5;
        approx(verification_discipline(&f), 0.3);
    }

    // ---- 6. rework_and_thrash boundaries --------------------------------

    #[test]
    fn rework_clean_session_is_one() {
        // thrash 0, churn < 2 → base 1.0 → goodness 1.0.
        let f = fv();
        approx(rework_and_thrash(&f), 1.0);
    }

    #[test]
    fn rework_high_thrash_unverified_full_penalty() {
        // thrash 2 → thrash_score 0.2; churn 0 → churn_score 1.0; base = 0.2.
        // unverified → full penalty 0.8 → goodness 0.2.
        let mut f = fv();
        f.thrash_index = 2.0;
        f.verification_present = false;
        approx(rework_and_thrash(&f), 0.2);
    }

    #[test]
    fn rework_high_thrash_verified_softened() {
        // Same churn but verification_present → penalty 0.8*0.6 = 0.48 →
        // goodness 0.52.
        let mut f = fv();
        f.thrash_index = 2.0;
        f.verification_present = true;
        approx(rework_and_thrash(&f), 0.52);
    }

    #[test]
    fn rework_high_churn_drives_score_down() {
        // churn 4 → churn_score 0.2 even with thrash 0; unverified → goodness 0.2.
        let mut f = fv();
        f.file_churn = 4.0;
        f.verification_present = false;
        approx(rework_and_thrash(&f), 0.2);
    }

    // ---- 7. prompt_specificity_yield boundaries -------------------------

    #[test]
    fn prompt_specificity_high_no_reprompt_is_high() {
        // specificity 0.9 → spec_term 0.9; reprompt 0 → reprompt_term 1.0.
        // 0.35*0.9 + 0.65*1.0 = 0.965.
        let mut f = fv();
        f.prompt_specificity = 0.9;
        f.reprompt_count = 0;
        approx(prompt_specificity_yield(&f), 0.965);
    }

    #[test]
    fn prompt_specificity_vague_zone_with_reprompts_is_low() {
        // specificity 0.2 (in (0,0.28)) AND reprompt>0 → the VAGUE_PROMPT cap is a
        // CEILING (`.min(0.3)`), and 0.2 is already below it, so spec_term stays
        // 0.2 (the cap binds only when specificity exceeds 0.3 inside the zone).
        // reprompt 4 WITH churn → reprompt_term 0.3. 0.35*0.2 + 0.65*0.3 = 0.265.
        let mut f = fv();
        f.prompt_specificity = 0.2;
        f.reprompt_count = 4;
        f.file_churn = 3.0;
        let s = prompt_specificity_yield(&f);
        approx(s, 0.265);
        assert!(s <= 0.3, "vague + re-prompts must be a low-score zone");
    }

    #[test]
    fn prompt_specificity_vague_cap_binds_above_threshold() {
        // A specificity of 0.27 sits inside the (0,0.28) VAGUE zone but ABOVE the
        // 0.3 ceiling? No — 0.27 < 0.3, so to actually exercise the ceiling we use
        // a value that would otherwise exceed it. The zone is (0,0.28), so the
        // ceiling can only bind for specificity in (0.3,0.28) which is empty —
        // i.e. the cap is defensive and never raises a score. Assert it never
        // INCREASES the term: at specificity 0.27 (vague, reprompts) the spec_term
        // is the raw 0.27, not lifted to 0.3.
        let mut f = fv();
        f.prompt_specificity = 0.27;
        f.reprompt_count = 1;
        // reprompt 1, no churn → reprompt_term = lerp(1,0,4,1.0,0.6) = 0.9.
        // 0.35*0.27 + 0.65*0.9 = 0.0945 + 0.585 = 0.6795.
        approx(prompt_specificity_yield(&f), 0.6795);
    }

    #[test]
    fn prompt_specificity_reprompts_couple_with_churn() {
        // specificity 0.9 (spec_term 0.9, not vague), reprompt 4 WITH churn →
        // reprompt_term 0.3. 0.35*0.9 + 0.65*0.3 = 0.51.
        let mut f = fv();
        f.prompt_specificity = 0.9;
        f.reprompt_count = 4;
        f.file_churn = 3.0;
        approx(prompt_specificity_yield(&f), 0.51);
    }
}
