//! DOSSIER Phase 5b — profile synthesis + degradation (spec §5, §11, §12, §16).
//!
//! Assembles the final [`Profile`] from the deterministic substrate every
//! earlier phase produced (the [`WindowAggregate`], the [`EfficiencyScore`], the
//! archetype/trajectory classifiers, and the windowed detector findings).
//!
//! Two paths, one contract:
//!   * [`detector_only_profile`] — a FULLY deterministic, no-LLM profile. This is
//!     a first-class product surface, not a fallback stub: it builds all seven
//!     [`crate::dossier::types::DIMENSION_KEYS`] dimensions from signals alone, so
//!     WARDEN ships a real, useful profile even with no brain API key configured.
//!   * [`synthesize_profile`] — the LLM-ENRICHED path. It starts from the
//!     deterministic substrate, then lets an injected `synth` closure replace
//!     narratives/claims with model-written prose, mapping the model's cited
//!     `session_id`s back to [`EvidenceRef`]s. Every claim is run through
//!     [`crate::dossier::proof::gate_claims`] before it lands, so the LLM can
//!     never over-assert. It can only ever ADD to the substrate — it never
//!     returns fewer than seven dimensions.
//!
//! [`synthesize_profile_via_brain`] is the glue: brain unavailable OR any
//! network/parse error → fall back to [`detector_only_profile`]. It NEVER
//! propagates an error — it always hands back a `Profile`.
//!
//! PRIVACY (hard, non-negotiable, spec §19): the only thing that crosses the
//! network is the distilled context built in [`build_llm_context`] — aggregate
//! numbers, family sub-scores, archetype/trajectory summaries, length-capped
//! rollup digests, and an evidence MENU of finding titles + their `session_id`s.
//! Raw transcripts are NEVER sent.

use std::collections::HashSet;
use std::future::Future;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::brain::Brain;
use crate::dossier::aggregate::WindowAggregate;
use crate::dossier::efficiency::EfficiencyScore;
use crate::dossier::proof::gate_claims;
use crate::dossier::scope::Window;
use crate::dossier::summarize::{ProjectSummary, WeekSummary};
use crate::dossier::types::{
    Claim, ClaimStatus, Leak, Profile, ProfileDimension, ProjectArchetype, TraitTrend, TrendDir,
    DIMENSION_KEYS,
};
use crate::ir::{EvidenceRef, Finding};

/// A strength is asserted as "strong" when its family sub-score clears this bar.
const STRENGTH_SUB_SCORE: f64 = 0.65;
/// At most this many ranked leaks (spec §5 "where you lose" top-5).
const MAX_RANKED_LEAKS: usize = 5;
/// At most this many holes surfaced as claims (keep the dimension readable).
const MAX_HOLE_CLAIMS: usize = 5;

/// Everything synthesis needs, gathered by Phase 6 and handed in as one value.
///
/// This is the seam between the deterministic pipeline and the (optional) LLM
/// layer: the aggregator, scorer, archetype/trajectory classifiers and the
/// windowed detector all deposit their outputs here, and synthesis reads ONLY
/// from this struct (no store, no clock, no network of its own).
pub struct SynthesisInputs {
    pub window: Window,
    pub data_hash: String,
    pub session_count: u32,
    pub aggregate: WindowAggregate,
    pub efficiency: EfficiencyScore,
    pub archetypes: Vec<ProjectArchetype>,
    pub trajectory: Vec<TraitTrend>,
    /// Windowed detector findings — the evidence source for holes/leaks.
    pub findings: Vec<Finding>,
    pub week_summaries: Vec<WeekSummary>,
    pub project_summaries: Vec<ProjectSummary>,
}

