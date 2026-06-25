use crate::ir::*;
use crate::store::Store;
use crate::util::{stable_id, truncate_chars};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DetectorHit {
    pub pattern_id: &'static str,
    pub title: &'static str,
    pub severity: u8,
    pub affected: Vec<FeatureVector>,
    pub rationale: String,
}

pub fn detect(profile: &CompetenceProfile, features: &[FeatureVector]) -> Vec<DetectorHit> {
    let mut hits = Vec::new();
    let mut add = |pattern_id: &'static str,
                   title: &'static str,
                   severity: u8,
                   affected: Vec<FeatureVector>,
                   rationale: String| {
        if affected.is_empty() {
            return;
        }
        hits.push(DetectorHit {
            pattern_id,
            title,
            severity,
            affected,
            rationale,
        });
    };

    // Each per-session detector filter delegates to `session_trips_pattern` — the
    // SINGLE source of truth for "did pattern P fire on this session's features?".
    // Living-Habits Piece 3 labels each in-window session clean/slip with the SAME
    // predicate, so the streak's clean/slip series can never drift from `detect()`.
    add("CONTEXT_BLOAT","Context bloat",4,features.iter().filter(|f|session_trips_pattern("CONTEXT_BLOAT",f)).cloned().collect(),"Search and file-reading tools are repeatedly used in the main Claude context, increasing token burn before useful edits.".into());
    add("NO_DELEGATION","No delegation",4,features.iter().filter(|f|session_trips_pattern("NO_DELEGATION",f)).cloned().collect(),"Search-heavy sessions rarely use Claude Code Task subagents, so discovery work competes with implementation context.".into());
    add("UNVERIFIED_COMPLETION","Unverified completion",5,features.iter().filter(|f|session_trips_pattern("UNVERIFIED_COMPLETION",f)).cloned().collect(),"Sessions reach substantial tool use without an observed build/test/verification command before completion.".into());
    add("IGNORED_TOOL_ERROR","Ignored tool errors",4,features.iter().filter(|f|session_trips_pattern("IGNORED_TOOL_ERROR",f)).cloned().collect(),"Tool errors appear at a high rate and are not consistently followed by clear verification/correction signals.".into());
    add(
        "VAGUE_PROMPT",
        "Vague prompts",
        3,
        features
            .iter()
            .filter(|f| session_trips_pattern("VAGUE_PROMPT", f))
            .cloned()
            .collect(),
        "Some sessions start from underspecified prompts and need corrective follow-up turns."
            .into(),
    );
    add("WHACK_A_MOLE","Whack-a-mole loops",4,features.iter().filter(|f|session_trips_pattern("WHACK_A_MOLE",f)).cloned().collect(),"Repeated edits or repeated failing commands suggest symptom-patching loops instead of a reset to root cause.".into());
    add("CACHE_COLD_RESTARTS","Cache-cold restarts",3,features.iter().filter(|f|session_trips_pattern("CACHE_COLD_RESTARTS",f)).cloned().collect(),"High-token sessions show low cache-read ratios, suggesting expensive cold context restarts.".into());
    if profile
        .repeated_explanation_clusters
        .iter()
        .any(|c| c.count >= 3)
    {
        add("REPEATED_EXPLANATION","Repeated explanation",3,features.iter().take(20).cloned().collect(),"Multiple sessions cluster around the same project context; durable project memory may reduce re-explanation.".into());
    }

    hits
}

