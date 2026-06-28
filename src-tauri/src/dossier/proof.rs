//! By-proof guard (spec §12) — honesty enforcement before a profile is shown.
//!
//! DOSSIER's promise is "profile by proof": nothing is *asserted* unless the
//! evidence clears a bar. This module is the chokepoint that enforces it on the
//! two claim-shaped outputs — individual [`Claim`]s and per-trait
//! [`TraitTrend`]s — by *downgrading* (never deleting) anything underpowered:
//!
//!   * A claim with fewer than [`MIN_EVIDENCE_ASSERTED`] cited [`EvidenceRef`]s
//!     is forced to [`ClaimStatus::Emerging`] and its confidence clamped to
//!     `≤ 0.5` ("we see a hint, not a pattern").
//!   * A trend whose buckets are too sparse (any bucket under
//!     [`MIN_SAMPLES_PER_BUCKET`] samples, or fewer than 3 buckets total) is
//!     forced to [`TrendDir::Insufficient`] with confidence `≤ 0.4` — a 2-week
//!     "trend" off two data points is noise, not a trajectory.
//!
//! Pure functions over owned values; no I/O, fully deterministic. The synthesis
//! layer (Phase 5b) runs everything through here before persistence so the cache
//! can never hold an over-asserted claim.

use crate::dossier::types::{Claim, ClaimStatus, TraitTrend, TrendDir};

/// Minimum cited evidence for a [`Claim`] to be [`ClaimStatus::Asserted`].
pub const MIN_EVIDENCE_ASSERTED: usize = 3;

/// Minimum samples a trend bucket must hold for the trend to be classifiable
/// (and the trend must span at least 3 buckets).
pub const MIN_SAMPLES_PER_BUCKET: usize = 3;

/// Confidence ceiling applied to a downgraded ("emerging") claim.
const EMERGING_CONF_CAP: f64 = 0.5;

/// Confidence ceiling applied to an "insufficient" trend.
const INSUFFICIENT_CONF_CAP: f64 = 0.4;

/// Gate one claim: assert it only if it carries `≥ MIN_EVIDENCE_ASSERTED`
/// evidence; otherwise downgrade to `Emerging` and clamp confidence to
/// `≤ EMERGING_CONF_CAP`. An asserted claim's confidence is left untouched.
pub fn gate_claim(mut claim: Claim) -> Claim {
    if claim.evidence.len() < MIN_EVIDENCE_ASSERTED {
        claim.status = ClaimStatus::Emerging;
        if claim.confidence > EMERGING_CONF_CAP {
            claim.confidence = EMERGING_CONF_CAP;
        }
    } else {
        claim.status = ClaimStatus::Asserted;
    }
    claim
}

/// Gate a batch of claims (see [`gate_claim`]).
pub fn gate_claims(claims: Vec<Claim>) -> Vec<Claim> {
    claims.into_iter().map(gate_claim).collect()
}