/// Build the fully deterministic, no-LLM profile — the degradation path AND the
/// substrate the LLM enriches.
///
/// Every one of the seven [`DIMENSION_KEYS`] dimensions is produced from signals
/// alone, and every [`Claim`] is run through [`gate_claims`] so the honesty bar
/// (≥3 cited sessions ⇒ `Asserted`, else `Emerging`) is enforced here too — a
/// strength derived from a family score carries no evidence, so it correctly
/// lands as `Emerging`.
pub fn detector_only_profile(inputs: &SynthesisInputs, generated_at: DateTime<Utc>) -> Profile {
    // --- ranked leaks: findings by est_cost_tokens desc, top N, ranked 1..=n ---
    let mut leak_findings: Vec<&Finding> = inputs.findings.iter().collect();
    leak_findings.sort_by(|a, b| {
        b.est_cost_tokens
            .cmp(&a.est_cost_tokens)
            // Deterministic tiebreak so equal-cost leaks never reorder run-to-run.
            .then_with(|| a.pattern_id.cmp(&b.pattern_id))
    });
    let ranked_leaks: Vec<Leak> = leak_findings
        .iter()
        .take(MAX_RANKED_LEAKS)
        .enumerate()
        .map(|(i, f)| Leak {
            rank: (i + 1) as u32,
            title: f.title.clone(),
            est_cost_tokens: f.est_cost_tokens,
            est_cost_minutes: f.est_cost_minutes,
            evidence: f.evidence.clone(),
        })
        .collect();

    let dimensions = vec![
        dim_orchestration_style(inputs),
        dim_signature_patterns(inputs),
        dim_holes(inputs),
        dim_strengths(inputs),
        dim_where_you_lose(inputs, &ranked_leaks),
        dim_project_archetypes(inputs),
        dim_trajectory(inputs),
    ];
    debug_assert_eq!(dimensions.len(), DIMENSION_KEYS.len());

    Profile {
        window: inputs.window,
        generated_at,
        data_hash: inputs.data_hash.clone(),
        rubric_version: crate::dossier::rubric::RUBRIC_VERSION.to_string(),
        efficiency: inputs.efficiency.clone(),
        dimensions,
        ranked_leaks,
        archetypes: inputs.archetypes.clone(),
        trajectory: inputs.trajectory.clone(),
        session_count: inputs.session_count,
        detector_only: true,
    }
}

/// Look up a family sub-score by key (0.0 when the family is absent).
fn family_sub_score(inputs: &SynthesisInputs, key: &str) -> f64 {
    inputs
        .efficiency
        .families
        .iter()
        .find(|f| f.key == key)
        .map(|f| f.sub_score)
        .unwrap_or(0.0)
}

fn d(key: &str, narrative: String, claims: Vec<Claim>) -> ProfileDimension {
    let title = DIMENSION_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, t)| (*t).to_string())
        .unwrap_or_else(|| key.to_string());
    ProfileDimension { key: key.to_string(), title, narrative, claims }
}

/// Dimension 1: templated from delegation / right-sized / context family scores
/// plus the archetype mix.
fn dim_orchestration_style(inputs: &SynthesisInputs) -> ProfileDimension {
    let delegation = family_sub_score(inputs, "delegation_hygiene");
    let right_sized = family_sub_score(inputs, "right_sized_delegation");
    let context = family_sub_score(inputs, "context_discipline");

    let deleg_phrase = if delegation >= 0.65 {
        format!("delegate exploration well ({delegation:.2})")
    } else {
        format!("under-delegate exploration ({delegation:.2})")
    };
    let sizing_phrase = if right_sized >= 0.65 {
        format!("size delegations sensibly ({right_sized:.2})")
    } else {
        format!("mis-size delegations ({right_sized:.2})")
    };
    let context_phrase = if context >= 0.65 {
        format!("keep context disciplined ({context:.2})")
    } else {
        format!("ride context hot ({context:.2})")
    };

    let arch_phrase = archetype_mix_phrase(inputs);
    let narrative = format!(
        "You {deleg_phrase}, {sizing_phrase}, and {context_phrase}. {arch_phrase}"
    );
    d("orchestration_style", narrative, vec![])
}

/// One-line summary of the dominant project archetypes (e.g. "Most work is
/// web_app + infra.").
fn archetype_mix_phrase(inputs: &SynthesisInputs) -> String {
    if inputs.archetypes.is_empty() {
        return "No project archetypes detected yet.".to_string();
    }
    let top: Vec<String> = inputs
        .archetypes
        .iter()
        .take(2)
        .map(|a| a.archetype.clone())
        .collect();
    format!("Most work is {}.", top.join(" + "))
}

