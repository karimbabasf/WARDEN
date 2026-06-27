//! Per-agent construction: join a stored session's size/composition/labels/activity
//! into a [`RadarAgent`], and tail its events into a recent-activity feed.

use super::composition::{
    self, claude_context_size, codex_context_size, exact_composition, tokenize_len, ContextSize,
};
use super::context::{context_breakdown, est_cost_usd, estimate_for_session};
use super::identity::{first_task, identity};
use super::liveness::AgentStatus;
use super::model::{RadarActivity, RadarAgent, RadarComposition, RadarExact};
use crate::ir::{Event, Harness, Session};
use crate::store::Store;

/// Build one [`RadarAgent`] from a stored session, joining size/composition
/// (Tasks 7/8), labels/identity (per harness), recent activity, and est cost.
pub(crate) fn build_agent(
    store: &Store,
    s: &Session,
    parent_id: Option<String>,
    depth: u32,
    child_count: u32,
    status: AgentStatus,
) -> RadarAgent {
    let events = store.session_events(&s.id).unwrap_or_default();

    // Last TokenUsage drives live occupancy + exact composition.
    let last_usage = events
        .iter()
        .rev()
        .find(|(_, e)| matches!(e.event, Event::TokenUsage { .. }))
        .map(|(_, e)| e.event.clone());
    let model = last_usage
        .as_ref()
        .and_then(|e| match e {
            Event::TokenUsage { model, .. } if !model.trim().is_empty() => Some(model.clone()),
            _ => None,
        })
        .or_else(|| first_non_empty_model_id(s));

    let (base_size, exact) = match &last_usage {
        Some(u) => {
            let m = model.clone().unwrap_or_default();
            let size = match s.harness {
                Harness::Codex => {
                    // Codex resident size = input_tokens; window from the transcript
                    // metadata when present, falling back to the provider/model table.
                    let input = match u {
                        Event::TokenUsage { input, .. } => *input as u64,
                        _ => 0,
                    };
                    codex_context_size(input, codex_context_window(s, &m))
                }
                _ => claude_context_size(u, &m),
            };
            (size, exact_composition(u))
        }
        None => (
            ContextSize {
                context_tokens: 0,
                max_tokens: composition::max_window_for_model(&model.clone().unwrap_or_default()),
                fill_pct: 0.0,
            },
            composition::ExactComposition {
                cache_read: 0,
                fresh: 0,
                output: 0,
            },
        ),
    };
    let pending_tail_tokens = if last_usage.is_some() {
        pending_context_after_latest_usage(&events)
    } else {
        0
    };
    let size = with_pending_context(base_size, pending_tail_tokens);

    let estimated = estimate_for_session(store, s, &events, base_size.context_tokens);
    let context_breakdown = context_breakdown(
        s.harness.clone(),
        size,
        estimated.clone(),
        &events,
        pending_tail_tokens,
    );

    let recent_activity = recent_activity(&events);
    let est_cost_usd = est_cost_usd(&model, &exact);
    let task = first_task(&events);
    let (label, nickname, role, origin) = identity(s, task);
    let cwd = s
        .project
        .as_ref()
        .and_then(|p| p.cwd.file_name())
        .map(|n| n.to_string_lossy().to_string());

    RadarAgent {
        id: s.id.clone(),
        harness: s.harness.as_str().to_string(),
        origin,
        parent_id,
        depth,
        label,
        nickname,
        cwd,
        role,
        model,
        status: status.as_str().to_string(),
        context_tokens: size.context_tokens,
        max_tokens: size.max_tokens,
        fill_pct: size.fill_pct,
        context_breakdown,
        composition: RadarComposition {
            exact: RadarExact {
                cache_read: exact.cache_read,
                fresh: exact.fresh,
                output: exact.output,
            },
            estimated,
        },
        recent_activity,
        child_count,
        started_at: s.started_at.to_rfc3339(),
        est_cost_usd,
    }
}

fn first_non_empty_model_id(s: &Session) -> Option<String> {
    s.model_ids.iter().find(|m| !m.trim().is_empty()).cloned()
}

fn codex_context_window(s: &Session, model: &str) -> u64 {
    s.meta
        .get("model_context_window")
        .and_then(serde_json::Value::as_u64)
        .filter(|n| *n > 0)
        .unwrap_or_else(|| composition::max_window_for_model(model))
}

