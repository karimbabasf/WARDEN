//! Per-session outcome composite — the *numerator* of efficiency.
//!
//! Implements the rubric's **outcome signal** (`dossier-rubric-v1`, "Outcome
//! signal" section): a per-session "good session" label scored on
//! **yield-per-resource, NOT minimal resource**. It exists to *empirically
//! validate* the efficiency families later (Phase 2) without circularity, so it
//! leans on the hardest computable markers (the verification gate, thrash/error
//! counts, cache-cold zone) rather than re-deriving the families.
//!
//! THE CARDINAL RULE (rubric, repeated): `token_burn_total` is **never** a
//! standalone negative. A high-burn session that also verified its work, made
//! edits that stuck, and kept rework low is a GOOD session; the bad session is
//! high burn *with* thrash, ignored errors, re-prompts, and no verification.
//!
//! ## Components (each a 0..1 sub-term, name → value, surfaced for transparency)
//! 1. `productive_work` — the gate: `verification_present` (a test/build ran).
//!    This is the one hard anti-hallucinated-completion marker WARDEN computes
//!    per session (`saw_edit`/`saw_done` are transcript markers not on the
//!    FeatureVector, so verification stands in as the productivity gate).
//! 2. `low_rework` — below the WHACK_A_MOLE / IGNORED_TOOL_ERROR thresholds
//!    (`thrash_index < 2`, `file_churn < 4`, `ignored_error_count == 0`,
//!    `tool_error_rate < 0.25`) and low `reprompt_count`.
//! 3. `cache_hygiene` — `cache_read_ratio` as virtue, but GATED: only credited
//!    when real work is present (escapes the idle-frozen-prompt false positive),
//!    and floored when in the CACHE_COLD_RESTARTS zone (`burn > 20k` & ratio < 0.08).
//! 4. `right_sized` — `search_in_main_context` below CONTEXT_BLOAT (8), delegation
//!    present when `tool_call_count` is high (escapes NO_DELEGATION), and
//!    `context_saturation_peak` not riding extreme.
//!
//! `productive` (the boolean label) is the verification gate: productive,
//! verified work is present. `score` is the mean of the four sub-terms.

use crate::ir::FeatureVector;

/// Whether real work is present at all — the floor that gates cache hygiene so an
/// idle session re-reading a frozen prompt at a high cache ratio is not rewarded.
/// Mirrors the rubric's "tool_call_count above a small floor OR token_burn above
/// a floor" gate.
const WORK_TOOL_FLOOR: u32 = 4;
const WORK_BURN_FLOOR: u64 = 5_000;

/// The composite "good session" outcome for one session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutcomeScore {
    pub session_id: String,
    /// Mean of the four component sub-terms, in `0..=1`.
    pub score: f64,
    /// The hard productivity gate: verified work is present.
    pub productive: bool,
    /// The named sub-terms (`name`, `0..1 value`) that compose `score`, surfaced
    /// so the headline is always explainable.
    pub components: Vec<(String, f64)>,
}

/// True when the session shows real work (so cache hygiene can count as virtue).
fn has_real_work(fv: &FeatureVector) -> bool {
    fv.tool_call_count >= WORK_TOOL_FLOOR || fv.token_burn_total >= WORK_BURN_FLOOR
}