/// Dimension 2: recurring patterns from `aggregate.pattern_freq` plus extreme
/// family scores.
fn dim_signature_patterns(inputs: &SynthesisInputs) -> ProfileDimension {
    let mut pats: Vec<&crate::dossier::aggregate::PatternStat> =
        inputs.aggregate.pattern_freq.iter().collect();
    // Recurring first (highest count), deterministic tiebreak on id.
    pats.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.pattern_id.cmp(&b.pattern_id)));
    let recurring: Vec<String> = pats
        .iter()
        .filter(|p| p.count >= 2)
        .map(|p| format!("{} (×{})", p.pattern_id, p.count))
        .collect();

    // Extreme family scores (very strong or very weak) read as signatures.
    let mut extremes: Vec<String> = Vec::new();
    for f in &inputs.efficiency.families {
        if f.sub_score >= 0.8 {
            extremes.push(format!("strong {} ({:.2})", f.key, f.sub_score));
        } else if f.sub_score <= 0.35 {
            extremes.push(format!("weak {} ({:.2})", f.key, f.sub_score));
        }
    }

    let narrative = if recurring.is_empty() && extremes.is_empty() {
        "No strongly recurring signature patterns over this window.".to_string()
    } else {
        let mut parts = Vec::new();
        if !recurring.is_empty() {
            parts.push(format!("Recurring: {}.", recurring.join(", ")));
        }
        if !extremes.is_empty() {
            parts.push(format!("Standout traits: {}.", extremes.join(", ")));
        }
        parts.join(" ")
    };
    d("signature_patterns", narrative, vec![])
}

/// Dimension 3: top findings by severity → one gated [`Claim`] each.
fn dim_holes(inputs: &SynthesisInputs) -> ProfileDimension {
    let mut by_sev: Vec<&Finding> = inputs.findings.iter().collect();
    by_sev.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            // Then by cost desc, then id — fully deterministic ordering.
            .then_with(|| b.est_cost_tokens.cmp(&a.est_cost_tokens))
            .then_with(|| a.pattern_id.cmp(&b.pattern_id))
    });
    let claims: Vec<Claim> = by_sev
        .iter()
        .take(MAX_HOLE_CLAIMS)
        .map(|f| Claim {
            text: f.title.clone(),
            confidence: f.confidence,
            status: ClaimStatus::Asserted, // gate_claims decides; start optimistic.
            evidence: f.evidence.clone(),
        })
        .collect();
    let claims = gate_claims(claims);

    let narrative = if claims.is_empty() {
        "No significant holes surfaced by the detectors over this window.".to_string()
    } else {
        format!(
            "{} recurring weakness{} detected; the costliest is \u{201c}{}\u{201d}.",
            claims.len(),
            if claims.len() == 1 { "" } else { "es" },
            by_sev[0].title
        )
    };
    d("holes", narrative, claims)
}

