//! Deterministic trend classification (dimension 7 — trajectory).
//!
//! Given an already-oriented series of [`TrendPoint`]s (higher value = "better"
//! — the caller flips any "lower is better" trait before calling), decide
//! whether the trait is [`TrendDir::Improving`], [`TrendDir::Regressing`],
//! [`TrendDir::Plateaued`], or — when there's too little data —
//! [`TrendDir::Insufficient`].
//!
//! The classifier is a plain ordinary-least-squares fit: regress the point
//! values against their index (`x = 0, 1, 2, …`) and read the sign of the slope.
//! A fixed dead-band [`EPS`] keeps tiny wiggles from registering as a trend, so a
//! near-flat line reads `Plateaued` rather than flickering Improving/Regressing.
//! Confidence scales with how many points we have, saturating at
//! [`CONF_SATURATION`] buckets.
//!
//! Pure and deterministic: the same series always yields the same [`TraitTrend`]
//! (no corpus, no clock, no RNG). This is the heuristic floor; the by-proof guard
//! ([`super::proof::gate_trend`]) still has the final say on whether a classified
//! trend has enough per-bucket support to be shown as anything but Insufficient.

use crate::dossier::types::{TraitTrend, TrendDir, TrendPoint};

/// Minimum points required to attempt a classification.
const MIN_POINTS: usize = 3;

/// Slope dead-band on the already-normalized `0..1` values: `|slope| <= EPS`
/// reads as flat. A fixed small epsilon (per the design) — not data-derived —
/// so classification is stable and reproducible.
const EPS: f64 = 0.01;

/// Number of buckets at which confidence saturates to `1.0`
/// (`confidence = clamp(points / CONF_SATURATION, 0..1)`).
const CONF_SATURATION: f64 = 6.0;

/// Confidence ceiling for an `Insufficient` classification.
const INSUFFICIENT_CONF_CAP: f64 = 0.4;

/// Ordinary-least-squares slope of `values` against their index. `None` when
/// there are fewer than 2 points or every x is identical (degenerate fit).
fn ols_slope(values: &[f64]) -> Option<f64> {
    let n = values.len();
    if n < 2 {
        return None;
    }
    let nf = n as f64;
    let mean_x = (0..n).map(|i| i as f64).sum::<f64>() / nf;
    let mean_y = values.iter().sum::<f64>() / nf;

    let mut num = 0.0;
    let mut den = 0.0;
    for (i, &y) in values.iter().enumerate() {
        let dx = i as f64 - mean_x;
        num += dx * (y - mean_y);
        den += dx * dx;
    }
    if den == 0.0 {
        None
    } else {
        Some(num / den)
    }
}

/// Classify a per-trait series into a [`TraitTrend`].
///
/// `< MIN_POINTS` points → `Insufficient` (confidence capped at
/// [`INSUFFICIENT_CONF_CAP`]). Otherwise fit an OLS slope over the values:
/// `slope > +EPS` → `Improving`, `slope < -EPS` → `Regressing`, else
/// `Plateaued`. Confidence = `clamp(points / CONF_SATURATION, 0..=1)`.
pub fn classify_trend(trait_key: &str, points: Vec<TrendPoint>) -> TraitTrend {
    if points.len() < MIN_POINTS {
        let confidence = (points.len() as f64 / CONF_SATURATION).min(INSUFFICIENT_CONF_CAP);
        return TraitTrend {
            trait_key: trait_key.to_string(),
            direction: TrendDir::Insufficient,
            points,
            confidence,
        };
    }

    let values: Vec<f64> = points.iter().map(|p| p.value).collect();
    let direction = match ols_slope(&values) {
        Some(slope) if slope > EPS => TrendDir::Improving,
        Some(slope) if slope < -EPS => TrendDir::Regressing,
        _ => TrendDir::Plateaued,
    };

    let confidence = (points.len() as f64 / CONF_SATURATION).clamp(0.0, 1.0);

    TraitTrend {
        trait_key: trait_key.to_string(),
        direction,
        points,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(values: &[f64]) -> Vec<TrendPoint> {
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| TrendPoint { bucket: format!("2026-W{i}"), value: v })
            .collect()
    }

    /// A cleanly rising series classifies Improving.
    #[test]
    fn rising_series_is_improving() {
        let t = classify_trend("verification_present", pts(&[0.2, 0.4, 0.6, 0.8]));
        assert_eq!(t.direction, TrendDir::Improving);
        // 4 points / 6 ≈ 0.667
        assert!((t.confidence - 4.0 / 6.0).abs() < 1e-9, "got {}", t.confidence);
    }

    /// A cleanly falling series classifies Regressing.
    #[test]
    fn falling_series_is_regressing() {
        let t = classify_trend("tool_error_rate", pts(&[0.8, 0.6, 0.4]));
        assert_eq!(t.direction, TrendDir::Regressing);
    }

    /// A near-flat jittery series stays inside the dead-band → Plateaued.
    #[test]
    fn flat_series_is_plateaued() {
        let t = classify_trend("planning_ratio", pts(&[0.5, 0.51, 0.49, 0.5]));
        assert_eq!(t.direction, TrendDir::Plateaued);
    }

    /// Fewer than three points cannot be a trend → Insufficient, confidence ≤ 0.4.
    #[test]
    fn two_points_is_insufficient() {
        let t = classify_trend("cache_read_ratio", pts(&[0.2, 0.9]));
        assert_eq!(t.direction, TrendDir::Insufficient);
        assert!(t.confidence <= 0.4, "got {}", t.confidence);
    }

    /// Empty series is Insufficient with zero confidence (no panic on no data).
    #[test]
    fn empty_series_is_insufficient() {
        let t = classify_trend("x", pts(&[]));
        assert_eq!(t.direction, TrendDir::Insufficient);
        assert_eq!(t.confidence, 0.0);
    }

    /// Confidence saturates at 1.0 for 6+ buckets.
    #[test]
    fn confidence_saturates_at_six_points() {
        let t = classify_trend("x", pts(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7]));
        assert_eq!(t.confidence, 1.0);
        assert_eq!(t.direction, TrendDir::Improving);
    }

    /// A slope just inside the dead-band reads Plateaued, just outside reads
    /// Improving — pins the EPS boundary behavior.
    #[test]
    fn eps_dead_band_boundary() {
        // slope exactly 0.005 (< EPS=0.01) over 3 points → Plateaued.
        let inside = classify_trend("x", pts(&[0.50, 0.505, 0.51]));
        assert_eq!(inside.direction, TrendDir::Plateaued);
        // slope 0.05 (> EPS) → Improving.
        let outside = classify_trend("x", pts(&[0.50, 0.55, 0.60]));
        assert_eq!(outside.direction, TrendDir::Improving);
    }
}