/// Score the rubric's outcome signal for a single session.
///
/// See the module docs for the component semantics. Every term is clamped to
/// `0..=1`; `token_burn_total` is consulted only to *gate* cache hygiene and to
/// detect the cold-restart zone — never as a standalone penalty.
pub fn session_outcome(fv: &FeatureVector) -> OutcomeScore {
    let real_work = has_real_work(fv);

    // (1) Productive-work gate — verification ran (anti-hallucinated-completion).
    let productive = fv.verification_present;
    let productive_work = if productive { 1.0 } else { 0.0 };

    // (2) Low-rework — below the WHACK_A_MOLE/IGNORED_TOOL_ERROR thresholds, and
    // low re-prompt. Each breach removes a fixed slice; clamp at 0.
    let mut low_rework = 1.0_f64;
    if fv.thrash_index >= 2.0 {
        low_rework -= 0.4;
    }
    if fv.file_churn >= 4.0 {
        low_rework -= 0.3;
    }
    if fv.ignored_error_count > 0 {
        low_rework -= 0.2;
    }
    if fv.tool_error_rate > 0.25 {
        low_rework -= 0.2;
    }
    // Re-prompts scale the penalty: 0 → none, ≥4 → a full 0.3 slice.
    low_rework -= 0.3 * (fv.reprompt_count.min(4) as f64 / 4.0);
    let low_rework = low_rework.clamp(0.0, 1.0);

    // (3) Cache hygiene — the ratio is already a 0..1 fraction. GATE on real work
    // (idle high-ratio sessions are NEUTRAL, not virtuous), and FLOOR in the
    // CACHE_COLD_RESTARTS zone (burn > 20k with ratio < 0.08).
    let cold_restart = fv.token_burn_total > 20_000 && fv.cache_read_ratio < 0.08;
    let cache_hygiene = if !real_work {
        0.5 // neutral — no work, so the ratio is not evidence of anything
    } else if cold_restart {
        0.15 // cache-busting / cold restarts: hard low
    } else {
        fv.cache_read_ratio.clamp(0.0, 1.0)
    };

    // (4) Right-sized delegation & context. search_in_main_context below the
    // CONTEXT_BLOAT threshold (8): 1.0 at ≤2, linear to 0.0 at ≥8. Penalize the
    // NO_DELEGATION signature (heavy in-main search with zero subagents on a
    // substantive session) and extreme context saturation.
    let search_term = if fv.search_in_main_context <= 2 {
        1.0
    } else if fv.search_in_main_context >= 8 {
        0.0
    } else {
        1.0 - (fv.search_in_main_context as f64 - 2.0) / 6.0
    };
    let mut right_sized = search_term;
    // NO_DELEGATION: substantive in-main search with no delegation at all.
    if fv.search_in_main_context >= 3 && fv.subagent_spawn_count == 0 && fv.tool_call_count >= 4 {
        right_sized = right_sized.min(0.4);
    }
    // Extreme context saturation (riding/over the window) caps the term.
    if fv.context_saturation_peak >= 1.0 {
        right_sized = right_sized.min(0.4);
    }
    let right_sized = right_sized.clamp(0.0, 1.0);

    let components = vec![
        ("productive_work".to_string(), productive_work),
        ("low_rework".to_string(), low_rework),
        ("cache_hygiene".to_string(), cache_hygiene),
        ("right_sized".to_string(), right_sized),
    ];
    let score = components.iter().map(|(_, v)| *v).sum::<f64>() / components.len() as f64;

    OutcomeScore {
        session_id: fv.session_id.clone(),
        score,
        productive,
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean, verified session with real work and no rework scores high and is
    /// labeled productive.
    #[test]
    fn clean_verified_session_scores_high() {
        let fv = FeatureVector {
            session_id: "clean".into(),
            verification_present: true,
            tool_call_count: 12,
            token_burn_total: 8_000,
            cache_read_ratio: 0.85,
            search_in_main_context: 1,
            subagent_spawn_count: 2,
            thrash_index: 0.0,
            file_churn: 1.0,
            reprompt_count: 0,
            ignored_error_count: 0,
            tool_error_rate: 0.0,
            context_saturation_peak: 0.3,
            ..Default::default()
        };
        let out = session_outcome(&fv);
        assert!(out.productive, "verified work present → productive");
        assert!(
            out.score > 0.8,
            "clean verified session should score high, got {}",
            out.score
        );
    }

    /// A thrashing, unverified session scores low and is NOT productive.
    #[test]
    fn thrash_unverified_session_scores_low() {
        let fv = FeatureVector {
            session_id: "thrash".into(),
            verification_present: false,
            tool_call_count: 20,
            token_burn_total: 30_000,
            cache_read_ratio: 0.05, // also in the cold-restart zone
            search_in_main_context: 9,
            subagent_spawn_count: 0,
            thrash_index: 4.0,
            file_churn: 6.0,
            reprompt_count: 5,
            ignored_error_count: 3,
            tool_error_rate: 0.5,
            context_saturation_peak: 1.2,
            ..Default::default()
        };
        let out = session_outcome(&fv);
        assert!(!out.productive, "no verification → not productive");
        assert!(
            out.score < 0.25,
            "thrashing unverified session should score low, got {}",
            out.score
        );
    }

    /// THE RATIO GUARD (Lesson-014 / "cheapness trap"): a large-burn session that
    /// nonetheless verified its work and kept rework low is a GOOD session. High
    /// `token_burn_total` must NEVER, on its own, drag the outcome down.
    #[test]
    fn high_burn_clean_session_is_good() {
        let fv = FeatureVector {
            session_id: "bigburn".into(),
            verification_present: true,
            tool_call_count: 60,
            token_burn_total: 500_000, // huge spend …
            cache_read_ratio: 0.9,     // … but well-cached, real, verified work
            search_in_main_context: 1,
            subagent_spawn_count: 5,
            thrash_index: 0.0,
            file_churn: 2.0,
            reprompt_count: 0,
            ignored_error_count: 0,
            tool_error_rate: 0.02,
            context_saturation_peak: 0.5,
            ..Default::default()
        };
        let out = session_outcome(&fv);
        assert!(out.productive, "high-burn verified work is still productive");
        assert!(
            out.score > 0.8,
            "high burn + verified + low rework must be GOOD, got {}",
            out.score
        );

        // Direct ratio proof: an identical session with 10x LESS burn must not
        // score higher purely for spending less — burn is not a standalone term.
        let cheaper = FeatureVector {
            token_burn_total: 50_000,
            ..fv.clone()
        };
        let cheap_out = session_outcome(&cheaper);
        assert!(
            (cheap_out.score - out.score).abs() < 1e-9,
            "burn alone must not move the score: {} vs {}",
            cheap_out.score,
            out.score
        );
    }

    /// THE IDLE GUARD: a high `cache_read_ratio` with near-zero tool calls and
    /// trivial burn is an idle frozen-prompt session — it is NOT rewarded as
    /// virtuous (cache hygiene is gated to neutral, not credited at the ratio).
    #[test]
    fn idle_high_cache_not_rewarded() {
        let idle = FeatureVector {
            session_id: "idle".into(),
            verification_present: false,
            tool_call_count: 0,
            token_burn_total: 200, // below the work floor
            cache_read_ratio: 0.98,
            ..Default::default()
        };
        let out = session_outcome(&idle);

        // The cache term must be the neutral 0.5, not the raw 0.98 ratio.
        let cache = out
            .components
            .iter()
            .find(|(n, _)| n == "cache_hygiene")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(cache, 0.5, "idle high-cache is neutral, not credited");

        // And a working session at the SAME ratio must out-score the idle one,
        // proving the ratio is only virtue when real work backs it.
        let working = FeatureVector {
            session_id: "working".into(),
            tool_call_count: 10,
            token_burn_total: 8_000,
            ..idle.clone()
        };
        let work_out = session_outcome(&working);
        assert!(
            work_out
                .components
                .iter()
                .find(|(n, _)| n == "cache_hygiene")
                .map(|(_, v)| *v)
                .unwrap()
                > cache,
            "the same cache ratio counts more when real work is present"
        );
    }
}
