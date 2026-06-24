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

    add("CONTEXT_BLOAT","Context bloat",4,features.iter().filter(|f|f.search_in_main_context>=8 || (f.search_in_main_context>=4 && f.context_saturation_peak>0.35)).cloned().collect(),"Search and file-reading tools are repeatedly used in the main Claude context, increasing token burn before useful edits.".into());
    add("NO_DELEGATION","No delegation",4,features.iter().filter(|f|f.tool_call_count>=12 && f.subagent_spawn_count==0 && f.search_in_main_context>=3).cloned().collect(),"Search-heavy sessions rarely use Claude Code Task subagents, so discovery work competes with implementation context.".into());
    add("UNVERIFIED_COMPLETION","Unverified completion",5,features.iter().filter(|f|f.tool_call_count>=4 && !f.verification_present).cloned().collect(),"Sessions reach substantial tool use without an observed build/test/verification command before completion.".into());
    add("IGNORED_TOOL_ERROR","Ignored tool errors",4,features.iter().filter(|f|f.ignored_error_count>0 || f.tool_error_rate>0.25).cloned().collect(),"Tool errors appear at a high rate and are not consistently followed by clear verification/correction signals.".into());
    add(
        "VAGUE_PROMPT",
        "Vague prompts",
        3,
        features
            .iter()
            .filter(|f| {
                f.prompt_specificity > 0.0 && f.prompt_specificity < 0.28 && f.reprompt_count > 0
            })
            .cloned()
            .collect(),
        "Some sessions start from underspecified prompts and need corrective follow-up turns."
            .into(),
    );
    add("WHACK_A_MOLE","Whack-a-mole loops",4,features.iter().filter(|f|f.thrash_index>=2.0 || f.file_churn>=4.0).cloned().collect(),"Repeated edits or repeated failing commands suggest symptom-patching loops instead of a reset to root cause.".into());
    add("CACHE_COLD_RESTARTS","Cache-cold restarts",3,features.iter().filter(|f|f.token_burn_total>20_000 && f.cache_read_ratio<0.08).cloned().collect(),"High-token sessions show low cache-read ratios, suggesting expensive cold context restarts.".into());
    if profile
        .repeated_explanation_clusters
        .iter()
        .any(|c| c.count >= 3)
    {
        add("REPEATED_EXPLANATION","Repeated explanation",3,features.iter().take(20).cloned().collect(),"Multiple sessions cluster around the same project context; durable project memory may reduce re-explanation.".into());
    }

    hits
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
    let features = store.all_features()?;
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
}
