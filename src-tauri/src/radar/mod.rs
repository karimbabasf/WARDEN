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
use std::collections::{HashMap, HashSet};
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
    /// The agent's project-folder basename (root only), e.g. `WARDEN`. Carried
    /// separately from `label` so the FACE can render a "folder · model" subtitle
    /// even when `label` is the agent's task. `None` when there is no project cwd.
    pub cwd: Option<String>,
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
    is_codex_open: &dyn Fn(&Session) -> bool,
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

    // parent link per session id (None = root). Built for every stored session so
    // we can resolve a subagent's root before deciding membership.
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for s in &sessions {
        let parent = store.parent_of(&s.id).ok().flatten();
        parent_of.insert(s.id.clone(), parent);
    }

    // The OPEN FOREST: include a session ONLY if it is currently open (spec §3 "the
    // set of agent trees currently open", §5 "the live forest"). A root is directly
    // open when — Claude: its `external_id` is in the live registry partition (a dead
    // PID was already dropped by `partition_claude`); Codex: its rollout currently
    // lives under `~/.codex/sessions/` and has NOT been archived (`is_codex_open`).
    // A subagent is open iff the ROOT of its parent-chain is directly open — close
    // the root and the whole tree implodes; this also guarantees no kept subagent
    // ever dangles (its parent shares the same open root, so it is kept too).
    let directly_open = |s: &Session| -> bool {
        match s.harness {
            Harness::Codex => is_codex_open(s),
            _ => claude_status.contains_key(&s.external_id),
        }
    };
    let by_id: HashMap<&str, &Session> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let open: HashMap<String, bool> = sessions
        .iter()
        .map(|s| (s.id.clone(), root_is_open(&s.id, &parent_of, &by_id, &directly_open)))
        .collect();
    let is_open = |id: &str| open.get(id).copied().unwrap_or(false);

    // ── dedupe: one live session = one globe ────────────────────────────────────
    // A long-running Claude session is re-ingested as SEVERAL store rows that share a
    // single `external_id` (one row per compaction/segment). They are the same live
    // agent, so the forest must show ONE globe — not one per row. Collapse each
    // external_id group of OPEN sessions to a canonical row (the freshest: latest
    // started_at, then ingested_at, then id for determinism) and remember the mapping
    // so a child whose parent link points at a dropped row is re-pointed onto it.
    let mut by_ext: HashMap<&str, Vec<&Session>> = HashMap::new();
    for s in sessions.iter().filter(|s| is_open(&s.id)) {
        by_ext.entry(s.external_id.as_str()).or_default().push(s);
    }
    let mut keep: HashSet<String> = HashSet::new();
    let mut canonical: HashMap<String, String> = HashMap::new();
    for group in by_ext.values() {
        let chosen = group
            .iter()
            .copied()
            .max_by(|a, b| {
                a.started_at
                    .cmp(&b.started_at)
                    .then(a.ingested_at.cmp(&b.ingested_at))
                    .then(a.id.cmp(&b.id))
            })
            .expect("group is non-empty");
        keep.insert(chosen.id.clone());
        for s in group {
            canonical.insert(s.id.clone(), chosen.id.clone());
        }
    }

    // Parent map over the KEPT set, with dropped-duplicate parents remapped onto their
    // canonical row (so a subagent linked to a collapsed root still nests under it).
    let mut kept_parent: HashMap<String, Option<String>> = HashMap::new();
    for id in &keep {
        let rp = parent_of
            .get(id)
            .cloned()
            .flatten()
            .map(|p| canonical.get(&p).cloned().unwrap_or(p))
            .filter(|p| p != id);
        kept_parent.insert(id.clone(), rp);
    }

    // childCount over the kept set only (a closed/duplicate child never inflates it).
    let mut child_count: HashMap<String, u32> = HashMap::new();
    for id in &keep {
        if let Some(Some(p)) = kept_parent.get(id) {
            *child_count.entry(p.clone()).or_insert(0) += 1;
        }
    }

    // Build one agent per kept session. Depth is the parent-chain length within the
    // (kept) tree (root = 0). Iterate `sessions` for a stable, source-ordered forest.
    let mut agents = Vec::with_capacity(keep.len());
    for s in &sessions {
        if !keep.contains(&s.id) {
            continue;
        }
        let parent_id = kept_parent.get(&s.id).cloned().flatten();
        let depth = depth_of(&s.id, &kept_parent);
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

/// Walk a session's parent-chain to its root and report whether that root is
/// directly open. A session is a member of the live forest iff its root agent is
/// open (a subagent rides on its open root; an orphan under a closed root is
/// excluded). Bounded to avoid looping on a malformed cycle; a chain whose parent
/// id is absent from the store is treated as ending at the current node (root).
fn root_is_open(
    id: &str,
    parent_of: &HashMap<String, Option<String>>,
    by_id: &HashMap<&str, &Session>,
    directly_open: &dyn Fn(&Session) -> bool,
) -> bool {
    let mut cur = id.to_string();
    for _ in 0..64 {
        match parent_of.get(&cur).and_then(|p| p.clone()) {
            // A parent that is not itself a stored session can't anchor a tree —
            // stop here and judge the current node as the effective root.
            Some(p) if by_id.contains_key(p.as_str()) => cur = p,
            _ => break,
        }
    }
    by_id
        .get(cur.as_str())
        .is_some_and(|root| directly_open(root))
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

    let estimated = estimate_for_session(store, s, &events, size.context_tokens);

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

/// Compute the raw, pre-calibration token sums for a session's estimated
/// composition (the EXPENSIVE part — it tokenizes the transcript). `None` when there
/// is no turn-1 `TokenUsage` baseline (checked FIRST, so a baseline-less session does
/// zero tokenization). These sums depend only on transcript content, so they are
/// safely cacheable by content hash (Fix #3).
fn compute_token_counts(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Option<crate::store::RadarTokenCounts> {
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

    Some(crate::store::RadarTokenCounts {
        turn1_total,
        first_user_tokens,
        conversation,
        tool_output,
        thinking,
    })
}

/// Derive the estimated (semantic) composition for a session, calibrated to its
/// current exact total. `None` when there is no turn-1 `TokenUsage` baseline.
///
/// Fix #3 (incremental): the expensive raw token sums are cached by
/// `(session id, content hash)`. A cache HIT skips re-tokenizing entirely; a MISS
/// (new or changed transcript) tokenizes once and upserts. The cheap, deterministic
/// `estimate_composition` calibration to the LIVE `exact_total` is always applied
/// fresh — so the output is byte-identical to tokenizing every time, only the
/// redundant tokenization is removed. Best-effort: a cache read/write failure simply
/// falls back to tokenizing (the value is never wrong, only recomputed).
fn estimate_for_session(
    store: &Store,
    session: &Session,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    exact_total: u64,
) -> Option<RadarEstimated> {
    // The content hash is the change-key: it advances whenever the transcript is
    // re-ingested with new bytes, invalidating the cache for exactly that session.
    let change_key = session.raw_hash;

    let counts = match store.radar_token_cache_get(&session.id, change_key) {
        Ok(Some(hit)) => hit, // unchanged transcript → reuse, no tokenization
        _ => {
            // Miss (or read error): tokenize once, then persist under the change-key.
            let fresh = compute_token_counts(events)?;
            let _ = store.radar_token_cache_put(&session.id, change_key, &fresh);
            fresh
        }
    };

    let est = estimate_composition(
        counts.turn1_total,
        counts.first_user_tokens,
        counts.conversation,
        counts.tool_output,
        counts.thinking,
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
        if out.len() >= RECENT_ACTIVITY_N {
            break;
        }
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
    let target = if let Some(f) = s("file_path").or_else(|| s("path")).or_else(|| s("notebook_path")) {
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

/// The agent's originating task: its first non-meta user prompt, truncated to a
/// name-sized string. `None` when the session has no real user prompt yet (so the
/// label falls back to the folder). Skips `is_meta` prompts (system/tool-injected).
fn first_task(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> Option<String> {
    events.iter().find_map(|(_, e)| match &e.event {
        Event::UserPrompt { text, is_meta, .. } if !is_meta && !text.trim().is_empty() => {
            Some(crate::util::truncate_chars(text.trim(), 64))
        }
        _ => None,
    })
}

/// Identity quad: `(label, nickname, role, origin)`.
/// * Claude subagent → label = its sidecar `description`, role = its `agentType`
///   (persisted onto the child `meta` when the parent linkage is recorded);
/// * Claude root → label = its originating `task` (so several live sessions in the
///   same repo are differentiated by WHAT each is doing), falling back to cwd basename;
/// * Codex → nickname/role/origin from `session_meta`; label = nickname when set;
/// * final fallback for any harness = cwd basename → nickname → external id.
fn identity(
    s: &Session,
    task: Option<String>,
) -> (String, Option<String>, Option<String>, Option<String>) {
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

    // Task-first naming for Claude: a session in a shared repo is named by WHAT it
    // is doing (its originating prompt), not just the folder — so several live
    // sessions in the same cwd are differentiated. Codex keeps its existing
    // nickname/cwd naming (its sessions already carry good `session_meta` names).
    let task_label = if matches!(s.harness, Harness::Codex) {
        None
    } else {
        task
    };

    // Label precedence: Claude subagent description → Claude root task → cwd basename
    // → Codex nickname → external id.
    let label = claude_description
        .or(task_label)
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
    // The Codex live set is the set of rollout uuids whose file currently sits under
    // `~/.codex/sessions/` (and NOT under `~/.codex/archived_sessions/`). We scan the
    // two roots ONCE here, then close over the resulting set so `assemble` stays a
    // pure join (no per-session FS walk inside the tested path). `source_path` in the
    // store can be stale after Codex moves a rollout to the archive, so membership is
    // decided by the CURRENT on-disk location, never by the stored path.
    let live_codex = live_codex_rollout_ids(
        &crate::util::default_codex_sessions(),
        &crate::util::default_codex_archived_sessions(),
    );
    let is_codex_open = |s: &Session| live_codex.contains(s.external_id.as_str());
    assemble(
        store,
        sessions_dir,
        &liveness::pid_alive,
        &is_codex_open,
        Utc::now(),
    )
}

/// Scan the two Codex roots and return the set of rollout UUIDs that are currently
/// OPEN — i.e. present under `sessions_dir` and absent from `archived_dir`. The
/// archive move is Codex's "done" signal (spec §4.3), so an id in the archive is
/// closed even if a stale `sessions/` copy lingers. Thin FS wrapper, kept OUT of the
/// unit-tested path (`assemble` receives the resolved set as a closure). A missing
/// dir contributes nothing (yields an empty contribution, not an error).
fn live_codex_rollout_ids(
    sessions_dir: &Path,
    archived_dir: &Path,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let scan = |root: &Path| -> HashSet<String> {
        let mut ids = HashSet::new();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let is_rollout = entry.file_type().is_file()
                && p.extension().map(|x| x == "jsonl").unwrap_or(false)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("rollout-"))
                    .unwrap_or(false);
            if is_rollout {
                ids.insert(crate::ingest::codex::external_id_from_filename(p));
            }
        }
        ids
    };
    let archived = scan(archived_dir);
    scan(sessions_dir)
        .into_iter()
        .filter(|id| !archived.contains(id))
        .collect()
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

    /// Build a temp Claude liveness registry dir holding a `<pid>.json` per
    /// `(pid, external_id)`, so the named root sessions count as currently OPEN under
    /// the membership filter. Returns the tempdir guard (drop = cleanup) — keep it
    /// alive for the duration of the test.
    fn claude_registry(entries: &[(u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (pid, sid) in entries {
            std::fs::write(
                dir.path().join(format!("{pid}.json")),
                serde_json::json!({ "pid": pid, "sessionId": sid, "cwd": "/work" }).to_string(),
            )
            .unwrap();
        }
        dir
    }

    /// A Codex-open predicate that treats every Codex session as open — used by
    /// tests whose subject is composition/labels/links, not membership.
    fn codex_all_open(_: &Session) -> bool {
        true
    }

    /// One live Claude session re-ingested as SEVERAL store rows (same external id,
    /// distinct store ids — the real shape of a long session crossing compaction
    /// segments) must collapse to a SINGLE globe, not one per row. Regression for the
    /// "14 globes for 6 live sessions" duplication.
    #[test]
    fn assemble_collapses_duplicate_external_id_rows_to_one_agent() {
        let store = Store::memory().unwrap();
        for i in 0..5 {
            seed(
                &store,
                &format!("dup-row-{i}"),
                "live-sid", // shared external id across all five rows
                Harness::ClaudeCode,
                Some("/Users/k/Developer/MyRepo"),
                Some((2, 100, 1000, 50, "claude-opus-4-8")),
            );
        }
        let reg = claude_registry(&[(4242, "live-sid")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        assert_eq!(
            state.agents.len(),
            1,
            "five store rows of one live session must render as ONE globe"
        );
        assert_eq!(state.agents[0].depth, 0);
        assert_eq!(state.agents[0].child_count, 0);
    }

    /// Fix #3 — INCREMENTAL token cache: re-assembling an UNCHANGED store must NOT
    /// re-tokenize. The first assemble tokenizes (cache miss) and persists the raw
    /// sums keyed by the session's content hash; the second assemble hits the cache
    /// and performs ZERO additional `tokenize_len` calls, while producing a
    /// byte-identical estimated composition. This is what drops a steady-state
    /// recompute (only the one written session changes) to ~ms.
    #[test]
    fn assemble_uses_token_cache_on_unchanged_session() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "live-sid",
            "live-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            Some((2, 13761, 331244, 2620, "claude-opus-4-8")),
        );
        let reg = claude_registry(&[(100, "live-ext")]);

        // First assemble: a cache miss → it tokenizes the transcript.
        let before1 = composition::tokenize_call_count();
        let state1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let tokenized_run1 = composition::tokenize_call_count() - before1;
        assert!(
            tokenized_run1 > 0,
            "the first assemble must tokenize (cache miss), did {tokenized_run1} calls"
        );
        let est1 = state1
            .agents
            .iter()
            .find(|a| a.id == "live-sid")
            .and_then(|a| a.composition.estimated.clone())
            .expect("estimated composition present");

        // Second assemble: the store is unchanged → cache hit → ZERO tokenization.
        let before2 = composition::tokenize_call_count();
        let state2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let tokenized_run2 = composition::tokenize_call_count() - before2;
        assert_eq!(
            tokenized_run2, 0,
            "an unchanged session must NOT re-tokenize on the second assemble, did {tokenized_run2}"
        );
        let est2 = state2
            .agents
            .iter()
            .find(|a| a.id == "live-sid")
            .and_then(|a| a.composition.estimated.clone())
            .expect("estimated composition present");

        assert_eq!(
            est1, est2,
            "the cached estimated composition must be byte-identical to the freshly-tokenized one"
        );
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

        // The root is registered as open (its external id is in the live registry);
        // the child rides on its open root. is_alive=true; codex predicate unused.
        let reg = claude_registry(&[(100, "root-ext")]);
        let now = Utc::now();
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);

        assert_eq!(state.agents.len(), 2);
        let root = state
            .agents
            .iter()
            .find(|a| a.id == "root-sid")
            .expect("root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(root.child_count, 1, "root has one linked child");
        assert_eq!(
            root.label, "do the thing",
            "a Claude root is labeled by its originating task"
        );
        assert_eq!(
            root.cwd.as_deref(),
            Some("MyRepo"),
            "the cwd basename is still exposed for the folder subtitle"
        );
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

        // Re-derive linkage as `recompute_radar_state` does, then assemble with the
        // two Codex sessions injected as open (membership decided by the closure, so
        // the test does not depend on real ~/.codex rollouts on disk).
        relink_store_subagents(&store);
        let open_ids = ["thread-parent", "thread-child"];
        let is_codex_open = |s: &Session| open_ids.contains(&s.external_id.as_str());
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            Utc::now(),
        );

        // Recompute re-derived the link and persisted it.
        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            Some("cx-parent".to_string()),
            "relink must persist the newly-resolvable parent"
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

    /// The "what is it doing" signal: a tool call's recent-activity label names its
    /// TARGET (file path basename / command), not just the bare tool name, and the
    /// opaque `result <call_id>` rows are dropped (they were pure noise). Built from
    /// the real `Event::ToolCall.input` shapes for Claude (`file_path`/`command`) and
    /// Codex (`cmd`).
    #[test]
    fn recent_activity_names_tool_targets_and_drops_result_rows() {
        let now = Utc::now();
        let turn = Turn {
            id: "t".into(),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let mk = |i: u64, event: Event| {
            (
                turn.clone(),
                EventRecord {
                    id: format!("e{i}"),
                    turn_id: "t".into(),
                    session_id: "s".into(),
                    ts: now,
                    event,
                    raw_ref: RawRef {
                        source_path: PathBuf::from("/x.jsonl"),
                        offset: i,
                        line: i as u32,
                    },
                },
            )
        };
        let events = vec![
            mk(
                1,
                Event::ToolCall {
                    tool: "Read".into(),
                    input: serde_json::json!({"file_path":"/Users/k/WARDEN/src/viz/orbLayout.ts"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            mk(
                2,
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: ToolStatus::Ok,
                    bytes: 10,
                    summary: None,
                },
            ),
            mk(
                3,
                Event::ToolCall {
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"cargo test radar"}),
                    call_id: "c2".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            mk(
                4,
                Event::ToolCall {
                    tool: "exec_command".into(),
                    input: serde_json::json!({"cmd":"cargo build","workdir":"/Users/k/WARDEN"}),
                    call_id: "c3".into(),
                    kind: ToolKind::Builtin,
                },
            ),
        ];
        let acts = recent_activity(&events);
        assert!(
            acts.iter().all(|a| !a.label.starts_with("result ")),
            "opaque `result <id>` rows must be dropped, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("orbLayout.ts")),
            "a Read must name the file it touches, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("cargo test radar")),
            "a Bash must name the command it runs, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("cargo build")),
            "a Codex exec_command must name the command it runs, got {acts:?}"
        );
    }

    /// Naming: a Claude ROOT agent is named by its originating task (its first
    /// non-meta user prompt), not merely the cwd basename — so several live sessions
    /// in the same repo are differentiated by what each is doing. The folder basename
    /// is still exposed (as `cwd`) for the secondary "folder · model" subtitle.
    #[test]
    fn claude_root_label_is_its_task_with_cwd_exposed() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "r",
            "r-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/WARDEN"),
            Some((2, 100, 1000, 50, "claude-opus-4-8")),
        );
        let reg = claude_registry(&[(100, "r-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let a = state.agents.iter().find(|a| a.id == "r").expect("root present");
        assert_eq!(
            a.label, "do the thing",
            "a Claude root is named by its originating task, not the cwd basename"
        );
        assert_eq!(
            a.cwd.as_deref(),
            Some("WARDEN"),
            "the folder basename is exposed as `cwd` for the subtitle"
        );
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

        let reg = claude_registry(&[(100, "p-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
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

        // Root is labeled by its task; its folder is still exposed as `cwd`.
        let p = state.agents.iter().find(|a| a.id == "p-sid").unwrap();
        assert_eq!(p.label, "do the thing", "root label is its originating task");
        assert_eq!(p.cwd.as_deref(), Some("MyRepo"), "root still exposes its cwd");
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
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &codex_all_open, Utc::now());
        let a = &state.agents[0];
        assert_eq!(a.context_tokens, 0);
        assert_eq!(a.fill_pct, 0.0);
        assert!(a.composition.estimated.is_none(), "no baseline → null estimate");
        assert_eq!(a.est_cost_usd, None, "no model → no cost");
    }

    /// THE FIX (spec §3/§5: the forest is the OPEN set). A backfilled Claude session
    /// whose external id is NOT in the live registry is EXCLUDED — the archive of
    /// every transcript ever ingested must not render. A Claude session that IS in the
    /// registry is included, status from mtime (idle here, with no fresh write).
    #[test]
    fn claude_forest_includes_only_registry_open_sessions() {
        let store = Store::memory().unwrap();
        // Historical/backfill session: ingested long ago, no live registry entry.
        seed(&store, "hist", "hist-ext", Harness::ClaudeCode, Some("/tmp/old"), None);
        // Currently-open session: a live `<pid>.json` references its session id.
        seed(&store, "live", "live-ext", Harness::ClaudeCode, Some("/tmp/now"), None);

        let reg = claude_registry(&[(100, "live-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        assert_eq!(state.agents.len(), 1, "only the registry-open session is in the forest");
        let a = &state.agents[0];
        assert_eq!(a.id, "live", "the open session is the live one, not the backfill");
        assert_eq!(a.status, "idle", "open but no fresh transcript write → idle");
        assert!(
            !state.agents.iter().any(|a| a.id == "hist"),
            "the historical/backfill session must be excluded"
        );
    }

    /// THE FIX (spec §4.3: the archive move is the Codex 'done' signal). A Codex
    /// session whose rollout is archived (closed) is EXCLUDED; a non-archived rollout
    /// is included. Membership rides on the injected `is_codex_open` closure — the
    /// real collector resolves it from the on-disk location, never the stale
    /// `source_path`.
    #[test]
    fn codex_forest_excludes_archived_sessions() {
        let store = Store::memory().unwrap();
        seed(&store, "open-cx", "open-uuid", Harness::Codex, Some("/tmp/p1"), None);
        seed(&store, "done-cx", "done-uuid", Harness::Codex, Some("/tmp/p2"), None);

        // Only `open-uuid` currently lives under sessions/ (done-uuid was archived).
        let is_codex_open = |s: &Session| s.external_id == "open-uuid";
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &is_codex_open, Utc::now());

        assert_eq!(state.agents.len(), 1, "only the non-archived Codex session is open");
        assert_eq!(state.agents[0].id, "open-cx");
        assert!(
            !state.agents.iter().any(|a| a.id == "done-cx"),
            "an archived (closed) Codex session must be excluded"
        );
    }

    /// THE FIX (subagent rule): an OPEN root with an open subagent still links
    /// (depth/childCount intact). A root EXCLUDED for being closed takes its
    /// now-orphaned subagent out too — assert the subagent is gone AND no surviving
    /// agent dangles a `parentId` pointing at a non-present parent.
    #[test]
    fn closed_root_drops_orphaned_subagents_no_dangling_parent() {
        let store = Store::memory().unwrap();
        // Open tree: root `op-root` (in registry) + Claude subagent `op-sub`.
        seed(&store, "op-root", "op-root-ext", Harness::ClaudeCode, Some("/tmp/a"), None);
        seed(&store, "op-sub", "op-sub-ext", Harness::ClaudeCode, None, None);
        store.link_child_session("op-sub", "op-root").unwrap();
        // Closed tree: root `cl-root` (NOT in registry) + subagent `cl-sub`.
        seed(&store, "cl-root", "cl-root-ext", Harness::ClaudeCode, Some("/tmp/b"), None);
        seed(&store, "cl-sub", "cl-sub-ext", Harness::ClaudeCode, None, None);
        store.link_child_session("cl-sub", "cl-root").unwrap();

        // Only the open root is registered alive.
        let reg = claude_registry(&[(100, "op-root-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        // The open tree survives, nested and counted.
        let root = state.agents.iter().find(|a| a.id == "op-root").expect("open root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(root.child_count, 1, "open root counts its one open subagent");
        let sub = state.agents.iter().find(|a| a.id == "op-sub").expect("open subagent present");
        assert_eq!(sub.depth, 1, "subagent rides on its open root, nested");
        assert_eq!(sub.parent_id.as_deref(), Some("op-root"));

        // The closed tree is gone entirely (root AND its orphaned subagent).
        assert!(!state.agents.iter().any(|a| a.id == "cl-root"), "closed root excluded");
        assert!(
            !state.agents.iter().any(|a| a.id == "cl-sub"),
            "a subagent under a closed root is excluded, not orphaned"
        );

        // No surviving agent points at a parent that isn't itself in the forest.
        let present: std::collections::HashSet<&str> =
            state.agents.iter().map(|a| a.id.as_str()).collect();
        for a in &state.agents {
            if let Some(p) = &a.parent_id {
                assert!(
                    present.contains(p.as_str()),
                    "agent {} dangles parentId {} not in the forest",
                    a.id,
                    p
                );
            }
        }
    }
}