fn with_pending_context(mut size: ContextSize, pending_tail_tokens: u64) -> ContextSize {
    if pending_tail_tokens == 0 {
        return size;
    }
    size.context_tokens = size.context_tokens.saturating_add(pending_tail_tokens);
    size.fill_pct = if size.max_tokens == 0 {
        0.0
    } else {
        (size.context_tokens as f64 / size.max_tokens as f64).clamp(0.0, 1.0)
    };
    size
}

fn pending_context_after_latest_usage(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> u64 {
    let Some(last_usage_idx) = events
        .iter()
        .rposition(|(_, e)| matches!(e.event, Event::TokenUsage { .. }))
    else {
        return 0;
    };
    events
        .iter()
        .skip(last_usage_idx + 1)
        .map(|(_, e)| match &e.event {
            Event::UserPrompt { text, .. } | Event::AssistantText { text } => tokenize_len(text),
            Event::Thinking { tokens } => *tokens as u64,
            Event::ToolCall { tool, input, .. } => tokenize_len(&format!("{tool} {input}")),
            Event::ToolResult { bytes, .. } => bytes / 4,
            _ => 0,
        })
        .sum()
}

/// Every action event as a recent-activity row (newest first): a kind glyph-friendly
/// `kind` plus a short label. No cap — the detail panel shows ~10 rows in a
/// scrollable feed and lets you scroll back to the very first action.
pub(crate) fn recent_activity(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Vec<RadarActivity> {
    let mut out: Vec<RadarActivity> = Vec::new();
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by(|(_, a), (_, b)| {
        b.ts.cmp(&a.ts)
            .then(b.raw_ref.offset.cmp(&a.raw_ref.offset))
            .then(b.id.cmp(&a.id))
    });
    for (_, e) in ordered {
        let (kind, label) = match &e.event {
            // The "what is it doing" signal: name the file touched / command run, not
            // just the bare tool name.
            Event::ToolCall { tool, input, .. } => ("tool", tool_activity_label(tool, input)),
            Event::AssistantText { text } => ("message", crate::util::truncate_chars(text, 80)),
            Event::UserPrompt { text, .. } => ("message", crate::util::truncate_chars(text, 80)),
            Event::Thinking { .. } => ("thinking", "thinking".to_string()),
            // ToolResult (and the rest) is not a distinct action — its bare
            // `result <call_id>` row was pure noise, so it is dropped here.
            _ => continue,
        };
        out.push(RadarActivity {
            ts: e.ts.to_rfc3339(),
            kind: kind.to_string(),
            label,
        });
    }
    out
}

/// A target-rich activity label for a tool call: the file it touches or the command
/// it runs, prefixed by a compact tool name — e.g. `Read orbLayout.ts`,
/// `Bash cargo test`, `exec_command cargo build`. Falls back to the tool name alone
/// when the input carries no obvious target. Mirrors the real `Event::ToolCall.input`
/// shapes for Claude (`file_path`/`command`/`pattern`) and Codex (`cmd`).
fn tool_activity_label(tool: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    let short = short_tool_name(tool);
    let target = if let Some(f) = s("file_path")
        .or_else(|| s("path"))
        .or_else(|| s("notebook_path"))
    {
        Some(path_basename(f))
    } else if let Some(c) = s("command").or_else(|| s("cmd")) {
        Some(crate::util::truncate_chars(c.trim(), 64))
    } else if let Some(p) = s("pattern") {
        Some(crate::util::truncate_chars(p, 48))
    } else {
        None
    };
    match target {
        Some(t) if !t.is_empty() => format!("{short} {t}"),
        _ => short,
    }
}

/// A compact tool name: an MCP tool (`mcp__server__tool`) collapses to its final
/// segment (`tool`); everything else passes through unchanged.
fn short_tool_name(tool: &str) -> String {
    tool.rsplit("__").next().unwrap_or(tool).to_string()
}

/// The last component of a slash/backslash path (keeps the filename, drops the long
/// directory prefix). Returns the input unchanged when it has no separators.
fn path_basename(p: &str) -> String {
    p.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(p)
        .to_string()
}
