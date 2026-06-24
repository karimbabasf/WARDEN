//! RADAR: the live agent-forest collector.
//!
//! M3 assembles an ephemeral constellation of currently-open Claude/Codex agents
//! and their subagents from local files, computes per-agent context size + honest
//! composition, and emits a `radar_state` event. The forest is recomputed from
//! files on each FS event — no heavy persistence (see the M3 design spec).
//!
//! Submodules:
//! * [`hierarchy`] — pure resolvers that link subagents to their parents
//!   (Claude `subagents/` + `toolUseId`; Codex `parent_thread_id`).
//! * [`liveness`] — open/working/idle/closed partition (pure core + thin syscall).
//! * [`composition`] — exact + estimated context composition (pure).

pub mod composition;
pub mod hierarchy;
pub mod liveness;

pub use liveness::{AgentStatus, LiveSession};

use crate::ir::{Event, Harness, Session};
use crate::store::Store;
use chrono::{DateTime, Utc};
use composition::{
    claude_context_size, codex_context_size, estimate_composition, exact_composition, tokenize_len,
    ContextSize,
};
use liveness::{partition_claude, read_claude_registry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One agent (root or subagent) in the live forest — the frozen `radar_state`
/// contract, serialized camelCase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarAgent {
    pub id: String,
    pub harness: String,
    pub origin: Option<String>,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub label: String,
    pub nickname: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub context_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
    pub composition: RadarComposition,
    pub recent_activity: Vec<RadarActivity>,
    pub child_count: u32,
    pub started_at: String,
    pub est_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarExact {
    pub cache_read: u64,
    pub fresh: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarEstimated {
    pub preamble: u64,
    pub conversation: u64,
    pub tool_output: u64,
    pub thinking: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarComposition {
    pub exact: RadarExact,
    /// `None` (serialized `null`) when there is no turn-1 baseline to estimate from.
    pub estimated: Option<RadarEstimated>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarActivity {
    pub ts: String,
    pub kind: String,
    pub label: String,
}

/// The full live forest, emitted as event `radar_state` and returned by the
/// `get_radar_state` command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarState {
    pub generated_at: String,
    pub agents: Vec<RadarAgent>,
}

/// How many recent events to surface per agent in `recentActivity`.
const RECENT_ACTIVITY_N: usize = 8;

/// Assemble the live agent forest from the store + the Claude liveness registry.
///
/// `is_alive`/`now` are injected so the join is deterministic and unit-testable
/// without real PIDs or a real clock. The forest is the set of store sessions;
/// each becomes a [`RadarAgent`] with:
/// * `parentId`/`depth`/`childCount` from `Store::parent_of`;
/// * size + exact composition from the session's last `TokenUsage` event
///   (Tasks 7), and an estimated composition from its turn-1 baseline (Task 8);
/// * `status` from the liveness partition (Claude registry match by external id,
///   else idle) — Task 6.
///
/// Honest viz: labels/origin/nickname come straight from the session metadata; no
/// children are fabricated (a child only exists when linkage was persisted).
pub fn assemble(
    store: &Store,
    sessions_dir: &Path,
    is_alive: &dyn Fn(u32) -> bool,
    now: DateTime<Utc>,
) -> RadarState {
    let sessions = store.sessions().unwrap_or_default();

    // Liveness: map a Claude external session id → status from the registry.
    let registry = read_claude_registry(sessions_dir);
    let working_secs = crate::util::radar_working_ms() / 1000;
    let mtime_secs_ago = |sid: &str| transcript_mtime_secs_ago(&sessions, sid, now);
    let live = partition_claude(&registry, is_alive, &mtime_secs_ago, working_secs);
    let claude_status: HashMap<String, AgentStatus> = live
        .into_iter()
        .map(|(s, st)| (s.session_id, st))
        .collect();

    // childCount per session id (how many sessions name it as parent).
    let mut child_count: HashMap<String, u32> = HashMap::new();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for s in &sessions {
        let parent = store.parent_of(&s.id).ok().flatten();
        if let Some(p) = &parent {
            *child_count.entry(p.clone()).or_insert(0) += 1;
        }
        parent_of.insert(s.id.clone(), parent);
    }

    // Depth is the parent-chain length (root = 0). Memoized over the parent map.
    let mut agents = Vec::with_capacity(sessions.len());
    for s in &sessions {
        let parent_id = parent_of.get(&s.id).cloned().flatten();
        let depth = depth_of(&s.id, &parent_of);
        let status = agent_status(s, &claude_status, &mtime_secs_ago);
        agents.push(build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            status,
        ));
    }

    RadarState {
        generated_at: now.to_rfc3339(),
        agents,
    }
}

/// Status for one session: a Claude session uses the registry partition (by
/// external id); anything else (Codex, etc.) is Idle when recently written, else
/// Idle — the live collector treats store-resident sessions as open/idle and lets
/// the watcher's recompute drop a session that has left the live set.
fn agent_status(
    s: &Session,
    claude_status: &HashMap<String, AgentStatus>,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
) -> AgentStatus {
    if let Some(st) = claude_status.get(&s.external_id) {
        return *st;
    }
    // Non-registry sessions: working when their transcript was just written.
    let working_secs = crate::util::radar_working_ms() / 1000;
    match mtime_secs_ago(&s.external_id) {
        Some(secs) if secs < working_secs => AgentStatus::Working,
        _ => AgentStatus::Idle,
    }
}

/// Seconds since the session's transcript was last modified, by `external_id`.
/// Returns `None` when the session/file is unknown or its mtime is unreadable.
fn transcript_mtime_secs_ago(
    sessions: &[Session],
    external_id: &str,
    now: DateTime<Utc>,
) -> Option<u64> {
    let session = sessions.iter().find(|s| s.external_id == external_id)?;
    let modified = std::fs::metadata(&session.source_path)
        .and_then(|m| m.modified())
        .ok()?;
    let modified: DateTime<Utc> = modified.into();
    let secs = now.signed_duration_since(modified).num_seconds();
    Some(secs.max(0) as u64)
}

/// Depth = number of ancestors via the persisted parent links (root = 0). Bounded
/// to avoid looping on a malformed cycle.
fn depth_of(id: &str, parent_of: &HashMap<String, Option<String>>) -> u32 {
    let mut depth = 0;
    let mut cur = id.to_string();
    for _ in 0..64 {
        match parent_of.get(&cur).and_then(|p| p.clone()) {
            Some(p) => {
                depth += 1;
                cur = p;
            }
            None => break,
        }
    }
    depth
}

/// Build one [`RadarAgent`] from a stored session, joining size/composition
/// (Tasks 7/8), labels/identity (per harness), recent activity, and est cost.
fn build_agent(
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
            Event::TokenUsage { model, .. } => Some(model.clone()),
            _ => None,
        })
        .or_else(|| s.model_ids.first().cloned());

    let (size, exact) = match &last_usage {
        Some(u) => {
            let m = model.clone().unwrap_or_default();
            let size = match s.harness {
                Harness::Codex => {
                    // Codex resident size = input_tokens; window from model lookup.
                    let input = match u {
                        Event::TokenUsage { input, .. } => *input as u64,
                        _ => 0,
                    };
                    codex_context_size(input, composition::max_window_for_model(&m))
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

    let estimated = estimate_for_session(&events, size.context_tokens);

    let recent_activity = recent_activity(&events);
    let est_cost_usd = est_cost_usd(&model, &exact);
    let (label, nickname, role, origin) = identity(s);

    RadarAgent {
        id: s.id.clone(),
        harness: s.harness.as_str().to_string(),
        origin,
        parent_id,
        depth,
        label,
        nickname,
        role,
        model,
        status: status.as_str().to_string(),
        context_tokens: size.context_tokens,
        max_tokens: size.max_tokens,
        fill_pct: size.fill_pct,
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

/// Derive the estimated (semantic) composition for a session, calibrated to its
/// current exact total. `None` when there is no turn-1 `TokenUsage` baseline.
fn estimate_for_session(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    exact_total: u64,
) -> Option<RadarEstimated> {
    // turn-1 total = first TokenUsage's resident size (input+cache_read+cache_creation).
    let turn1_total = events.iter().find_map(|(_, e)| match &e.event {
        Event::TokenUsage {
            input,
            cache_creation,
            cache_read,
            ..
        } => Some(*input as u64 + *cache_creation as u64 + *cache_read as u64),
        _ => None,
    })?;

    let first_user_tokens = events
        .iter()
        .find_map(|(_, e)| match &e.event {
            Event::UserPrompt { text, .. } => Some(tokenize_len(text)),
            _ => None,
        })
        .unwrap_or(0);

    let mut conversation = 0u64;
    let mut tool_output = 0u64;
    let mut thinking = 0u64;
    for (_, e) in events {
        match &e.event {
            Event::AssistantText { text } => conversation += tokenize_len(text),
            Event::UserPrompt { text, .. } => conversation += tokenize_len(text),
            // ToolResult byte size as a coarse token proxy (≈ bytes/4); the
            // calibration step rescales it to the exact anchor anyway.
            Event::ToolResult { bytes, .. } => tool_output += bytes / 4,
            Event::Thinking { tokens } => thinking += *tokens as u64,
            _ => {}
        }
    }

    let est = estimate_composition(
        turn1_total,
        first_user_tokens,
        conversation,
        tool_output,
        thinking,
        exact_total,
    );
    Some(RadarEstimated {
        preamble: est.preamble,
        conversation: est.conversation,
        tool_output: est.tool_output,
        thinking: est.thinking,
    })
}

/// The last N events as recent-activity rows (newest first): a kind glyph-friendly
/// `kind` plus a short label.
fn recent_activity(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> Vec<RadarActivity> {
    let mut out: Vec<RadarActivity> = Vec::new();
    for (_, e) in events.iter().rev() {
        let (kind, label) = match &e.event {
            Event::ToolCall { tool, .. } => ("tool", tool.clone()),
            Event::ToolResult { call_id, .. } => ("tool", format!("result {call_id}")),
            Event::AssistantText { text } => ("message", crate::util::truncate_chars(text, 80)),
            Event::UserPrompt { text, .. } => ("message", crate::util::truncate_chars(text, 80)),
            Event::Thinking { .. } => ("thinking", "thinking".to_string()),
            _ => continue,
        };
        out.push(RadarActivity {
            ts: e.ts.to_rfc3339(),
            kind: kind.to_string(),
            label,
        });
        if out.len() >= RECENT_ACTIVITY_N {
            break;
        }
    }
    out
}

/// Identity quad: `(label, nickname, role, origin)`.
/// * Claude subagent → label = its sidecar `description`, role = its `agentType`
///   (persisted onto the child `meta` when the parent linkage is recorded);
/// * Codex → nickname/role/origin from `session_meta`; label = nickname when set;
/// * otherwise label = cwd basename (root), falling back to the external id.
fn identity(s: &Session) -> (String, Option<String>, Option<String>, Option<String>) {
    let nickname = s
        .meta
        .get("agent_nickname")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let origin = s
        .meta
        .get("originator")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // A Claude subagent carries its sidecar `description`/`agentType` on its meta
    // (written when the parent link is persisted). When present they win the label
    // and role — these keys never appear on a Codex session or a root.
    let claude_description = s
        .meta
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    let claude_agent_type = s
        .meta
        .get("agentType")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);

    // Role: Claude subagent `agentType`, else the Codex `agent_role`.
    let role = claude_agent_type.clone().or_else(|| {
        s.meta
            .get("agent_role")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });

    // Label precedence: Claude subagent description → cwd basename (root) → Codex
    // nickname → external id.
    let label = claude_description
        .or_else(|| {
            s.project
                .as_ref()
                .and_then(|p| p.cwd.file_name())
                .map(|n| n.to_string_lossy().to_string())
        })
        .or_else(|| nickname.clone())
        .unwrap_or_else(|| s.external_id.clone());

    (label, nickname, role, origin)
}

/// Rough USD cost for the turn's tokens from a small per-model price table
/// ($/1M tokens). `None` when the model is unknown — honest, never fabricated.
fn est_cost_usd(model: &Option<String>, exact: &composition::ExactComposition) -> Option<f64> {
    let m = model.as_deref()?.to_ascii_lowercase();
    // (input $/1M, output $/1M).
    let (in_rate, out_rate) = if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku") {
        (0.80, 4.0)
    } else if m.contains("gpt-5") || m.contains("codex") {
        (1.25, 10.0)
    } else {
        return None;
    };
    // Cache reads bill ~10× cheaper than fresh input across these providers, so
    // split the bill: cache_read at the cache-read rate, fresh at the input rate.
    const CACHE_READ_FACTOR: f64 = 0.1;
    let cost = (exact.cache_read as f64 / 1_000_000.0) * in_rate * CACHE_READ_FACTOR
        + (exact.fresh as f64 / 1_000_000.0) * in_rate
        + (exact.output as f64 / 1_000_000.0) * out_rate;
    Some(cost)
}

/// Re-derive subagent linkage over the current store sessions, persisting any
/// newly-resolvable `parent_session_id`. Runs both the Codex `parent_thread_id`
/// resolver and the Claude `SubagentSpawn` resolver idempotently (both keyed by ids
/// already in the store), so a subagent tree that forms AFTER startup is linked on
/// the next recompute instead of staying flat until a full backfill. Best-effort:
/// a linkage failure is swallowed so the live forest still renders.
fn relink_store_subagents(store: &Store) {
    let _ = crate::ingest::codex::link_codex_subagents_in_store(store);
    let _ = crate::ingest::claude_code::link_claude_subagents_in_store(store);
}

/// Recompute the forest and return it. The scheduler's watcher calls this on each
/// relevant FS event; `lib.rs` then emits it as `radar_state`. Uses the real
/// `pid_alive` syscall and the current clock.
///
/// Linkage is re-derived from the current store sessions first (cheap + idempotent)
/// so live trees — Codex Desktop subagents, Claude subagents — render nested even
/// when they form after the startup backfill.
pub fn recompute_radar_state(store: &Store, sessions_dir: &Path) -> RadarState {
    relink_store_subagents(store);
    assemble(store, sessions_dir, &liveness::pid_alive, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use chrono::Utc;
    use std::path::PathBuf;

    /// Seed a session with the given id/external/harness and an optional last
    /// `TokenUsage` event (so size/composition populate).
    fn seed(
        store: &Store,
        id: &str,
        external: &str,
        harness: Harness,
        cwd: Option<&str>,
        usage: Option<(u32, u32, u32, u32, &str)>,
    ) {
        let now = Utc::now();
        let session = Session {
            id: id.into(),
            harness,
            external_id: external.into(),
            project: cwd.map(|c| ProjectRef {
                cwd: PathBuf::from(c),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let tid = format!("{id}-t0");
        let mut events = vec![EventRecord {
            id: format!("{id}-p"),
            turn_id: tid.clone(),
            session_id: id.into(),
            ts: now,
            event: Event::UserPrompt {
                text: "do the thing".into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        }];
        if let Some((input, cc, cr, output, model)) = usage {
            events.push(EventRecord {
                id: format!("{id}-u"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now,
                event: Event::TokenUsage {
                    input,
                    output,
                    cache_creation: cc,
                    cache_read: cr,
                    model: model.into(),
                    orchestration: None,
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: 1,
                    line: 2,
                },
            });
        }
        let turn = Turn {
            id: tid,
            session_id: id.into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        store
            .upsert_session_batch(&session, &[turn], &events, 0)
            .unwrap();
    }

    /// `assemble` builds a forest: the root (depth 0, parentId null, childCount 1,
    /// populated occupancy + exact composition) and a linked child (depth 1,
    /// parentId == root). JSON serializes with camelCase keys.
    #[test]
    fn assemble_builds_root_and_child_with_size_and_links() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "root-sid",
            "root-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            Some((2, 13761, 331244, 2620, "claude-opus-4-8")),
        );
        seed(
            &store,
            "child-sid",
            "child-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("child-sid", "root-sid").unwrap();

        // No registry dir → liveness falls back to mtime/idle; inject is_alive=true.
        let now = Utc::now();
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, now);

        assert_eq!(state.agents.len(), 2);
        let root = state
            .agents
            .iter()
            .find(|a| a.id == "root-sid")
            .expect("root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(root.child_count, 1, "root has one linked child");
        assert_eq!(root.label, "MyRepo", "label is the cwd basename");
        assert_eq!(root.context_tokens, 345_007, "2+13761+331244");
        assert!((root.fill_pct - 1.0).abs() < 1e-9, "clamped to 1.0");
        assert_eq!(root.composition.exact.cache_read, 331_244);
        assert_eq!(root.composition.exact.fresh, 2 + 13_761);
        assert_eq!(root.composition.exact.output, 2_620);
        assert!(
            root.composition.estimated.is_some(),
            "a turn-1 baseline yields an estimated composition"
        );
        assert!(root.est_cost_usd.is_some(), "opus model → a cost estimate");

        let child = state
            .agents
            .iter()
            .find(|a| a.id == "child-sid")
            .expect("child present");
        assert_eq!(child.depth, 1);
        assert_eq!(child.parent_id.as_deref(), Some("root-sid"));
        assert_eq!(child.child_count, 0);

        // Contract: camelCase keys present in the serialized payload.
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"fillPct\""), "camelCase fillPct");
        assert!(json.contains("\"contextTokens\""), "camelCase contextTokens");
        assert!(json.contains("\"parentId\""), "camelCase parentId");
        assert!(json.contains("\"childCount\""), "camelCase childCount");
        assert!(json.contains("\"cacheRead\""), "camelCase nested cacheRead");
        assert!(json.contains("\"generatedAt\""), "camelCase generatedAt");
    }

    /// Finding 2: a Codex Desktop subagent inserted into the store WITHOUT any
    /// pre-run linkage pass is linked (non-NULL parent, appears as a child in the
    /// forest) after `recompute_radar_state` — recompute re-derives linkage over the
    /// current store sessions, so a tree that forms after startup is not flat.
    #[test]
    fn recompute_relinks_codex_subagent_without_pre_pass() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let mk = |id: &str, ext: &str, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::Codex,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        // Parent Codex Desktop session.
        store
            .upsert_session_batch(
                &mk("cx-parent", "thread-parent", serde_json::json!({ "originator": "Codex Desktop" })),
                &[],
                &[],
                0,
            )
            .unwrap();
        // Subagent: thread_source=subagent + parent_thread_id pointing at the parent.
        store
            .upsert_session_batch(
                &mk(
                    "cx-child",
                    "thread-child",
                    serde_json::json!({
                        "thread_source": "subagent",
                        "parent_thread_id": "thread-parent",
                        "agent_role": "explorer",
                        "agent_nickname": "Hilbert",
                        "originator": "Codex Desktop",
                    }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();

        // No pre-run linkage pass: parent is NULL right now.
        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            None,
            "precondition: child is unlinked before recompute"
        );

        let state = recompute_radar_state(&store, Path::new("/no/registry"));

        // Recompute re-derived the link and persisted it.
        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            Some("cx-parent".to_string()),
            "recompute must persist the newly-resolvable parent"
        );
        let child = state
            .agents
            .iter()
            .find(|a| a.id == "cx-child")
            .expect("child present");
        assert_eq!(child.parent_id.as_deref(), Some("cx-parent"));
        assert_eq!(child.depth, 1, "child renders nested, not flat");
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 1, "parent shows one child");
    }

    /// Finding 1: a linked Claude subagent surfaces its sidecar `description` as the
    /// `label` and its `agentType` as the `role` (the frozen `radar_state` contract),
    /// instead of falling back to the external id with a null role.
    #[test]
    fn claude_subagent_uses_description_and_agent_type() {
        let store = Store::memory().unwrap();
        // Parent root (Claude, has a cwd → label = basename).
        seed(
            &store,
            "p-sid",
            "p-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            None,
        );
        // Child subagent: meta carries the description + agentType the ingest path
        // persists from the sidecar `agent-<id>.meta.json`.
        let now = Utc::now();
        let child = Session {
            id: "c-sid".into(),
            harness: Harness::ClaudeCode,
            external_id: "c-ext".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/c.jsonl"),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({
                "description": "hunt for dead code in the radar module",
                "agentType": "Explore",
            }),
        };
        store
            .upsert_session_batch(&child, &[], &[], 0)
            .unwrap();
        store.link_child_session("c-sid", "p-sid").unwrap();

        let state = assemble(&store, Path::new("/no/registry"), &|_| true, Utc::now());
        let c = state
            .agents
            .iter()
            .find(|a| a.id == "c-sid")
            .expect("child present");
        assert_eq!(
            c.label, "hunt for dead code in the radar module",
            "Claude subagent label is its description"
        );
        assert_eq!(
            c.role.as_deref(),
            Some("Explore"),
            "Claude subagent role is its agentType"
        );

        // Root label is NOT regressed (still the cwd basename).
        let p = state.agents.iter().find(|a| a.id == "p-sid").unwrap();
        assert_eq!(p.label, "MyRepo", "root label stays the cwd basename");
    }

    /// `est_cost_usd` bills cache reads ~10× cheaper than fresh input: for opus
    /// (input $15/1M, cache-read $1.50/1M), 1M cache-read tokens cost ≈ $1.50, NOT
    /// the $15.00 the old "full input rate on the whole sum" path produced. Fresh
    /// input still bills at the full input rate; output at the output rate.
    #[test]
    fn est_cost_bills_cache_read_cheaper_than_fresh() {
        let model = Some("claude-opus-4-8".to_string());

        // Pure cache-read: 1M tokens at the cache-read rate (~0.1× input).
        let cache_only = composition::ExactComposition {
            cache_read: 1_000_000,
            fresh: 0,
            output: 0,
        };
        let cost = est_cost_usd(&model, &cache_only).expect("opus → a cost");
        assert!(
            (cost - 1.50).abs() < 1e-6,
            "1M cache-read tokens bill at the cache-read rate (~$1.50), got {cost}"
        );

        // Pure fresh input: 1M tokens at the full input rate.
        let fresh_only = composition::ExactComposition {
            cache_read: 0,
            fresh: 1_000_000,
            output: 0,
        };
        let fresh_cost = est_cost_usd(&model, &fresh_only).expect("opus → a cost");
        assert!(
            (fresh_cost - 15.0).abs() < 1e-6,
            "1M fresh tokens bill at the input rate ($15.00), got {fresh_cost}"
        );

        // Cache reads are strictly cheaper than the same volume of fresh input.
        assert!(cost < fresh_cost, "cache reads must be cheaper than fresh input");

        // Unknown model stays nullable.
        assert_eq!(est_cost_usd(&Some("mystery".into()), &cache_only), None);
    }

    /// A session with no `TokenUsage` reports zero occupancy and a `null` estimated
    /// composition (no turn-1 baseline) — honest, never fabricated.
    #[test]
    fn assemble_session_without_usage_is_zeroed_and_unestimated() {
        let store = Store::memory().unwrap();
        seed(&store, "s", "e", Harness::Codex, Some("/tmp/proj"), None);
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, Utc::now());
        let a = &state.agents[0];
        assert_eq!(a.context_tokens, 0);
        assert_eq!(a.fill_pct, 0.0);
        assert!(a.composition.estimated.is_none(), "no baseline → null estimate");
        assert_eq!(a.est_cost_usd, None, "no model → no cost");
    }
}