/// Dimension 4: families with `sub_score >= 0.65` → a strength [`Claim`] each.
/// These are signal-derived (no cited session), so gating marks them `Emerging`.
fn dim_strengths(inputs: &SynthesisInputs) -> ProfileDimension {
    let mut strong: Vec<&crate::dossier::efficiency::FamilyScore> = inputs
        .efficiency
        .families
        .iter()
        .filter(|f| f.sub_score >= STRENGTH_SUB_SCORE)
        .collect();
    // Strongest first, deterministic tiebreak on key.
    strong.sort_by(|a, b| {
        b.sub_score
            .partial_cmp(&a.sub_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    let claims: Vec<Claim> = strong
        .iter()
        .map(|f| Claim {
            text: format!("Strong {}: {:.2}", f.key, f.sub_score),
            confidence: f.sub_score,
            status: ClaimStatus::Asserted, // gated below → Emerging (no evidence).
            evidence: vec![],
        })
        .collect();
    let claims = gate_claims(claims);

    let narrative = if claims.is_empty() {
        "No family scored strongly enough to assert as a strength this window.".to_string()
    } else {
        format!(
            "{} family score{} above the strength bar; shown as emerging until cited.",
            claims.len(),
            if claims.len() == 1 { "" } else { "s" }
        )
    };
    d("strengths", narrative, claims)
}

/// Dimension 5: narrative over the ranked leaks (the `Leak`s themselves live on
/// `Profile.ranked_leaks`).
fn dim_where_you_lose(inputs: &SynthesisInputs, ranked_leaks: &[Leak]) -> ProfileDimension {
    let _ = inputs;
    let narrative = if ranked_leaks.is_empty() {
        "No measurable token/time leaks attributed this window.".to_string()
    } else {
        let total_tokens: u64 = ranked_leaks.iter().map(|l| l.est_cost_tokens).sum();
        format!(
            "Your top {} leak{} cost an estimated {} tokens; the largest is \u{201c}{}\u{201d} (~{} tokens).",
            ranked_leaks.len(),
            if ranked_leaks.len() == 1 { "" } else { "s" },
            total_tokens,
            ranked_leaks[0].title,
            ranked_leaks[0].est_cost_tokens
        )
    };
    // Claims optional here; the quantified Leaks carry the evidence.
    d("where_you_lose", narrative, vec![])
}

/// Dimension 6: narrative over the project archetype mix.
fn dim_project_archetypes(inputs: &SynthesisInputs) -> ProfileDimension {
    let narrative = if inputs.archetypes.is_empty() {
        "No project archetypes classified for this window.".to_string()
    } else {
        let parts: Vec<String> = inputs
            .archetypes
            .iter()
            .map(|a| format!("{} ({})", a.archetype, a.note))
            .collect();
        format!("Across {} archetype(s): {}.", inputs.archetypes.len(), parts.join("; "))
    };
    d("project_archetypes", narrative, vec![])
}

/// Dimension 7: narrative over the per-trait trends.
fn dim_trajectory(inputs: &SynthesisInputs) -> ProfileDimension {
    let narrative = if inputs.trajectory.is_empty() {
        "Not enough history to chart a trajectory yet.".to_string()
    } else {
        let parts: Vec<String> = inputs
            .trajectory
            .iter()
            .map(|t| format!("{} is {}", t.trait_key, trend_word(&t.direction)))
            .collect();
        format!("Trajectory: {}.", parts.join(", "))
    };
    d("trajectory", narrative, vec![])
}

fn trend_word(dir: &TrendDir) -> &'static str {
    match dir {
        TrendDir::Improving => "improving",
        TrendDir::Regressing => "regressing",
        TrendDir::Plateaued => "plateaued",
        TrendDir::Insufficient => "still too sparse to call",
    }
}

/// Build the compact, DISTILLED context handed to the LLM. Privacy-critical:
/// this is the *only* thing that crosses the network, and it carries aggregate
/// numbers, family sub-scores, archetype/trajectory summaries, length-capped
/// rollup digests, and an evidence MENU (finding titles + their `session_id`s).
/// NEVER raw transcripts.
fn build_llm_context(inputs: &SynthesisInputs) -> Value {
    let families: Vec<Value> = inputs
        .efficiency
        .families
        .iter()
        .map(|f| json!({ "key": f.key, "sub_score": f.sub_score, "weight": f.weight }))
        .collect();

    let patterns: Vec<Value> = inputs
        .aggregate
        .pattern_freq
        .iter()
        .map(|p| json!({ "pattern_id": p.pattern_id, "count": p.count, "est_cost_tokens": p.est_cost_tokens }))
        .collect();

    let archetypes: Vec<Value> = inputs
        .archetypes
        .iter()
        .map(|a| json!({ "archetype": a.archetype, "session_count": a.session_count, "note": a.note }))
        .collect();

    let trajectory: Vec<Value> = inputs
        .trajectory
        .iter()
        .map(|t| json!({ "trait_key": t.trait_key, "direction": t.direction, "confidence": t.confidence }))
        .collect();

    // Evidence menu: the model may cite ONLY these session_ids (validated on the
    // way back). Titles are detector-authored, not transcript text.
    let evidence_menu: Vec<Value> = inputs
        .findings
        .iter()
        .map(|f| {
            let sessions: Vec<&str> =
                f.evidence.iter().map(|e| e.session_id.as_str()).collect();
            json!({
                "pattern_id": f.pattern_id,
                "title": f.title,
                "severity": f.severity,
                "est_cost_tokens": f.est_cost_tokens,
                "session_ids": sessions,
            })
        })
        .collect();

    let week_digests: Vec<Value> = inputs
        .week_summaries
        .iter()
        .map(|w| json!({ "iso_week": w.iso_week, "session_count": w.session_count, "digest": w.digest }))
        .collect();
    let project_digests: Vec<Value> = inputs
        .project_summaries
        .iter()
        .map(|p| json!({ "project": p.project, "session_count": p.session_count, "digest": p.digest }))
        .collect();

    json!({
        "window": inputs.window,
        "session_count": inputs.session_count,
        "efficiency_headline": inputs.efficiency.headline,
        "families": families,
        "pattern_freq": patterns,
        "mean_outcome": inputs.aggregate.mean_outcome,
        "archetypes": archetypes,
        "trajectory": trajectory,
        "evidence_menu": evidence_menu,
        "week_digests": week_digests,
        "project_digests": project_digests,
        "dimension_keys": DIMENSION_KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
    })
}

/// The set of `session_id`s the model is allowed to cite — every session that
/// appears in any finding's evidence. Unknown ids in the model output are dropped.
fn known_session_ids(inputs: &SynthesisInputs) -> HashSet<String> {
    let mut set = HashSet::new();
    for f in &inputs.findings {
        for e in &f.evidence {
            set.insert(e.session_id.clone());
        }
    }
    set
}

/// Map model-cited `session_id`s → [`EvidenceRef`]s, dropping any id not in the
/// allowed set. Only the session id survives — turn/event/quote/source are left
/// `None` (the LLM cites sessions, not raw spans).
fn map_evidence(ids: &[String], known: &HashSet<String>) -> Vec<EvidenceRef> {
    ids.iter()
        .filter(|id| known.contains(*id))
        .map(|id| EvidenceRef {
            session_id: id.clone(),
            turn_id: None,
            event_id: None,
            quote: None,
            source_path: None,
        })
        .collect()
}

/// LLM-enriched synthesis (injectable `synth` for tests; no real network here).
///
/// Starts from [`detector_only_profile`] (the substrate) and overlays whatever
/// the model returns: per-dimension narrative + gated claims, and the ranked
/// leaks. The model can only ADD — any of the seven dimensions it omits keeps its
/// deterministic value, so the result is ALWAYS seven dimensions in order.
/// `proposed_weights` are RECORDED only (logged, never applied — v1 pins the
/// rubric weights).
pub async fn synthesize_profile<F, Fut>(
    inputs: &SynthesisInputs,
    generated_at: DateTime<Utc>,
    synth: F,
) -> anyhow::Result<Profile>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = anyhow::Result<Value>>,
{
    let context = build_llm_context(inputs);
    let out = synth(context).await?;

    // Start from the deterministic substrate, then flip the flag.
    let mut profile = detector_only_profile(inputs, generated_at);
    profile.detector_only = false;

    let known = known_session_ids(inputs);

    // --- overlay per-dimension narrative + claims by key ---
    if let Some(dims) = out.get("dimensions").and_then(Value::as_array) {
        for dv in dims {
            let Some(key) = dv.get("key").and_then(Value::as_str) else {
                continue;
            };
            // Ignore any key the contract doesn't recognize (never grows past 7).
            let Some(slot) = profile.dimensions.iter_mut().find(|d| d.key == key) else {
                continue;
            };
            if let Some(narr) = dv.get("narrative").and_then(Value::as_str) {
                slot.narrative = narr.to_string();
            }
            if let Some(claims) = dv.get("claims").and_then(Value::as_array) {
                let mapped: Vec<Claim> = claims
                    .iter()
                    .filter_map(|c| {
                        let text = c.get("text").and_then(Value::as_str)?.to_string();
                        let confidence =
                            c.get("confidence").and_then(Value::as_f64).unwrap_or(0.5);
                        let ids: Vec<String> = c
                            .get("evidence_session_ids")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let evidence = map_evidence(&ids, &known);
                        Some(Claim {
                            text,
                            confidence,
                            // Optimistic; gate_claims is the final authority.
                            status: ClaimStatus::Asserted,
                            evidence,
                        })
                    })
                    .collect();
                slot.claims = gate_claims(mapped);
            }
        }
    }

    // --- overlay ranked leaks when the model supplies them ---
    if let Some(leaks) = out.get("ranked_leaks").and_then(Value::as_array) {
        let mapped: Vec<Leak> = leaks
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let title = l.get("title").and_then(Value::as_str)?.to_string();
                let est_cost_tokens =
                    l.get("est_cost_tokens").and_then(Value::as_u64).unwrap_or(0);
                let est_cost_minutes =
                    l.get("est_cost_minutes").and_then(Value::as_u64).unwrap_or(0);
                let ids: Vec<String> = l
                    .get("evidence_session_ids")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                Some(Leak {
                    rank: (i + 1) as u32,
                    title,
                    est_cost_tokens,
                    est_cost_minutes,
                    evidence: map_evidence(&ids, &known),
                })
            })
            .take(MAX_RANKED_LEAKS)
            .collect();
        if !mapped.is_empty() {
            profile.ranked_leaks = mapped;
        }
    }

    // --- proposed_weights: RECORD only (v1 pins the rubric weights) ---
    if let Some(weights) = out.get("proposed_weights").and_then(Value::as_array) {
        if !weights.is_empty() {
            tracing::info!(
                "dossier synthesis proposed {} weight override(s) (recorded, NOT applied; v1 pins the rubric): {:?}",
                weights.len(),
                weights
            );
        }
    }

    debug_assert_eq!(profile.dimensions.len(), DIMENSION_KEYS.len());
    Ok(profile)
}

