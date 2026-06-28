//! DOSSIER profile contract — the deterministic shape every later phase fills.
//!
//! This is the *output* type of the whole DOSSIER pipeline: the synthesis layer
//! (Phase 5b), the by-proof guard ([`super::proof`]), the trajectory classifier
//! ([`super::trajectory`]) and the archetype heuristic ([`super::archetype`])
//! all produce values that land here, and Phase 6 persists/serves exactly this
//! struct over IPC. The field set is frozen — downstream phases depend on it —
//! so changes here ripple through every consumer.
//!
//! Everything is pure data: `serde`-(de)serializable, no behavior, no I/O. The
//! one invariant worth stating is that a [`Profile`] round-trips through
//! `serde_json` unchanged (asserted in tests), because the cache layer stores it
//! as JSON keyed by `(window, data_hash)`.

use crate::dossier::efficiency::EfficiencyScore;
use crate::dossier::scope::Window;
use crate::ir::EvidenceRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The full operator profile for one [`Window`].
///
/// `data_hash` fingerprints the scoped session set so the cache can detect when
/// a re-roll is needed; `rubric_version` pins the scoring rubric the
/// `efficiency` number was computed under (a score only compares to another with
/// the same version). `detector_only` flags a degraded profile built without the
/// LLM synthesis layer (detector signals only) so the UI can be honest about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub window: Window,
    pub generated_at: DateTime<Utc>,
    pub data_hash: String,
    pub rubric_version: String,
    pub efficiency: EfficiencyScore,
    pub dimensions: Vec<ProfileDimension>,
    pub ranked_leaks: Vec<Leak>,
    pub archetypes: Vec<ProjectArchetype>,
    pub trajectory: Vec<TraitTrend>,
    pub session_count: u32,
    pub detector_only: bool,
}

/// One of the seven profile dimensions (see [`DIMENSION_KEYS`]): a stable `key`,
/// a human `title`, a synthesized `narrative`, and the evidence-backed `claims`
/// that compose it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDimension {
    pub key: String,
    pub title: String,
    pub narrative: String,
    pub claims: Vec<Claim>,
}

/// Whether a [`Claim`] cleared the by-proof evidence threshold (`Asserted`) or
/// fell short and is shown tentatively (`Emerging`). Gating lives in
/// [`super::proof::gate_claim`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Asserted,
    Emerging,
}

/// A single evidence-cited statement inside a [`ProfileDimension`]. `confidence`
/// is `0..1`; `evidence` are the resolvable [`EvidenceRef`]s that back it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub text: String,
    pub confidence: f64,
    pub status: ClaimStatus,
    pub evidence: Vec<EvidenceRef>,
}

/// A quantified, ranked "where you lose" item: estimated wasted tokens and
/// minutes, cited to the sessions that leaked them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leak {
    pub rank: u32,
    pub title: String,
    pub est_cost_tokens: u64,
    pub est_cost_minutes: u64,
    pub evidence: Vec<EvidenceRef>,
}

/// A project-archetype grouping (dimension 6): the archetype label, the member
/// projects, how many sessions fell under it, and a short human note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectArchetype {
    pub archetype: String,
    pub projects: Vec<String>,
    pub session_count: u32,
    pub note: String,
}

/// Direction of a per-trait trend over time (dimension 7). `Insufficient` is the
/// by-proof escape hatch when the series is too sparse to classify (see
/// [`super::proof::gate_trend`] / [`super::trajectory::classify_trend`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TrendDir {
    Improving,
    Regressing,
    Plateaued,
    Insufficient,
}

/// One sampled point on a trend line: a bucket label (e.g. an ISO week) and the
/// already-oriented value (higher = better) for that bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub bucket: String,
    pub value: f64,
}

/// A classified trend for one trait: its key, the [`TrendDir`], the underlying
/// points, and a `0..1` confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitTrend {
    pub trait_key: String,
    pub direction: TrendDir,
    pub points: Vec<TrendPoint>,
    pub confidence: f64,
}