/// Does pattern `pattern_id` fire on a SINGLE session's `fv`? This is the lone
/// definition of every per-session detector threshold — `detect()` filters through
/// it, and Living-Habits Piece 3 labels each in-window session clean/slip through
/// it, so the streak's clean/slip series is the same predicate `detect()` uses and
/// the two can never diverge. A session where the pattern fired is a SLIP; one where
/// it could apply and did NOT fire is CLEAN.
///
/// `REPEATED_EXPLANATION` is a profile-level pattern (it keys off cross-session
/// `repeated_explanation_clusters`, not one `FeatureVector`), so it has no single-
/// session condition and returns `false` here — it is not a per-session habit and
/// does not participate in the streak. Unknown ids also return `false`.
pub fn session_trips_pattern(pattern_id: &str, fv: &FeatureVector) -> bool {
    let f = fv;
    match pattern_id {
        "CONTEXT_BLOAT" => {
            f.search_in_main_context >= 8
                || (f.search_in_main_context >= 4 && f.context_saturation_peak > 0.35)
        }
        "NO_DELEGATION" => {
            f.tool_call_count >= 12 && f.subagent_spawn_count == 0 && f.search_in_main_context >= 3
        }
        "UNVERIFIED_COMPLETION" => f.tool_call_count >= 4 && !f.verification_present,
        "IGNORED_TOOL_ERROR" => f.ignored_error_count > 0 || f.tool_error_rate > 0.25,
        "VAGUE_PROMPT" => {
            f.prompt_specificity > 0.0 && f.prompt_specificity < 0.28 && f.reprompt_count > 0
        }
        "WHACK_A_MOLE" => f.thrash_index >= 2.0 || f.file_churn >= 4.0,
        "CACHE_COLD_RESTARTS" => f.token_burn_total > 20_000 && f.cache_read_ratio < 0.08,
        // Profile-level / unknown patterns have no per-session condition.
        _ => false,
    }
}

pub fn finding_from_hit(
    store: &Store,
    sessions_by_id: &HashMap<String, Session>,
    hit: &DetectorHit,
    affected: &[FeatureVector],
    total: usize,
) -> Finding {
    let evidence = affected
        .iter()
        .take(12)
        .map(|f| evidence_for(store, sessions_by_id.get(&f.session_id), hit.pattern_id, f))
        .collect::<Vec<_>>();
    let token_cost = affected
        .iter()
        .map(|f| estimate_tokens(hit.pattern_id, f))
        .sum();
    let min_cost = affected
        .iter()
        .map(|f| estimate_minutes(hit.pattern_id, f))
        .sum();
    Finding {
        id: stable_id(&[
            hit.pattern_id,
            &affected
                .iter()
                .map(|f| f.session_id.as_str())
                .collect::<Vec<_>>()
                .join(""),
        ]),
        pattern_id: hit.pattern_id.into(),
        title: hit.title.into(),
        severity: hit.severity,
        frequency: affected.len() as f64 / total.max(1) as f64,
        est_cost_tokens: token_cost,
        est_cost_minutes: min_cost,
        confidence: detector_confidence(hit.severity, affected.len(), total.max(1)),
        rationale: hit.rationale.clone(),
        evidence,
        status: "candidate".into(),
        verifier_verdict: None,
    }
}

pub fn nominate(store: &Store, profile: &CompetenceProfile) -> Result<Vec<Finding>> {
    nominate_windowed(store, profile, None)
}