/// Brain wrapper + degradation glue. NEVER propagates — always returns a Profile.
///
/// Brain unavailable → [`detector_only_profile`]. Otherwise call the LLM path; on
/// ANY error (network or parse) log it and fall back to the deterministic
/// profile. The product always has a profile to show.
pub async fn synthesize_profile_via_brain(
    inputs: &SynthesisInputs,
    generated_at: DateTime<Utc>,
    brain: &Brain,
) -> Profile {
    if !brain.available() {
        return detector_only_profile(inputs, generated_at);
    }
    match synthesize_profile(inputs, generated_at, |ctx| brain.synthesize_profile(ctx)).await {
        Ok(profile) => profile,
        Err(err) => {
            tracing::warn!(
                "dossier LLM synthesis failed ({err}); degrading to detector-only profile"
            );
            detector_only_profile(inputs, generated_at)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(session: &str) -> EvidenceRef {
        EvidenceRef {
            session_id: session.into(),
            turn_id: Some("t1".into()),
            event_id: None,
            quote: Some("q".into()),
            source_path: None,
        }
    }

    fn finding(
        id: &str,
        title: &str,
        severity: u8,
        cost_tokens: u64,
        confidence: f64,
        evidence: Vec<EvidenceRef>,
    ) -> Finding {
        Finding {
            id: id.into(),
            pattern_id: format!("pat-{id}"),
            title: title.into(),
            severity,
            frequency: 0.5,
            est_cost_tokens: cost_tokens,
            est_cost_minutes: cost_tokens / 6000,
            confidence,
            rationale: "r".into(),
            evidence,
            status: "open".into(),
            verifier_verdict: None,
        }
    }

    fn efficiency_fixture() -> EfficiencyScore {
        use crate::dossier::efficiency::FamilyScore;
        EfficiencyScore {
            headline: 0.6,
            rubric_version: "dossier-rubric-v1".into(),
            families: vec![
                FamilyScore { key: "delegation_hygiene".into(), sub_score: 0.81, weight: 0.15 },
                FamilyScore { key: "context_discipline".into(), sub_score: 0.42, weight: 0.2 },
                FamilyScore { key: "verification_discipline".into(), sub_score: 0.7, weight: 0.2 },
            ],
            session_count: 4,
        }
    }

    fn aggregate_fixture() -> WindowAggregate {
        use crate::dossier::aggregate::PatternStat;
        WindowAggregate {
            window: Window::ThirtyDays,
            session_count: 4,
            by_project: vec![],
            by_week: vec![],
            pattern_freq: vec![
                PatternStat { pattern_id: "no_verification".into(), count: 3, est_cost_tokens: 42_000 },
                PatternStat { pattern_id: "context_overflow".into(), count: 1, est_cost_tokens: 5_000 },
            ],
            mean_outcome: 0.6,
        }
    }

    fn archetype_fixture() -> Vec<ProjectArchetype> {
        vec![
            ProjectArchetype {
                archetype: "web_app".into(),
                projects: vec!["acme-web".into()],
                session_count: 3,
                note: "1 project, 3 sessions".into(),
            },
            ProjectArchetype {
                archetype: "infra".into(),
                projects: vec!["deploy".into()],
                session_count: 1,
                note: "1 project, 1 session".into(),
            },
        ]
    }

    fn trajectory_fixture() -> Vec<TraitTrend> {
        use crate::dossier::types::TrendPoint;
        vec![TraitTrend {
            trait_key: "verification_present".into(),
            direction: TrendDir::Improving,
            points: vec![
                TrendPoint { bucket: "2026-W22".into(), value: 0.3 },
                TrendPoint { bucket: "2026-W23".into(), value: 0.6 },
            ],
            confidence: 0.5,
        }]
    }

    fn inputs_fixture() -> SynthesisInputs {
        SynthesisInputs {
            window: Window::ThirtyDays,
            data_hash: "deadbeef".into(),
            session_count: 4,
            aggregate: aggregate_fixture(),
            efficiency: efficiency_fixture(),
            archetypes: archetype_fixture(),
            trajectory: trajectory_fixture(),
            findings: vec![
                finding("f1", "Skips verification after edits", 5, 42_000, 0.9, vec![ev("s1"), ev("s2"), ev("s3")]),
                finding("f2", "Re-reads files already in context", 3, 18_000, 0.7, vec![ev("s4")]),
                finding("f3", "Over-broad agent delegation", 2, 9_000, 0.6, vec![ev("s5"), ev("s6")]),
            ],
            week_summaries: vec![],
            project_summaries: vec![],
        }
    }

    fn now() -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap()
    }

    #[test]
    fn detector_only_builds_seven_dimensions() {
        let inputs = inputs_fixture();
        let p = detector_only_profile(&inputs, now());

        // Exactly the seven canonical dimensions, in DIMENSION_KEYS order.
        let keys: Vec<&str> = p.dimensions.iter().map(|d| d.key.as_str()).collect();
        let expected: Vec<&str> = DIMENSION_KEYS.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, expected, "dimensions must be the 7 keys in order");

        assert!(p.detector_only, "detector-only path sets the flag");
        assert_eq!(p.rubric_version, "dossier-rubric-v1");
        assert_eq!(p.session_count, 4);
        assert_eq!(p.data_hash, "deadbeef");

        // ranked_leaks ranked by cost desc, capped, ranks assigned 1..=n.
        assert_eq!(p.ranked_leaks.len(), 3);
        assert_eq!(p.ranked_leaks[0].est_cost_tokens, 42_000);
        assert_eq!(p.ranked_leaks[0].rank, 1);
        assert_eq!(p.ranked_leaks[1].est_cost_tokens, 18_000);
        assert_eq!(p.ranked_leaks[1].rank, 2);
        assert_eq!(p.ranked_leaks[2].rank, 3);

        // strengths with no evidence must be Emerging (gated), never Asserted.
        let strengths = p
            .dimensions
            .iter()
            .find(|d| d.key == "strengths")
            .expect("strengths dimension present");
        assert!(!strengths.claims.is_empty(), "a >=0.65 family yields a strength claim");
        for c in &strengths.claims {
            assert_eq!(c.status, ClaimStatus::Emerging, "no-evidence strength must be Emerging");
        }

        // holes: top finding (3 evidence) clears the bar → Asserted.
        let holes = p.dimensions.iter().find(|d| d.key == "holes").unwrap();
        let top = holes.claims.first().expect("a hole claim");
        assert_eq!(top.status, ClaimStatus::Asserted);

        // trajectory dimension mirrored onto Profile.trajectory.
        assert_eq!(p.trajectory.len(), 1);
        assert_eq!(p.trajectory[0].trait_key, "verification_present");
    }

    #[tokio::test]
    async fn synthesize_maps_evidence_and_gates() {
        let inputs = inputs_fixture();
        // The model cites 3 known session_ids for one claim (→ Asserted), 1 for
        // another (→ Emerging), and one UNKNOWN id which must be dropped. It only
        // supplies 3 of the 7 dimensions; the rest must be filled from substrate.
        let synth = |_ctx: Value| async move {
            Ok(json!({
                "dimensions": [
                    {
                        "key": "holes",
                        "narrative": "LLM holes narrative.",
                        "claims": [
                            { "text": "Strongly skips verification", "confidence": 0.95,
                              "evidence_session_ids": ["s1", "s2", "s3"] },
                            { "text": "Sometimes re-reads files", "confidence": 0.9,
                              "evidence_session_ids": ["s4", "ghost-unknown"] }
                        ]
                    },
                    { "key": "strengths", "narrative": "LLM strengths.", "claims": [] },
                    { "key": "trajectory", "narrative": "LLM trajectory.", "claims": [] }
                ],
                "ranked_leaks": [
                    { "title": "Verification gap", "est_cost_tokens": 42000,
                      "est_cost_minutes": 7, "evidence_session_ids": ["s1"] }
                ],
                "proposed_weights": [
                    { "key": "verification_discipline", "weight": 0.3, "rationale": "ignored" }
                ]
            }))
        };

        let p = synthesize_profile(&inputs, now(), synth).await.unwrap();

        assert!(!p.detector_only, "LLM path clears the flag");

        // All seven dimensions present even though the JSON supplied only three.
        let keys: Vec<&str> = p.dimensions.iter().map(|d| d.key.as_str()).collect();
        let expected: Vec<&str> = DIMENSION_KEYS.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, expected, "missing dimensions filled from substrate, still 7 in order");

        let holes = p.dimensions.iter().find(|d| d.key == "holes").unwrap();
        assert_eq!(holes.narrative, "LLM holes narrative.");
        assert_eq!(holes.claims.len(), 2);

        // 3 known ids → Asserted, evidence mapped.
        let c0 = &holes.claims[0];
        assert_eq!(c0.status, ClaimStatus::Asserted);
        assert_eq!(c0.evidence.len(), 3);

        // 1 known id (+1 unknown dropped) → Emerging, exactly 1 evidence.
        let c1 = &holes.claims[1];
        assert_eq!(c1.status, ClaimStatus::Emerging);
        assert_eq!(c1.evidence.len(), 1, "unknown session_id dropped");
        assert_eq!(c1.evidence[0].session_id, "s4");
        assert!(c1.confidence <= 0.5, "emerging confidence clamped");
    }

    #[tokio::test]
    async fn via_brain_unavailable_degrades() {
        use crate::store::Store;
        // A Brain with no env key/url is unavailable → must degrade, no network.
        let brain = Brain::new(Store::memory().unwrap());
        assert!(!brain.available());
        let inputs = inputs_fixture();
        let p = synthesize_profile_via_brain(&inputs, now(), &brain).await;
        assert!(p.detector_only, "degraded path returns a detector-only profile");
        assert_eq!(p.dimensions.len(), 7);
    }
}