/// The seven canonical dimensions, as `(key, title)` pairs, in display order.
/// The `key`s are the stable handles every dimension producer and the frontend
/// agree on; the `title`s are the human labels.
pub const DIMENSION_KEYS: [(&str, &str); 7] = [
    ("orchestration_style", "Orchestration style"),
    ("signature_patterns", "Signature patterns"),
    ("holes", "Holes & mistakes"),
    ("strengths", "Strengths"),
    ("where_you_lose", "Where you lose"),
    ("project_archetypes", "Project archetypes"),
    ("trajectory", "Trajectory"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dossier::efficiency::{EfficiencyScore, FamilyScore};
    use crate::ir::EvidenceRef;
    use chrono::TimeZone;

    fn fixture_profile() -> Profile {
        let ev = EvidenceRef {
            session_id: "sess-1".into(),
            turn_id: Some("turn-3".into()),
            event_id: None,
            quote: Some("ran `cargo build` with no verification".into()),
            source_path: Some(std::path::PathBuf::from("/tmp/sess-1.jsonl")),
        };
        Profile {
            window: Window::ThirtyDays,
            generated_at: Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap(),
            data_hash: "deadbeef".into(),
            rubric_version: "dossier-rubric-v1".into(),
            efficiency: EfficiencyScore {
                headline: 0.62,
                rubric_version: "dossier-rubric-v1".into(),
                families: vec![FamilyScore {
                    key: "context_discipline".into(),
                    sub_score: 0.7,
                    weight: 0.2,
                }],
                session_count: 4,
            },
            dimensions: vec![ProfileDimension {
                key: "holes".into(),
                title: "Holes & mistakes".into(),
                narrative: "Repeatedly claims completion without running tests.".into(),
                claims: vec![Claim {
                    text: "Skips verification on multi-tool sessions".into(),
                    confidence: 0.8,
                    status: ClaimStatus::Asserted,
                    evidence: vec![ev.clone()],
                }],
            }],
            ranked_leaks: vec![Leak {
                rank: 1,
                title: "Re-reading files already in context".into(),
                est_cost_tokens: 42_000,
                est_cost_minutes: 7,
                evidence: vec![ev.clone()],
            }],
            archetypes: vec![ProjectArchetype {
                archetype: "web_app".into(),
                projects: vec!["acme-web-app".into()],
                session_count: 2,
                note: "1 project, 2 sessions".into(),
            }],
            trajectory: vec![TraitTrend {
                trait_key: "verification_present".into(),
                direction: TrendDir::Improving,
                points: vec![
                    TrendPoint { bucket: "2026-W24".into(), value: 0.2 },
                    TrendPoint { bucket: "2026-W25".into(), value: 0.5 },
                    TrendPoint { bucket: "2026-W26".into(), value: 0.8 },
                ],
                confidence: 0.5,
            }],
            session_count: 4,
            detector_only: false,
        }
    }

    /// A `Profile` survives a `serde_json` round-trip unchanged. `EvidenceRef`
    /// does not derive `PartialEq`, so we compare the canonical JSON encodings
    /// rather than the structs — equal JSON ⇒ structurally identical profile,
    /// which is exactly what the cache layer relies on.
    #[test]
    fn profile_round_trips_through_serde_json() {
        let p = fixture_profile();
        let json = serde_json::to_string(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json, json2, "profile JSON must be stable across a round-trip");
    }

    /// The window enum serializes to the frozen wire string, embedded in the
    /// profile (guards against a silent `Window` rename drifting the contract).
    #[test]
    fn profile_serializes_window_as_wire_string() {
        let json = serde_json::to_string(&fixture_profile()).unwrap();
        assert!(json.contains("\"window\":\"30d\""), "got: {json}");
    }

    /// Status round-trips as snake_case (`asserted` / `emerging`).
    #[test]
    fn claim_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ClaimStatus::Asserted).unwrap(),
            "\"asserted\""
        );
        assert_eq!(
            serde_json::to_string(&ClaimStatus::Emerging).unwrap(),
            "\"emerging\""
        );
    }

    /// Trend direction round-trips as snake_case, including `insufficient`.
    #[test]
    fn trend_dir_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&TrendDir::Insufficient).unwrap(),
            "\"insufficient\""
        );
        assert_eq!(
            serde_json::to_string(&TrendDir::Plateaued).unwrap(),
            "\"plateaued\""
        );
    }

    /// The seven dimension keys are present, ordered, and unique.
    #[test]
    fn dimension_keys_are_the_seven_in_order() {
        let keys: Vec<&str> = DIMENSION_KEYS.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                "orchestration_style",
                "signature_patterns",
                "holes",
                "strengths",
                "where_you_lose",
                "project_archetypes",
                "trajectory",
            ]
        );
        let mut uniq = keys.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 7, "dimension keys must be unique");
    }
}