/// Gate a trend against its per-bucket sample counts. If the series has fewer
/// than 3 buckets, or *any* bucket holds `< MIN_SAMPLES_PER_BUCKET` samples, the
/// trend is forced to `Insufficient` and its confidence clamped to
/// `≤ INSUFFICIENT_CONF_CAP`. Otherwise it passes through unchanged.
///
/// `samples_per_bucket` is the count of underlying sessions behind each bucket,
/// in the same order as `trend.points`; a count mismatch is treated as sparse
/// (fail closed — never assert a trend whose support we can't account for).
pub fn gate_trend(mut trend: TraitTrend, samples_per_bucket: &[usize]) -> TraitTrend {
    let too_few_buckets = samples_per_bucket.len() < 3;
    let count_mismatch = samples_per_bucket.len() != trend.points.len();
    let sparse_bucket = samples_per_bucket.iter().any(|&n| n < MIN_SAMPLES_PER_BUCKET);

    if too_few_buckets || count_mismatch || sparse_bucket {
        trend.direction = TrendDir::Insufficient;
        if trend.confidence > INSUFFICIENT_CONF_CAP {
            trend.confidence = INSUFFICIENT_CONF_CAP;
        }
    }
    trend
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dossier::types::{TraitTrend, TrendDir, TrendPoint};
    use crate::ir::EvidenceRef;

    fn ev(id: &str) -> EvidenceRef {
        EvidenceRef {
            session_id: id.into(),
            turn_id: None,
            event_id: None,
            quote: None,
            source_path: None,
        }
    }

    fn claim(n_evidence: usize, confidence: f64) -> Claim {
        Claim {
            text: "x".into(),
            confidence,
            // Start optimistically asserted so we prove the guard *downgrades*.
            status: ClaimStatus::Asserted,
            evidence: (0..n_evidence).map(|i| ev(&format!("s{i}"))).collect(),
        }
    }

    fn trend(n_points: usize, confidence: f64) -> TraitTrend {
        TraitTrend {
            trait_key: "verification_present".into(),
            direction: TrendDir::Improving,
            points: (0..n_points)
                .map(|i| TrendPoint { bucket: format!("2026-W{i}"), value: i as f64 })
                .collect(),
            confidence,
        }
    }

    /// 2 evidence (< 3): downgraded to Emerging, confidence clamped to ≤ 0.5.
    #[test]
    fn two_evidence_becomes_emerging_and_clamps_confidence() {
        let gated = gate_claim(claim(2, 0.9));
        assert_eq!(gated.status, ClaimStatus::Emerging);
        assert!(gated.confidence <= 0.5, "got {}", gated.confidence);
    }

    /// An already-low confidence on an emerging claim is not raised.
    #[test]
    fn emerging_keeps_lower_confidence() {
        let gated = gate_claim(claim(1, 0.2));
        assert_eq!(gated.status, ClaimStatus::Emerging);
        assert_eq!(gated.confidence, 0.2);
    }

    /// 3 evidence (== threshold): asserted, confidence untouched.
    #[test]
    fn three_evidence_is_asserted_and_keeps_confidence() {
        let gated = gate_claim(claim(3, 0.85));
        assert_eq!(gated.status, ClaimStatus::Asserted);
        assert_eq!(gated.confidence, 0.85);
    }

    /// gate_claims maps the guard across the batch.
    #[test]
    fn gate_claims_applies_per_claim() {
        let out = gate_claims(vec![claim(3, 0.9), claim(1, 0.9)]);
        assert_eq!(out[0].status, ClaimStatus::Asserted);
        assert_eq!(out[1].status, ClaimStatus::Emerging);
        assert!(out[1].confidence <= 0.5);
    }

    /// A trend with a sparse bucket (one bucket has 2 < 3 samples) → Insufficient,
    /// confidence clamped to ≤ 0.4.
    #[test]
    fn sparse_bucket_forces_insufficient() {
        let gated = gate_trend(trend(3, 0.9), &[5, 2, 4]);
        assert_eq!(gated.direction, TrendDir::Insufficient);
        assert!(gated.confidence <= 0.4, "got {}", gated.confidence);
    }

    /// Fewer than 3 buckets → Insufficient regardless of per-bucket counts.
    #[test]
    fn too_few_buckets_forces_insufficient() {
        let gated = gate_trend(trend(2, 0.9), &[9, 9]);
        assert_eq!(gated.direction, TrendDir::Insufficient);
        assert!(gated.confidence <= 0.4);
    }

    /// A healthy trend (3 buckets, each ≥ 3 samples) passes through unchanged.
    #[test]
    fn healthy_trend_is_unchanged() {
        let gated = gate_trend(trend(3, 0.83), &[4, 3, 5]);
        assert_eq!(gated.direction, TrendDir::Improving);
        assert_eq!(gated.confidence, 0.83);
    }

    /// A sample-count/points mismatch fails closed to Insufficient.
    #[test]
    fn count_mismatch_fails_closed() {
        // 3 points but only 2 sample counts provided.
        let gated = gate_trend(trend(3, 0.9), &[4, 5]);
        assert_eq!(gated.direction, TrendDir::Insufficient);
    }
}