/// Time-windowed `nominate`: nominates findings considering ONLY the
/// FeatureVectors whose session started at-or-after `since` (`None` = all-time,
/// which `nominate` delegates to — no behavior change for existing callers).
/// Identical ranking/cap to the all-time path; the single difference is the
/// feature set comes from `features_since(since)` instead of `all_features()`,
/// so a tighter window genuinely changes which candidates fire.
pub fn nominate_windowed(
    store: &Store,
    profile: &CompetenceProfile,
    since: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<Finding>> {
    let features = store.features_since(since)?;
    let sessions_by_id = store
        .sessions()?
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect::<HashMap<_, _>>();
    let total = features.len().max(1);
    let mut findings = detect(profile, &features)
        .into_iter()
        .map(|hit| finding_from_hit(store, &sessions_by_id, &hit, &hit.affected, total))
        .collect::<Vec<_>>();
    findings.sort_by(|a, b| {
        b.severity.cmp(&a.severity).then_with(|| {
            b.frequency
                .partial_cmp(&a.frequency)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    findings.truncate(8);
    Ok(findings)
}
fn estimate_tokens(pattern: &str, f: &FeatureVector) -> u64 {
    match pattern {
        "CONTEXT_BLOAT" => f.search_in_main_context as u64 * 3500,
        "NO_DELEGATION" => f.tool_call_count as u64 * 1800,
        "CACHE_COLD_RESTARTS" => f.token_burn_total / 3,
        "UNVERIFIED_COMPLETION" => 5000,
        "IGNORED_TOOL_ERROR" => f.token_burn_total / 10,
        _ => f.token_burn_total / 20,
    }
}
fn estimate_minutes(pattern: &str, f: &FeatureVector) -> u64 {
    match pattern {
        "UNVERIFIED_COMPLETION" => 20,
        "WHACK_A_MOLE" => (f.thrash_index as u64 + 1) * 12,
        "NO_DELEGATION" => f.tool_call_count as u64 / 3,
        "CONTEXT_BLOAT" => f.search_in_main_context as u64 / 2,
        _ => 5,
    }
}
fn detector_confidence(sev: u8, count: usize, total: usize) -> f64 {
    ((sev as f64 / 5.0) * 0.55 + ((count as f64) / (total.max(1) as f64)).min(1.0) * 0.30 + 0.15)
        .min(0.92)
}
fn evidence_for(
    store: &Store,
    session: Option<&Session>,
    pattern: &str,
    f: &FeatureVector,
) -> EvidenceRef {
    if let Ok(rows) = store.session_events(&f.session_id) {
        if pattern == "UNVERIFIED_COMPLETION" {
            return metric_evidence(session, rows.first().map(|(_, e)| e), pattern, f);
        }
        if let Some(e) = rows.iter().find_map(|(_t, e)| match &e.event {
            Event::ToolResult {
                status: ToolStatus::Error,
                summary,
                ..
            } if pattern == "IGNORED_TOOL_ERROR" => Some(EvidenceRef {
                session_id: f.session_id.clone(),
                turn_id: Some(e.turn_id.clone()),
                event_id: Some(e.id.clone()),
                quote: Some(truncate_chars(
                    &format!(
                        "Tool error rate {:.0}% with {} ignored error(s). Latest error: {}",
                        f.tool_error_rate * 100.0,
                        f.ignored_error_count,
                        summary.clone().unwrap_or_else(|| "no summary".into())
                    ),
                    220,
                )),
                source_path: Some(e.raw_ref.source_path.clone()),
            }),
            Event::ToolCall { tool, input, .. }
                if pattern == "CONTEXT_BLOAT"
                    && ["Read", "Grep", "Glob", "LS", "Bash"].contains(&tool.as_str()) =>
            {
                Some(EvidenceRef {
                    session_id: f.session_id.clone(),
                    turn_id: Some(e.turn_id.clone()),
                    event_id: Some(e.id.clone()),
                    quote: Some(truncate_chars(
                        &format!(
                            "Main-context search/read count {}; example: {tool} {input}",
                            f.search_in_main_context
                        ),
                        220,
                    )),
                    source_path: Some(e.raw_ref.source_path.clone()),
                })
            }
            Event::ToolCall { tool, input, .. }
                if pattern == "NO_DELEGATION"
                    && ["Read", "Grep", "Glob", "LS", "Bash"].contains(&tool.as_str()) =>
            {
                Some(EvidenceRef {
                    session_id: f.session_id.clone(),
                    turn_id: Some(e.turn_id.clone()),
                    event_id: Some(e.id.clone()),
                    quote: Some(truncate_chars(
                        &format!(
                            "{} tool calls, {} subagent spawns; example main-context discovery call: {tool} {input}",
                            f.tool_call_count, f.subagent_spawn_count
                        ),
                        220,
                    )),
                    source_path: Some(e.raw_ref.source_path.clone()),
                })
            }
            Event::FileSnapshot { .. } if pattern == "WHACK_A_MOLE" => Some(EvidenceRef {
                session_id: f.session_id.clone(),
                turn_id: Some(e.turn_id.clone()),
                event_id: Some(e.id.clone()),
                quote: Some(format!(
                    "Thrash index {:.1}; average file churn {:.1} in this session.",
                    f.thrash_index, f.file_churn
                )),
                source_path: Some(e.raw_ref.source_path.clone()),
            }),
            Event::TokenUsage { .. } if pattern == "CACHE_COLD_RESTARTS" => Some(EvidenceRef {
                session_id: f.session_id.clone(),
                turn_id: Some(e.turn_id.clone()),
                event_id: Some(e.id.clone()),
                quote: Some(format!(
                    "{} total tokens with {:.1}% cache-read ratio.",
                    f.token_burn_total,
                    f.cache_read_ratio * 100.0
                )),
                source_path: Some(e.raw_ref.source_path.clone()),
            }),
            Event::UserPrompt { text, .. } if pattern == "VAGUE_PROMPT" => Some(EvidenceRef {
                session_id: f.session_id.clone(),
                turn_id: Some(e.turn_id.clone()),
                event_id: Some(e.id.clone()),
                quote: Some(truncate_chars(
                    &format!(
                        "Prompt specificity {:.2}, reprompts {}; prompt: {}",
                        f.prompt_specificity, f.reprompt_count, text
                    ),
                    220,
                )),
                source_path: Some(e.raw_ref.source_path.clone()),
            }),
            Event::UserPrompt { text, .. } if text.len() > 20 => Some(EvidenceRef {
                session_id: f.session_id.clone(),
                turn_id: Some(e.turn_id.clone()),
                event_id: Some(e.id.clone()),
                quote: Some(truncate_chars(&fallback_quote(pattern, f, Some(text)), 220)),
                source_path: Some(e.raw_ref.source_path.clone()),
            }),
            _ => None,
        }) {
            return e;
        }
    }

    metric_evidence(session, None, pattern, f)
}

fn metric_evidence(
    session: Option<&Session>,
    event: Option<&EventRecord>,
    pattern: &str,
    f: &FeatureVector,
) -> EvidenceRef {
    EvidenceRef {
        session_id: f.session_id.clone(),
        turn_id: event.map(|e| e.turn_id.clone()),
        event_id: event.map(|e| e.id.clone()),
        quote: Some(truncate_chars(&fallback_quote(pattern, f, None), 220)),
        source_path: event
            .map(|e| e.raw_ref.source_path.clone())
            .or_else(|| session.map(|s| s.source_path.clone()))
            .or_else(|| Some(PathBuf::from("unknown"))),
    }
}

fn fallback_quote(pattern: &str, f: &FeatureVector, prompt: Option<&str>) -> String {
    let base = match pattern {
        "UNVERIFIED_COMPLETION" => format!(
            "{} tool calls but no observed verification command; project={}",
            f.tool_call_count,
            f.project.as_deref().unwrap_or("unknown")
        ),
        "IGNORED_TOOL_ERROR" => format!(
            "Tool error rate {:.0}% with {} ignored error(s).",
            f.tool_error_rate * 100.0,
            f.ignored_error_count
        ),
        "WHACK_A_MOLE" => format!(
            "Thrash index {:.1}; average file churn {:.1}.",
            f.thrash_index, f.file_churn
        ),
        "NO_DELEGATION" => format!(
            "{} tool calls and {} main-context discovery calls with no delegation.",
            f.tool_call_count, f.search_in_main_context
        ),
        "CONTEXT_BLOAT" => format!(
            "{} main-context discovery calls; context saturation peak {:.0}%.",
            f.search_in_main_context,
            f.context_saturation_peak * 100.0
        ),
        "CACHE_COLD_RESTARTS" => format!(
            "{} total tokens with {:.1}% cache-read ratio.",
            f.token_burn_total,
            f.cache_read_ratio * 100.0
        ),
        "VAGUE_PROMPT" => format!(
            "Prompt specificity {:.2} with {} reprompt(s).",
            f.prompt_specificity, f.reprompt_count
        ),
        _ => format!("Feature evidence for {pattern}."),
    };
    match prompt {
        Some(p) => format!("{base} Prompt excerpt: {p}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::Window;
    #[test]
    fn confidence_bounded() {
        assert!(detector_confidence(5, 10, 10) <= 0.92);
    }

    #[test]
    fn synthetic_evidence_names_unverified_metric() {
        let store = Store::memory().unwrap();
        let f = FeatureVector {
            session_id: "s".into(),
            tool_call_count: 7,
            verification_present: false,
            project: Some("/tmp/project".into()),
            ..Default::default()
        };
        let e = evidence_for(&store, None, "UNVERIFIED_COMPLETION", &f);
        assert!(e
            .quote
            .unwrap()
            .contains("no observed verification command"));
    }

    /// Seed a minimal session row (no events) so a saved FeatureVector satisfies
    /// the features→sessions foreign key. Detectors read only the feature fields,
    /// so the empty session is sufficient.
    fn seed_session(store: &Store, id: &str, started_at: chrono::DateTime<chrono::Utc>) {
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

    #[test]
    fn nominate_windowed_excludes_out_of_window_candidates() {
        use chrono::Duration;
        let store = Store::memory().unwrap();
        let now = chrono::Utc::now();

        // RECENT (1h ago): trips UNVERIFIED_COMPLETION (>=4 tool calls, no
        // verification) and nothing else.
        seed_session(&store, "recent", now - Duration::hours(1));
        store
            .save_feature(
                &FeatureVector {
                    session_id: "recent".into(),
                    started_at: Some(now - Duration::hours(1)),
                    tool_call_count: 5,
                    verification_present: false,
                    ..Default::default()
                },
                "test",
            )
            .unwrap();

        // OLD (100d ago): trips CACHE_COLD_RESTARTS (high burn, cold cache) and
        // deliberately does NOT trip UNVERIFIED_COMPLETION (verification_present).
        seed_session(&store, "old", now - Duration::days(100));
        store
            .save_feature(
                &FeatureVector {
                    session_id: "old".into(),
                    started_at: Some(now - Duration::days(100)),
                    token_burn_total: 30_000,
                    cache_read_ratio: 0.0,
                    verification_present: true,
                    ..Default::default()
                },
                "test",
            )
            .unwrap();

        let profile = CompetenceProfile::default();

        // 7-day window: only the recent feature is in scope → only
        // UNVERIFIED_COMPLETION nominates; the old session's CACHE_COLD_RESTARTS
        // is invisible.
        let windowed =
            nominate_windowed(&store, &profile, Window::D7.since(now)).unwrap();
        let win_patterns: Vec<&str> = windowed.iter().map(|f| f.pattern_id.as_str()).collect();
        assert!(
            win_patterns.contains(&"UNVERIFIED_COMPLETION"),
            "recent in-window session must still nominate, got {win_patterns:?}"
        );
        assert!(
            !win_patterns.contains(&"CACHE_COLD_RESTARTS"),
            "out-of-window (100d-old) feature must NOT contribute candidates, got {win_patterns:?}"
        );

        // All-time: both features in scope → both patterns nominate, proving the
        // candidate set genuinely differs from the 7-day window.
        let all = nominate_windowed(&store, &profile, Window::AllTime.since(now)).unwrap();
        let all_patterns: Vec<&str> = all.iter().map(|f| f.pattern_id.as_str()).collect();
        assert!(all_patterns.contains(&"UNVERIFIED_COMPLETION"));
        assert!(
            all_patterns.contains(&"CACHE_COLD_RESTARTS"),
            "all-time must surface the old session's candidate, got {all_patterns:?}"
        );

        // nominate() == nominate_windowed(None) == all-time set (no behavior drift).
        let legacy = nominate(&store, &profile).unwrap();
        let mut legacy_patterns: Vec<String> =
            legacy.iter().map(|f| f.pattern_id.clone()).collect();
        let mut all_patterns_owned: Vec<String> =
            all.iter().map(|f| f.pattern_id.clone()).collect();
        legacy_patterns.sort();
        all_patterns_owned.sort();
        assert_eq!(legacy_patterns, all_patterns_owned);
    }

    /// `session_trips_pattern` MUST agree with `detect()`: for every per-session
    /// pattern, a session appears in that pattern's `affected` set iff the helper
    /// says it trips. This is the anti-drift guard — `detect()` filters through the
    /// helper, so if anyone forks a threshold this fails. We cross-check a battery
    /// of feature vectors that sit on both sides of several thresholds.
    #[test]
    fn session_trips_pattern_agrees_with_detect_no_drift() {
        let per_session = [
            "CONTEXT_BLOAT",
            "NO_DELEGATION",
            "UNVERIFIED_COMPLETION",
            "IGNORED_TOOL_ERROR",
            "VAGUE_PROMPT",
            "WHACK_A_MOLE",
            "CACHE_COLD_RESTARTS",
        ];

        // A spread of sessions straddling the thresholds of every pattern.
        let cases = vec![
            // trips UNVERIFIED_COMPLETION only
            FeatureVector {
                session_id: "a".into(),
                tool_call_count: 5,
                verification_present: false,
                ..Default::default()
            },
            // clean for UNVERIFIED (verified) but trips CONTEXT_BLOAT (>=8 searches)
            FeatureVector {
                session_id: "b".into(),
                tool_call_count: 5,
                verification_present: true,
                search_in_main_context: 9,
                ..Default::default()
            },
            // trips CONTEXT_BLOAT via the (>=4 && saturation>0.35) branch + NO_DELEGATION
            FeatureVector {
                session_id: "c".into(),
                tool_call_count: 14,
                verification_present: true,
                search_in_main_context: 4,
                context_saturation_peak: 0.4,
                subagent_spawn_count: 0,
                ..Default::default()
            },
            // trips IGNORED_TOOL_ERROR (rate branch) + WHACK_A_MOLE (churn branch)
            FeatureVector {
                session_id: "d".into(),
                tool_call_count: 3,
                verification_present: true,
                tool_error_rate: 0.5,
                file_churn: 5.0,
                ..Default::default()
            },
            // trips VAGUE_PROMPT + CACHE_COLD_RESTARTS
            FeatureVector {
                session_id: "e".into(),
                tool_call_count: 3,
                verification_present: true,
                prompt_specificity: 0.2,
                reprompt_count: 1,
                token_burn_total: 30_000,
                cache_read_ratio: 0.0,
                ..Default::default()
            },
            // an all-clean session (trips nothing)
            FeatureVector {
                session_id: "f".into(),
                tool_call_count: 2,
                verification_present: true,
                prompt_specificity: 0.9,
                cache_read_ratio: 0.9,
                ..Default::default()
            },
            // boundary: WHACK_A_MOLE thrash exactly at 2.0 (>=) trips; VAGUE at 0.28 (<) clean
            FeatureVector {
                session_id: "g".into(),
                tool_call_count: 2,
                verification_present: true,
                thrash_index: 2.0,
                prompt_specificity: 0.28,
                reprompt_count: 1,
                ..Default::default()
            },
        ];

        let profile = CompetenceProfile::default();
        let hits = detect(&profile, &cases);

        for pat in per_session {
            // The session ids detect() flagged for this pattern.
            let flagged: std::collections::HashSet<String> = hits
                .iter()
                .filter(|h| h.pattern_id == pat)
                .flat_map(|h| h.affected.iter().map(|f| f.session_id.clone()))
                .collect();
            for fv in &cases {
                let helper = session_trips_pattern(pat, fv);
                let in_detect = flagged.contains(&fv.session_id);
                assert_eq!(
                    helper, in_detect,
                    "drift on pattern {pat} session {}: helper={helper} detect={in_detect}",
                    fv.session_id
                );
            }
        }

        // REPEATED_EXPLANATION is profile-level → never a per-session trip.
        for fv in &cases {
            assert!(
                !session_trips_pattern("REPEATED_EXPLANATION", fv),
                "REPEATED_EXPLANATION must never trip per-session"
            );
        }
        // Unknown ids never trip.
        assert!(!session_trips_pattern("NOPE_UNKNOWN", &cases[0]));
    }
}
