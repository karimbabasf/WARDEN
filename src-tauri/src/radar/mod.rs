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

use crate::ir::{Event, EventRecord, Harness, Session, ToolKind};
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
    pub context_breakdown: RadarContextBreakdown,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarContextBreakdown {
    pub used_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
    pub rows: Vec<RadarContextRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarContextRow {
    pub key: String,
    pub label: String,
    pub tokens: u64,
    pub percent: f64,
    pub count: Option<u32>,
    pub muted: bool,
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
    let mtime_secs_ago = |sid: &str| transcript_mtime_secs_ago(&sessions, sid, now);
    // FAULT B: when the registry carries no authoritative `status`, decide working/idle
    // from the session's CONVERSATION STATE (its last ingested event), not file mtime —
    // deterministic across reads, so the working↔idle flicker is gone. The closure
    // bridges the registry's external `sessionId` → the store row → its events.
    let stale_secs = crate::util::radar_working_stale_secs();
    let fallback_status = |ext: &str| {
        claude_conversation_status(store, &sessions, ext, now, stale_secs, &mtime_secs_ago)
    };
    let live = partition_claude(&registry, is_alive, &fallback_status);
    let claude_status: HashMap<String, AgentStatus> =
        live.into_iter().map(|(s, st)| (s.session_id, st)).collect();

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
        .map(|s| {
            (
                s.id.clone(),
                root_is_open(&s.id, &parent_of, &by_id, &directly_open),
            )
        })
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
                (!is_subagent_transcript_path(&a.source_path))
                    .cmp(&(!is_subagent_transcript_path(&b.source_path)))
                    .then(
                        a.started_at
                            .cmp(&b.started_at)
                            .then(a.ingested_at.cmp(&b.ingested_at))
                            .then(a.id.cmp(&b.id)),
                    )
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

    // ── subagent termination ─────────────────────────────────────────────────────
    // A subagent has no PID, so liveness can't see it finish. Derive it: the parent
    // logged a tool-result for the subagent's call (permanent ⇒ idempotent), or the
    // subagent fell silent past the backstop. Within a grace window we EMIT it as
    // `terminated` (the FACE implodes it); past the window we DROP it from the forest
    // so it never lingers as idle and never resurrects.
    let terminate_ms = crate::util::radar_subagent_terminate_ms();
    let grace_ms = crate::util::radar_terminate_grace_ms();
    let mut terminated_now: HashSet<String> = HashSet::new();
    let mut terminated_drop: HashSet<String> = HashSet::new();
    for id in &keep {
        let Some(Some(parent)) = kept_parent.get(id) else {
            continue; // roots are never "terminated" (they Close instead)
        };
        let Some(child) = by_id.get(id.as_str()) else {
            continue;
        };
        let tid = child.meta.get("toolUseId").and_then(|v| v.as_str());
        let parent_events = store.session_events(parent).unwrap_or_default();
        let last = mtime_secs_ago(&child.external_id)
            .map(|secs| now - chrono::Duration::seconds(secs as i64));
        if let Some(ts) = subagent_terminated_at(tid, &parent_events, last, now, terminate_ms) {
            let age_ms = now.signed_duration_since(ts).num_milliseconds().max(0) as u64;
            if age_ms <= grace_ms {
                terminated_now.insert(id.clone());
            } else {
                terminated_drop.insert(id.clone());
            }
        }
    }
    // Drop past-grace terminated subagents from the forest entirely (BEFORE counts).
    if !terminated_drop.is_empty() {
        keep.retain(|id| !terminated_drop.contains(id));
        kept_parent.retain(|id, _| !terminated_drop.contains(id));
    }

    // childCount over the kept set only (a closed/duplicate child never inflates it).
    let mut child_count: HashMap<String, u32> = HashMap::new();
    for id in &keep {
        if let Some(Some(p)) = kept_parent.get(id) {
            *child_count.entry(p.clone()).or_insert(0) += 1;
        }
    }

    // Per-parent subagent ordinals (1-based by spawn order) and per-folder root
    // disambiguators (1-based by spawn order) — both over the KEPT set so the
    // numbering is stable and never counts a dropped/closed sibling.
    let started = |id: &str| by_id.get(id).map(|s| (s.started_at, s.id.clone()));
    let mut subagent_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            if let Some(Some(p)) = kept_parent.get(id) {
                by_parent.entry(p.clone()).or_default().push(id.clone());
            }
        }
        for sibs in by_parent.values_mut() {
            sibs.sort_by_key(|id| started(id));
            for (i, id) in sibs.iter().enumerate() {
                subagent_ordinal.insert(id.clone(), (i as u32) + 1);
            }
        }
    }
    let mut root_dup_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            let is_root = kept_parent.get(id).map(|p| p.is_none()).unwrap_or(true);
            if !is_root {
                continue;
            }
            let folder = by_id
                .get(id.as_str())
                .and_then(|s| s.project.as_ref())
                .and_then(|p| p.cwd.file_name())
                .map(|n| n.to_string_lossy().to_string());
            if let Some(folder) = folder {
                by_folder.entry(folder).or_default().push(id.clone());
            }
        }
        for roots in by_folder.values_mut() {
            if roots.len() < 2 {
                continue; // a lone root keeps its bare folder name
            }
            roots.sort_by_key(|id| started(id));
            for (i, id) in roots.iter().enumerate() {
                root_dup_ordinal.insert(id.clone(), (i as u32) + 1);
            }
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
        let status = if terminated_now.contains(&s.id) {
            AgentStatus::Terminated
        } else {
            agent_status(store, s, &claude_status, &mtime_secs_ago, now)
        };
        let mut agent = build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            status,
        );
        agent.label = display_label(
            depth,
            agent.cwd.as_deref(),
            subagent_ordinal.get(&s.id).copied(),
            root_dup_ordinal.get(&s.id).copied(),
            &agent.label,
        );
        agents.push(agent);
    }

    RadarState {
        generated_at: now.to_rfc3339(),
        agents,
    }
}

fn is_subagent_transcript_path(path: &Path) -> bool {
    crate::ingest::claude_code::is_subagent_session_path(path)
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

/// Status for one session: a Claude session uses the registry partition (by external
/// id); anything else (Codex, etc., and Claude rows not in the live registry — e.g.
/// subagents, which have no PID) derives working/idle from its CONVERSATION STATE
/// (last ingested event), falling back to mtime only when it has no usable events.
/// The live collector treats store-resident sessions as open/idle and lets the
/// watcher's recompute drop a session that has left the live set.
fn agent_status(
    store: &Store,
    s: &Session,
    claude_status: &HashMap<String, AgentStatus>,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
) -> AgentStatus {
    if let Some(st) = claude_status.get(&s.external_id) {
        return *st;
    }
    // FAULT B: conversation-state first (deterministic), mtime only as a last resort.
    let stale_secs = crate::util::radar_working_stale_secs();
    let working_secs = crate::util::radar_working_ms() / 1000;
    let events = store.session_events(&s.id).unwrap_or_default();
    if matches!(s.harness, Harness::Codex) {
        let has_uningested_tail = source_has_uningested_tail(store, s);
        if let Some(st) =
            codex_status_from_last_event(&events, now, stale_secs, has_uningested_tail)
        {
            return st;
        }
        if has_uningested_tail {
            return match mtime_secs_ago(&s.external_id) {
                Some(secs) if secs < working_secs => AgentStatus::Working,
                _ => AgentStatus::Idle,
            };
        }
    }
    if let Some(st) = liveness::status_from_last_event(&events, now, stale_secs) {
        return st;
    }
    // No usable events at all → fall back to the old transcript-mtime heuristic.
    match mtime_secs_ago(&s.external_id) {
        Some(secs) if secs < working_secs => AgentStatus::Working,
        _ => AgentStatus::Idle,
    }
}

fn codex_status_from_last_event(
    events: &[(crate::ir::Turn, EventRecord)],
    now: DateTime<Utc>,
    stale_secs: u64,
    has_uningested_tail: bool,
) -> Option<AgentStatus> {
    let last = latest_codex_liveness_event(events)?;
    let fresh = codex_liveness_event_is_fresh(last, now, stale_secs);
    if matches!(last.event, Event::AssistantText { .. })
        || matches!(&last.event, Event::UserPrompt { text, .. } if is_completed_task_notification(text))
    {
        return Some(if has_uningested_tail && fresh {
            AgentStatus::Working
        } else {
            AgentStatus::Idle
        });
    }
    Some(if fresh {
        AgentStatus::Working
    } else {
        AgentStatus::Idle
    })
}

fn codex_liveness_event_is_fresh(event: &EventRecord, now: DateTime<Utc>, stale_secs: u64) -> bool {
    let age_secs = now.signed_duration_since(event.ts).num_seconds().max(0) as u64;
    age_secs <= stale_secs
}

fn latest_codex_liveness_event(events: &[(crate::ir::Turn, EventRecord)]) -> Option<&EventRecord> {
    events
        .iter()
        .filter(|(_, e)| codex_liveness_priority(&e.event).is_some())
        .max_by(|(_, a), (_, b)| {
            a.ts.cmp(&b.ts)
                .then(a.raw_ref.offset.cmp(&b.raw_ref.offset))
                .then(codex_liveness_priority(&a.event).cmp(&codex_liveness_priority(&b.event)))
                .then(a.id.cmp(&b.id))
        })
        .map(|(_, e)| e)
}

fn codex_liveness_priority(event: &Event) -> Option<u8> {
    match event {
        Event::UserPrompt { .. } => Some(3),
        Event::ToolCall { .. } => Some(3),
        Event::FileSnapshot { .. } => Some(2),
        Event::ToolResult { .. } => Some(2),
        Event::AssistantText { .. } => Some(1),
        _ => None,
    }
}

fn is_completed_task_notification(text: &str) -> bool {
    text.contains("<task-notification>") && text.contains("<status>completed</status>")
}

fn source_has_uningested_tail(store: &Store, s: &Session) -> bool {
    let Ok(watermark) = store.watermark_offset(&s.source_path) else {
        return false;
    };
    std::fs::metadata(&s.source_path)
        .map(|m| m.len() > watermark)
        .unwrap_or(false)
}

/// Decide a Claude session's working/idle status from its CONVERSATION STATE (Fault B):
/// resolve the registry's external `sessionId` → the store row → its last ingested
/// event via [`liveness::status_from_last_event`]. Falls back to the transcript-mtime
/// heuristic ONLY when the session has no usable events (or is not in the store). This
/// is deterministic across reads — the property that removes the working↔idle flicker.
fn claude_conversation_status(
    store: &Store,
    sessions: &[Session],
    external_id: &str,
    now: DateTime<Utc>,
    stale_secs: u64,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
) -> AgentStatus {
    // The registry keys by external `sessionId`; the store keys events by the internal
    // id. A long Claude session is re-ingested as SEVERAL store rows sharing one external
    // id (one row per compaction segment), and — contrary to an earlier assumption — the
    // conversational tail is NOT replicated across them: each row holds only its segment's
    // events. So `find()`-first (which, given `sessions()` orders by `started_at DESC`,
    // returns the row with the latest START, not the freshest TAIL) can read a stale
    // segment and mislabel a live agent idle. Evaluate the row whose LAST event is the
    // most recent — that segment carries the agent's true current state.
    let working_secs = crate::util::radar_working_ms() / 1000;
    let freshest = sessions
        .iter()
        .filter(|s| s.external_id == external_id)
        .filter_map(|s| {
            let events = store.session_events(&s.id).unwrap_or_default();
            let last_ts = events
                .iter()
                .rev()
                .find(|(_, e)| {
                    matches!(
                        e.event,
                        Event::UserPrompt { .. }
                            | Event::ToolCall { .. }
                            | Event::ToolResult { .. }
                            | Event::AssistantText { .. }
                            | Event::TokenUsage { .. }
                    )
                })
                .map(|(_, e)| e.ts)?;
            liveness::status_from_last_event(&events, now, stale_secs).map(|st| (last_ts, st))
        })
        .max_by_key(|(ts, _)| *ts);
    if let Some((_, st)) = freshest {
        return st;
    }
    // No row / no usable events → old transcript-mtime heuristic (last resort).
    match mtime_secs_ago(external_id) {
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

#[derive(Default)]
struct ToolContextStats {
    mcp_count: u32,
    system_count: u32,
    custom_count: u32,
    mcp_raw: u64,
    system_raw: u64,
    custom_raw: u64,
}

fn context_breakdown(
    harness: Harness,
    size: ContextSize,
    estimated: Option<RadarEstimated>,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> RadarContextBreakdown {
    let used = size.context_tokens;
    let max = size.max_tokens;
    let rows = match estimated {
        Some(est) => match harness {
            Harness::Codex => codex_context_rows(max, used, &est, events, pending_tail_tokens),
            _ => claude_context_rows(max, used, &est, events, pending_tail_tokens),
        },
        None => fallback_context_rows(max, used, pending_tail_tokens),
    };

    RadarContextBreakdown {
        used_tokens: used,
        max_tokens: max,
        fill_pct: size.fill_pct,
        rows,
    }
}

fn fallback_context_rows(max: u64, used: u64, pending_tail_tokens: u64) -> Vec<RadarContextRow> {
    let base = used.saturating_sub(pending_tail_tokens);
    let mut rows = vec![context_row("context", "Context", base, max, None, false)];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn claude_context_rows(
    max: u64,
    used: u64,
    est: &RadarEstimated,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> Vec<RadarContextRow> {
    let tools = tool_context_stats(events, est.tool_output);
    let memory_count = memory_file_count(events);
    let (skills, memory, system_prompt, deferred_mcp, deferred_system) = split_preamble_for_claude(
        est.preamble,
        memory_count,
        tools.mcp_count,
        tools.system_count,
    );

    let mut rows = vec![
        context_row("messages", "Messages", est.conversation, max, None, false),
        context_row("skills", "Skills", skills, max, None, false),
        context_row(
            "mcp_tools",
            "MCP tools",
            tools.mcp_raw,
            max,
            count_if_nonzero(tools.mcp_count),
            false,
        ),
        context_row(
            "memory_files",
            "Memory files",
            memory,
            max,
            count_if_nonzero(memory_count),
            false,
        ),
        context_row(
            "system_prompt",
            "System prompt",
            system_prompt,
            max,
            None,
            false,
        ),
        context_row(
            "system_tools",
            "System tools",
            tools.system_raw,
            max,
            count_if_nonzero(tools.system_count),
            false,
        ),
        context_row(
            "custom_agents",
            "Custom agents",
            tools.custom_raw,
            max,
            count_if_nonzero(tools.custom_count),
            false,
        ),
        context_row(
            "mcp_tools_deferred",
            "MCP tools (deferred)",
            deferred_mcp,
            max,
            None,
            true,
        ),
        context_row(
            "system_tools_deferred",
            "System tools (deferred)",
            deferred_system,
            max,
            None,
            true,
        ),
    ];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn codex_context_rows(
    max: u64,
    used: u64,
    est: &RadarEstimated,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> Vec<RadarContextRow> {
    let tools = tool_context_stats(events, est.tool_output);
    let mut rows = vec![
        context_row("messages", "Messages", est.conversation, max, None, false),
        context_row("reasoning", "Reasoning", est.thinking, max, None, false),
        context_row(
            "function_tools",
            "Function tools",
            tools.system_raw,
            max,
            count_if_nonzero(tools.system_count),
            false,
        ),
        context_row(
            "mcp_tools",
            "MCP tools",
            tools.mcp_raw,
            max,
            count_if_nonzero(tools.mcp_count),
            false,
        ),
        context_row(
            "custom_tools",
            "Custom tools",
            tools.custom_raw,
            max,
            count_if_nonzero(tools.custom_count),
            false,
        ),
        context_row(
            "base_instructions",
            "Base instructions",
            est.preamble,
            max,
            None,
            false,
        ),
    ];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn append_pending_and_free_rows(
    rows: &mut Vec<RadarContextRow>,
    max: u64,
    used: u64,
    pending_tail_tokens: u64,
) {
    if pending_tail_tokens > 0 {
        rows.push(context_row(
            "pending_tail",
            "Pending tail (est.)",
            pending_tail_tokens,
            max,
            None,
            false,
        ));
    }
    if max > 0 {
        rows.push(context_row(
            "free_space",
            "Free space",
            max.saturating_sub(used),
            max,
            None,
            true,
        ));
    }
}

fn context_row(
    key: &str,
    label: &str,
    tokens: u64,
    max: u64,
    count: Option<u32>,
    muted: bool,
) -> RadarContextRow {
    RadarContextRow {
        key: key.to_string(),
        label: label.to_string(),
        tokens,
        percent: if max == 0 {
            0.0
        } else {
            (tokens as f64 / max as f64).clamp(0.0, 1.0)
        },
        count,
        muted,
    }
}

fn count_if_nonzero(n: u32) -> Option<u32> {
    (n > 0).then_some(n)
}

fn split_preamble_for_claude(
    preamble: u64,
    memory_count: u32,
    mcp_count: u32,
    system_count: u32,
) -> (u64, u64, u64, u64, u64) {
    if preamble == 0 {
        return (0, 0, 0, 0, 0);
    }
    let weights = [
        4 + u64::from(mcp_count > 0),
        1 + memory_count as u64,
        5,
        u64::from(mcp_count),
        u64::from(system_count),
    ];
    let split = allocate_by_weights(preamble, weights);
    (split[0], split[1], split[2], split[3], split[4])
}

fn allocate_by_weights<const N: usize>(total: u64, weights: [u64; N]) -> [u64; N] {
    let sum: u64 = weights.iter().sum();
    if total == 0 || sum == 0 {
        return [0; N];
    }
    let mut out = [0; N];
    let mut assigned = 0u64;
    for (i, weight) in weights.iter().enumerate() {
        out[i] = total.saturating_mul(*weight) / sum;
        assigned = assigned.saturating_add(out[i]);
    }
    if assigned < total {
        out[0] = out[0].saturating_add(total - assigned);
    }
    out
}

fn tool_context_stats(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    tool_output_tokens: u64,
) -> ToolContextStats {
    let mut call_kind: HashMap<String, ToolKind> = HashMap::new();
    let mut stats = ToolContextStats::default();
    let mut raw = [0u64; 3];

    for (_, e) in events {
        match &e.event {
            Event::ToolCall { call_id, kind, .. } => {
                call_kind.insert(call_id.clone(), kind.clone());
                match kind {
                    ToolKind::Mcp => stats.mcp_count += 1,
                    ToolKind::SubagentTask => stats.custom_count += 1,
                    _ => stats.system_count += 1,
                }
            }
            Event::ToolResult { call_id, bytes, .. } => {
                match call_kind.get(call_id).unwrap_or(&ToolKind::Unknown) {
                    ToolKind::Mcp => raw[0] += *bytes,
                    ToolKind::SubagentTask => raw[2] += *bytes,
                    _ => raw[1] += *bytes,
                }
            }
            _ => {}
        }
    }

    if raw.iter().all(|n| *n == 0) {
        raw = [
            u64::from(stats.mcp_count),
            u64::from(stats.system_count),
            u64::from(stats.custom_count),
        ];
    }
    let split = allocate_by_weights(tool_output_tokens, raw);
    stats.mcp_raw = split[0];
    stats.system_raw = split[1];
    stats.custom_raw = split[2];
    stats
}

fn memory_file_count(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> u32 {
    let count = events
        .iter()
        .map(|(_, e)| match &e.event {
            Event::FileSnapshot { files } => files.len(),
            Event::UserPrompt { attachments, .. } => attachments.len(),
            _ => 0,
        })
        .sum::<usize>();
    count.min(u32::MAX as usize) as u32
}

/// Every action event as a recent-activity row (newest first): a kind glyph-friendly
/// `kind` plus a short label. No cap — the detail panel shows ~10 rows in a
/// scrollable feed and lets you scroll back to the very first action.
fn recent_activity(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> Vec<RadarActivity> {
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

/// The agent's originating task: its first non-meta user prompt, truncated to a
/// name-sized string. `None` when the session has no real user prompt yet (so the
/// label falls back to the folder). Skips `is_meta` prompts (system/tool-injected).
fn first_task(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> Option<String> {
    events.iter().find_map(|(_, e)| match &e.event {
        Event::UserPrompt { text, is_meta, .. } if !is_meta && !text.trim().is_empty() => {
            let name = clean_task_label(text);
            (!name.is_empty()).then_some(name)
        }
        _ => None,
    })
}

/// Clean a raw user prompt into a globe-sized agent name: collapse ALL whitespace
/// (a multi-line prompt becomes one line), drop a leading `@file`/URL token when real
/// text follows (so the name is WHAT the agent is doing, not an attachment path), and
/// truncate to a name-sized length. Returns "" only for empty/whitespace input.
fn clean_task_label(raw: &str) -> String {
    // Collapse newlines/tabs/runs-of-spaces into single spaces so a multi-line prompt
    // renders as one clean name.
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    // Drop a leading attachment/URL token when real text follows it, so the name is
    // the task — not a pasted path or link. Only the first token, only if text remains.
    let cleaned = match collapsed.split_once(' ') {
        Some((head, rest)) if is_noise_token(head) && !rest.trim().is_empty() => rest.trim(),
        _ => collapsed.as_str(),
    };
    crate::util::truncate_chars(cleaned, 60)
}

/// The radar display label. Roots are named by their project folder; subagents by a
/// per-parent ordinal ("subagent N"). `root_dup_ordinal` is `Some(n)` only when
/// several live roots share `cwd_basename` — `n == 1` (the oldest) keeps the bare
/// name, `n >= 2` gets a circled disambiguator. `fallback` is the identity-derived
/// label, used only for a root with no project folder.
fn display_label(
    depth: u32,
    cwd_basename: Option<&str>,
    subagent_ordinal: Option<u32>,
    root_dup_ordinal: Option<u32>,
    fallback: &str,
) -> String {
    if depth >= 1 {
        return format!("subagent {}", subagent_ordinal.unwrap_or(1));
    }
    match cwd_basename {
        Some(name) if !name.is_empty() => match root_dup_ordinal {
            Some(n) if n >= 2 => format!("{name} {}", circled(n)),
            _ => name.to_string(),
        },
        _ => fallback.to_string(),
    }
}

/// Circled-number glyph for 2..=20 (② = U+2461 = U+2460 + (n-1)), else " (n)".
fn circled(n: u32) -> String {
    if (2..=20).contains(&n) {
        char::from_u32(0x2460 + (n - 1))
            .map(|c| c.to_string())
            .unwrap_or_else(|| format!("({n})"))
    } else {
        format!("({n})")
    }
}

/// When a subagent became terminated, or `None` if still live.
/// Primary: the parent logged a tool RESULT for the subagent's `tool_use_id` →
/// terminated at the result's timestamp (a permanent transcript fact ⇒ idempotent
/// across recomputes). Backstop: no result, but the subagent has been silent longer
/// than `terminate_ms` while its parent is alive → terminated at `last + terminate_ms`.
fn subagent_terminated_at(
    tool_use_id: Option<&str>,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    child_last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    terminate_ms: u64,
) -> Option<DateTime<Utc>> {
    if let Some(tid) = tool_use_id {
        if let Some(ts) = parent_events
            .iter()
            .filter_map(|(_, e)| match &e.event {
                Event::UserPrompt { text, .. } if task_notification_completed_for(text, tid) => {
                    Some(e.ts)
                }
                _ => None,
            })
            .max()
        {
            return Some(ts);
        }
        if let Some(ts) = parent_events
            .iter()
            .filter_map(|(_, e)| match &e.event {
                Event::ToolResult {
                    call_id, summary, ..
                } if call_id == tid && !is_async_agent_launch_summary(summary.as_deref()) => {
                    Some(e.ts)
                }
                _ => None,
            })
            .max()
        {
            return Some(ts);
        }
    }
    let last = child_last_activity?;
    let quiet_ms = now.signed_duration_since(last).num_milliseconds().max(0) as u64;
    (quiet_ms > terminate_ms).then(|| last + chrono::Duration::milliseconds(terminate_ms as i64))
}

fn task_notification_completed_for(text: &str, tool_use_id: &str) -> bool {
    text.contains("<task-notification>")
        && text.contains(&format!("<tool-use-id>{tool_use_id}</tool-use-id>"))
        && text.contains("<status>completed</status>")
}

fn is_async_agent_launch_summary(summary: Option<&str>) -> bool {
    let Some(summary) = summary else {
        return false;
    };
    summary.contains("Async agent launched successfully")
        || summary.contains("The agent is working in the background")
}

/// A leading prompt token that is an attachment path (`@…`) or a bare URL — noise to
/// drop from an agent name when the real prompt text follows it.
fn is_noise_token(tok: &str) -> bool {
    tok.starts_with('@') || tok.starts_with("http://") || tok.starts_with("https://")
}

#[cfg(test)]
mod naming_tests {
    use super::clean_task_label;

    #[test]
    fn collapses_internal_whitespace_and_newlines() {
        assert_eq!(
            clean_task_label("  fix   the\n\nradar  glow "),
            "fix the radar glow"
        );
    }

    #[test]
    fn strips_leading_at_file_mention() {
        assert_eq!(
            clean_task_label("@/Users/k/Desktop/MOBIUS-intro.mp4 turn this into a launch video"),
            "turn this into a launch video"
        );
    }

    #[test]
    fn strips_leading_quoted_at_file_mention() {
        assert_eq!(
            clean_task_label("@\"/Users/k/clip.mp4\" This video needs captions"),
            "This video needs captions"
        );
    }

    #[test]
    fn strips_leading_bare_url() {
        assert_eq!(
            clean_task_label("https://github.com/foo/bar Can you review this repo"),
            "Can you review this repo"
        );
    }

    #[test]
    fn keeps_leading_token_when_it_is_the_whole_prompt() {
        // Nothing meaningful follows → keep the original rather than an empty name.
        assert_eq!(
            clean_task_label("@/Users/k/only-a-path.txt"),
            "@/Users/k/only-a-path.txt"
        );
    }

    #[test]
    fn truncates_to_name_size_with_ellipsis() {
        let long = "design a comprehensive multi agent orchestration radar with glow and tethers and side panels";
        let out = clean_task_label(long);
        assert!(
            out.chars().count() <= 60,
            "got {} chars: {out:?}",
            out.chars().count()
        );
        assert!(
            out.ends_with('…'),
            "long label should be ellipsized: {out:?}"
        );
    }

    #[test]
    fn empty_or_whitespace_is_empty() {
        assert_eq!(clean_task_label("   \n  "), "");
    }
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
#[cfg(test)]
fn relink_store_subagents(store: &Store) {
    let _ = crate::ingest::codex::link_codex_subagents_in_store(store);
    let _ = crate::ingest::claude_code::link_claude_subagents_in_store(store);
}

/// Pull current live transcript tails into the store. This is intentionally
/// explicit: startup/cold-read paths can close the "agent was already running before
/// WARDEN" gap, while steady heartbeat recomputes stay read-only.
pub fn refresh_live_context(store: &Store, sessions_dir: &Path) -> usize {
    let claude_projects_dir = crate::util::default_claude_projects();
    let claude_events = refresh_live_claude_transcripts(store, &claude_projects_dir, sessions_dir);
    let codex_sessions_dir = crate::util::default_codex_sessions();
    let codex_archived_dir = crate::util::default_codex_archived_sessions();
    let codex_events = refresh_live_codex_rollouts(store, &codex_sessions_dir, &codex_archived_dir);
    claude_events + codex_events
}

/// Recompute the forest and return it. The scheduler's watcher calls this on each
/// relevant FS/liveness event; `lib.rs` then emits it as `radar_state`. Uses the real
/// `pid_alive` syscall and the current clock.
///
/// Linkage is derived when transcript bytes are ingested (startup backfill, explicit
/// live refresh, or live tail watcher). Keeping this steady-state recompute read-only
/// avoids re-reading/hashing live transcript files on every heartbeat while preserving
/// live nesting when new data actually arrives.
pub fn recompute_radar_state(store: &Store, sessions_dir: &Path) -> RadarState {
    // The Codex live set is the set of rollout uuids whose file currently sits under
    // `~/.codex/sessions/` (and NOT under `~/.codex/archived_sessions/`). We scan the
    // two roots ONCE here, then close over the resulting set so `assemble` stays a
    // pure join (no per-session FS walk inside the tested path). `source_path` in the
    // store can be stale after Codex moves a rollout to the archive, so membership is
    // decided by the CURRENT on-disk location, never by the stored path.
    let codex_sessions_dir = crate::util::default_codex_sessions();
    let codex_archived_dir = crate::util::default_codex_archived_sessions();
    let live_codex = live_codex_rollout_ids(&codex_sessions_dir, &codex_archived_dir);
    let is_codex_open = |s: &Session| live_codex.contains(s.external_id.as_str());
    assemble(
        store,
        sessions_dir,
        &liveness::pid_alive,
        &is_codex_open,
        Utc::now(),
    )
}

/// Pull current live Claude transcript tails into the store before RADAR assembles
/// the forest. The liveness registry can say "this PID/session is open" while the
/// store still holds an old tail (or no row at all) because the session predates
/// WARDEN startup and no fresh watcher event fired. Reuse the scheduler's
/// byte-watermark ingester so unchanged transcripts are cheap, while appended root
/// and subagent bytes become live context/log rows immediately.
fn refresh_live_claude_transcripts(
    store: &Store,
    projects_dir: &Path,
    sessions_dir: &Path,
) -> usize {
    let paths = live_claude_transcript_paths(projects_dir, sessions_dir);
    if paths.is_empty() {
        return 0;
    }
    let registry = crate::ingest::AdapterRegistry::from_adapters(vec![Box::new(
        crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            projects_dir.to_path_buf(),
            store.clone(),
        ),
    )]);
    let mut events = 0usize;
    for path in paths {
        match crate::scheduler::ingest_file_once(&registry, store, &path) {
            Ok(n) => events += n,
            Err(e) => tracing::warn!(
                path=%path.display(),
                error=%format!("{e:#}"),
                "live Claude radar refresh failed"
            ),
        }
    }
    if events > 0 {
        let _ = crate::ingest::claude_code::link_claude_subagents_in_store(store);
    }
    events
}

/// Pull current live Codex rollout tails into the store before RADAR assembles the
/// forest. This closes the startup gap: a rollout that was already running before
/// WARDEN launched has a file on disk, but no watcher event may fire after startup,
/// so the live-id scan alone can find the agent while the store still has stale or
/// missing context. Reuse the scheduler's byte-watermark ingester so unchanged files
/// are cheap and appended bytes become live activity rows.
fn refresh_live_codex_rollouts(store: &Store, sessions_dir: &Path, archived_dir: &Path) -> usize {
    let paths = live_codex_rollout_paths(sessions_dir, archived_dir);
    if paths.is_empty() {
        return 0;
    }
    let registry = crate::ingest::AdapterRegistry::from_adapters(vec![Box::new(
        crate::ingest::codex::CodexAdapter::with_root(
            sessions_dir.to_path_buf(),
            archived_dir.to_path_buf(),
            store.clone(),
        ),
    )]);
    let mut events = 0usize;
    for path in paths {
        match crate::scheduler::ingest_file_once(&registry, store, &path) {
            Ok(n) => events += n,
            Err(e) => tracing::warn!(
                path=%path.display(),
                error=%format!("{e:#}"),
                "live Codex radar refresh failed"
            ),
        }
    }
    if events > 0 {
        let _ = crate::ingest::codex::link_codex_subagents_in_store(store);
    }
    events
}

/// Resolve live Claude registry entries to their current transcript files. Root
/// transcripts live at `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`; Claude
/// subagent transcripts for that live root live below
/// `<encoded-cwd>/<sessionId>/subagents/**.jsonl`. Missing files are skipped: the
/// registry is the liveness source, but the transcript is the renderable context.
fn live_claude_transcript_paths(
    projects_dir: &Path,
    sessions_dir: &Path,
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (pid, v) in read_claude_registry(sessions_dir) {
        if !liveness::pid_alive(pid) {
            continue;
        }
        let Some(session_id) = v.get("sessionId").and_then(|s| s.as_str()) else {
            continue;
        };
        let Some(cwd) = v.get("cwd").and_then(|s| s.as_str()) else {
            continue;
        };
        let root = projects_dir
            .join(claude_project_dir_name(cwd))
            .join(format!("{session_id}.jsonl"));
        if !root.is_file() {
            continue;
        }
        if seen.insert(root.clone()) {
            out.push(root.clone());
        }

        let subagents = root
            .parent()
            .map(|p| p.join(session_id).join("subagents"))
            .filter(|p| p.exists());
        let Some(subagents) = subagents else {
            continue;
        };
        for entry in walkdir::WalkDir::new(subagents)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if !entry.file_type().is_file() || p.extension().map(|x| x != "jsonl").unwrap_or(true) {
                continue;
            }
            let p = entry.into_path();
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_whitespace() {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Whether a non-archived Codex rollout is recent enough to still count as "present"
/// on the radar (hybrid stale policy). Codex has no process/termination signal, so a
/// rollout the user never archived would linger forever; we hide one idle longer than
/// `stale_secs` (`WARDEN_RADAR_CODEX_STALE_HRS`, default 6h). `stale_secs == 0`
/// disables the cutoff; an unknown mtime is KEPT (never drop on a missing stat).
fn codex_fresh(mtime_secs_ago: Option<u64>, stale_secs: u64) -> bool {
    if stale_secs == 0 {
        return true; // cutoff disabled
    }
    match mtime_secs_ago {
        Some(secs) => secs <= stale_secs,
        None => true, // unknown mtime → keep (never drop on a missing stat)
    }
}

#[cfg(test)]
mod codex_stale_tests {
    use super::codex_fresh;

    #[test]
    fn keeps_recent_drops_stale_handles_unknown_and_disabled() {
        assert!(codex_fresh(Some(60), 21_600), "1m ago within 6h → keep");
        assert!(
            !codex_fresh(Some(7 * 3600), 21_600),
            "7h ago past 6h → drop"
        );
        assert!(
            codex_fresh(Some(21_600), 21_600),
            "exactly at cutoff → keep (<=)"
        );
        assert!(
            codex_fresh(None, 21_600),
            "unknown mtime → keep (never drop on a missing stat)"
        );
        assert!(
            codex_fresh(Some(999_999), 0),
            "cutoff disabled (0) → keep all"
        );
    }
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
    live_codex_rollout_paths(sessions_dir, archived_dir)
        .into_iter()
        .map(|p| crate::ingest::codex::external_id_from_filename(&p))
        .collect()
}

/// Scan Codex roots and return currently-open live rollout paths: present under
/// `sessions_dir`, absent from `archived_dir`, and still fresh under the hybrid
/// stale policy.
fn live_codex_rollout_paths(sessions_dir: &Path, archived_dir: &Path) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;
    let is_rollout = |entry: &walkdir::DirEntry| -> bool {
        let p = entry.path();
        entry.file_type().is_file()
            && p.extension().map(|x| x == "jsonl").unwrap_or(false)
            && p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("rollout-"))
                .unwrap_or(false)
    };
    // Archived rollouts are closed regardless of age — collect their ids to exclude.
    let mut archived = HashSet::new();
    for entry in walkdir::WalkDir::new(archived_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if is_rollout(&entry) {
            archived.insert(crate::ingest::codex::external_id_from_filename(
                entry.path(),
            ));
        }
    }
    // Live rollouts: under sessions/, not archived, and not stale. The freshness
    // cutoff (hybrid policy) drops a rollout the user abandoned without archiving —
    // Codex has no PID/termination signal, so mtime age is the only "still active" cue.
    let stale_secs = crate::util::radar_codex_stale_secs();
    let now = std::time::SystemTime::now();
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !is_rollout(&entry) {
            continue;
        }
        let id = crate::ingest::codex::external_id_from_filename(entry.path());
        if archived.contains(&id) {
            continue;
        }
        let mtime_secs_ago = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mt| now.duration_since(mt).ok())
            .map(|d| d.as_secs());
        if codex_fresh(mtime_secs_ago, stale_secs) {
            out.push(entry.into_path());
        }
    }
    out
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
            // A turn that produced usage also produced its assistant response: end on a
            // completed `AssistantText` (a text-only `end_turn` turn) so the semantic
            // liveness rule reads this seeded session as SETTLED/idle — the realistic
            // shape of a finished turn. (A still-working turn is `seed(.., None)`, which
            // leaves the trailing UserPrompt → working.) Distinct increasing timestamps
            // keep the stored order UserPrompt → AssistantText → TokenUsage.
            events.push(EventRecord {
                id: format!("{id}-a"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now + chrono::Duration::milliseconds(10),
                event: Event::AssistantText {
                    text: "here you go".into(),
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: 1,
                    line: 2,
                },
            });
            events.push(EventRecord {
                id: format!("{id}-u"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now + chrono::Duration::milliseconds(20),
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
                    offset: 2,
                    line: 3,
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

    #[test]
    fn assemble_prefers_root_transcript_over_subagent_duplicate_external_id() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = |id: &str, external: &str, source: &str, started_at: DateTime<Utc>| Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: external.into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at,
            ended_at: None,
            source_path: PathBuf::from(source),
            raw_hash: 0,
            ingested_at: started_at,
            meta: serde_json::json!({}),
        };
        let root = session("root", "root-ext", "/tmp/proj/root-ext.jsonl", now);
        let duplicate = session(
            "duplicate-subagent-row",
            "root-ext",
            "/tmp/proj/root-ext/subagents/agent-child.jsonl",
            now + chrono::Duration::seconds(10),
        );
        let child = session(
            "child",
            "agent-child",
            "/tmp/proj/root-ext/subagents/agent-child.jsonl",
            now + chrono::Duration::seconds(20),
        );
        store.upsert_session_batch(&root, &[], &[], 0).unwrap();
        store.upsert_session_batch(&duplicate, &[], &[], 0).unwrap();
        store.upsert_session_batch(&child, &[], &[], 0).unwrap();
        store.link_child_session("child", "root").unwrap();

        let reg = claude_registry(&[(100, "root-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);

        assert!(
            state.agents.iter().any(|a| a.id == "root"),
            "the real root transcript must remain the canonical root"
        );
        assert!(
            state
                .agents
                .iter()
                .all(|a| a.id != "duplicate-subagent-row"),
            "a stale subagent-path duplicate must not replace the root"
        );
        let child = state.agents.iter().find(|a| a.id == "child").unwrap();
        assert_eq!(child.parent_id.as_deref(), Some("root"));
    }

    #[test]
    fn codex_tail_usage_with_empty_model_uses_session_window_metadata() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = Session {
            id: "codex-tail".into(),
            harness: Harness::Codex,
            external_id: "codex-tail-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/codex-tail.jsonl"),
            raw_hash: 1,
            ingested_at: now,
            meta: serde_json::json!({ "model_context_window": 258400 }),
        };
        let turn = Turn {
            id: "codex-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let usage = EventRecord {
            id: "codex-tail-usage".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: now,
            event: Event::TokenUsage {
                input: 94_263,
                output: 153,
                cache_creation: 0,
                cache_read: 93_056,
                model: "".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[usage], 100)
            .unwrap();

        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            now,
        );
        let agent = state.agents.iter().find(|a| a.id == "codex-tail").unwrap();
        assert_eq!(agent.context_tokens, 94_263);
        assert_eq!(
            agent.max_tokens, 258_400,
            "an incremental Codex token_count has no session_meta in the tail, so an empty event model must fall back to session/window metadata"
        );
        assert!(agent.fill_pct > 0.36 && agent.fill_pct < 0.37);
    }

    #[test]
    fn context_tokens_include_estimated_tail_after_latest_usage() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "tail-growth",
            "tail-growth-ext",
            Harness::ClaudeCode,
            Some("/tmp/WARDEN"),
            Some((100, 50, 1_000, 25, "claude-sonnet-4-5")),
        );
        let mut session = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == "tail-growth")
            .unwrap();
        session.raw_hash = 2;
        let now = Utc::now();
        let turn = Turn {
            id: "tail-growth-t1".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Tool,
            index: 2,
            started_at: now + chrono::Duration::milliseconds(30),
            duration_ms: None,
            is_sidechain: false,
        };
        let tail = EventRecord {
            id: "tail-growth-tool-result".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: now + chrono::Duration::milliseconds(30),
            event: Event::ToolResult {
                call_id: "c1".into(),
                status: ToolStatus::Ok,
                bytes: 400,
                summary: Some("fresh tool output".into()),
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 3,
                line: 4,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[tail], 400)
            .unwrap();

        let reg = claude_registry(&[(4242, "tail-growth-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        let agent = state.agents.iter().find(|a| a.id == "tail-growth").unwrap();
        assert_eq!(
            agent.context_tokens, 1_250,
            "last API usage is 1150 resident tokens; the 400-byte tail should add an estimated 100 tokens"
        );
        assert!(
            agent
                .context_breakdown
                .rows
                .iter()
                .any(|row| row.key == "pending_tail" && row.tokens == 100),
            "the context window must disclose post-usage tail bytes as an estimated row"
        );
    }

    #[test]
    fn assemble_chooses_newer_tail_usage_even_when_tail_turn_index_restarts() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = Session {
            id: "tail-order".into(),
            harness: Harness::Codex,
            external_id: "tail-order-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/tail-order.jsonl"),
            raw_hash: 1,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let old_turn = Turn {
            id: "tail-order-old-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 2,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let old_usage = EventRecord {
            id: "tail-order-old-usage".into(),
            turn_id: old_turn.id.clone(),
            session_id: session.id.clone(),
            ts: now,
            event: Event::TokenUsage {
                input: 10_000,
                output: 10,
                cache_creation: 0,
                cache_read: 0,
                model: "openai".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 100,
                line: 10,
            },
        };
        store
            .upsert_session_batch(&session, &[old_turn], &[old_usage], 100)
            .unwrap();

        let mut tail_session = session.clone();
        tail_session.raw_hash = 2;
        let tail_turn = Turn {
            id: "tail-order-new-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now + chrono::Duration::seconds(1),
            duration_ms: None,
            is_sidechain: false,
        };
        let new_usage = EventRecord {
            id: "tail-order-new-usage".into(),
            turn_id: tail_turn.id.clone(),
            session_id: session.id.clone(),
            ts: now + chrono::Duration::seconds(1),
            event: Event::TokenUsage {
                input: 42_000,
                output: 10,
                cache_creation: 0,
                cache_read: 0,
                model: "".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 200,
                line: 20,
            },
        };
        store
            .upsert_session_batch(&tail_session, &[tail_turn], &[new_usage], 200)
            .unwrap();

        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            now + chrono::Duration::seconds(1),
        );
        let agent = state.agents.iter().find(|a| a.id == "tail-order").unwrap();
        assert_eq!(
            agent.context_tokens, 42_000,
            "newer live-tail usage must win even when the tail parser restarts local turn indexes"
        );
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
            root.label, "MyRepo",
            "a Claude root is labeled by its project folder (B1)"
        );
        assert_eq!(
            root.cwd.as_deref(),
            Some("MyRepo"),
            "the cwd basename is still exposed for the folder subtitle"
        );
        assert_eq!(root.context_tokens, 345_007, "2+13761+331244");
        assert!(
            (root.fill_pct - 0.345_007).abs() < 1e-6,
            "345007 / 1M Opus window ≈ 0.345 (not clamped against the old 200k)"
        );
        assert_eq!(root.composition.exact.cache_read, 331_244);
        assert_eq!(root.composition.exact.fresh, 2 + 13_761);
        assert_eq!(root.composition.exact.output, 2_620);
        assert!(
            root.composition.estimated.is_some(),
            "a turn-1 baseline yields an estimated composition"
        );
        assert_eq!(root.context_breakdown.used_tokens, 345_007);
        assert_eq!(root.context_breakdown.max_tokens, 1_000_000);
        assert!(
            root.context_breakdown
                .rows
                .iter()
                .any(|r| r.key == "messages" && r.tokens > 0),
            "context window rows must include live message occupancy"
        );
        assert!(
            root.context_breakdown
                .rows
                .iter()
                .any(|r| r.key == "free_space" && r.tokens == 1_000_000 - 345_007),
            "context window rows must include free space against the real max window"
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
        assert!(
            json.contains("\"contextTokens\""),
            "camelCase contextTokens"
        );
        assert!(
            json.contains("\"contextBreakdown\""),
            "camelCase contextBreakdown"
        );
        assert!(json.contains("\"parentId\""), "camelCase parentId");
        assert!(json.contains("\"childCount\""), "camelCase childCount");
        assert!(json.contains("\"cacheRead\""), "camelCase nested cacheRead");
        assert!(json.contains("\"generatedAt\""), "camelCase generatedAt");
    }

    /// A Codex Desktop subagent inserted into the store WITHOUT any pre-run linkage
    /// pass is linked by the explicit relink boundary (startup/live ingest), then
    /// appears as a child in the forest. Steady recomputes are read-only.
    #[test]
    fn explicit_relink_links_codex_subagent_without_pre_pass() {
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
                &mk(
                    "cx-parent",
                    "thread-parent",
                    serde_json::json!({ "originator": "Codex Desktop" }),
                ),
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

    #[test]
    fn steady_recompute_does_not_relink_without_new_ingest() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

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
        store
            .upsert_session_batch(
                &mk(
                    "cx-parent",
                    "thread-parent",
                    serde_json::json!({ "originator": "Codex Desktop" }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();
        store
            .upsert_session_batch(
                &mk(
                    "cx-child",
                    "thread-child",
                    serde_json::json!({
                        "thread_source": "subagent",
                        "parent_thread_id": "thread-parent",
                        "originator": "Codex Desktop",
                    }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let _ = recompute_radar_state(&store, registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            None,
            "heartbeat/read recomputes must not run the whole-store relinker when no new bytes were ingested"
        );
    }

    #[test]
    fn steady_recompute_does_not_ingest_live_codex_rollout_without_explicit_refresh() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

        let live_dir = sessions_root.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let path =
            live_dir.join("rollout-2026-06-25T00-00-00-019efd6c-8f60-7f42-8da1-3977122aa6be.jsonl");
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        let t1 = (now + chrono::Duration::milliseconds(100)).to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{t0}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"019efd6c-8f60-7f42-8da1-3977122aa6be\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\"}}}}\n\
                 {{\"timestamp\":\"{t1}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"do not ingest me on heartbeat\"}}}}\n",
            ),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let claude_registry = tempfile::tempdir().unwrap();
        let state = recompute_radar_state(&store, claude_registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            store.sessions().unwrap().is_empty(),
            "steady heartbeat/read recompute must not ingest transcript bytes"
        );
        assert!(
            state.agents.is_empty(),
            "without an explicit live refresh or backfill, recompute should only assemble the persisted store"
        );
    }

    /// Regression: a Codex rollout that was already open before WARDEN started must
    /// appear after the explicit startup/cold-read refresh even when the store is
    /// empty/stale. The refresh path pulls live Codex tails before assembly; ordinary
    /// heartbeat recompute remains read-only.
    #[test]
    fn explicit_refresh_ingests_live_codex_rollout_before_assembling() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

        let live_dir = sessions_root.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let path =
            live_dir.join("rollout-2026-06-25T00-00-00-019efd6c-8f60-7f42-8da1-3977122aa6be.jsonl");
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        let t1 = (now + chrono::Duration::milliseconds(100)).to_rfc3339();
        let t2 = (now + chrono::Duration::milliseconds(200)).to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{t0}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"019efd6c-8f60-7f42-8da1-3977122aa6be\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\"}}}}\n\
                 {{\"timestamp\":\"{t1}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n\
                 {{\"timestamp\":\"{t2}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"keep tracking this live codex context\"}}}}\n",
            ),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let claude_registry = tempfile::tempdir().unwrap();
        let refreshed = refresh_live_context(&store, claude_registry.path());
        let state = recompute_radar_state(&store, claude_registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            refreshed > 0,
            "explicit live refresh should ingest the live Codex rollout before assembly"
        );
        let codex = state
            .agents
            .iter()
            .find(|a| a.harness == "codex")
            .expect("live Codex rollout should render on first recompute");
        assert_eq!(codex.harness, "codex");
        assert_eq!(codex.status, "working");
        assert!(
            codex
                .recent_activity
                .iter()
                .any(|a| a.label.contains("keep tracking this live codex context")),
            "freshly ingested Codex activity should drive the live log: {:?}",
            codex.recent_activity
        );
    }

    /// Regression: a Claude Code session that was already running before WARDEN
    /// started must have its transcript tail pulled by the explicit startup/cold-read
    /// refresh before the live forest is assembled. The liveness registry alone can
    /// say the PID/session is open, but without a fresh store row RADAR has no
    /// context/logs to render and the globe is absent or stale until a later
    /// watcher/backfill catches up.
    #[test]
    fn explicit_refresh_ingests_live_claude_transcript_before_assembling() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_claude_projects = std::env::var_os("WARDEN_CLAUDE_PROJECTS");
        let old_codex_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_codex_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let claude_projects = tempfile::tempdir().unwrap();
        let codex_sessions = tempfile::tempdir().unwrap();
        let codex_archived = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CLAUDE_PROJECTS", claude_projects.path());
        std::env::set_var("WARDEN_CODEX_SESSIONS", codex_sessions.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", codex_archived.path());

        let session_id = "live-claude-session";
        let project_dir = claude_projects.path().join("-tmp-LiveClaude");
        std::fs::create_dir_all(&project_dir).unwrap();
        let transcript = project_dir.join(format!("{session_id}.jsonl"));
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        std::fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{session_id}\",\"timestamp\":\"{t0}\",\"cwd\":\"/tmp/LiveClaude\",\"message\":{{\"role\":\"user\",\"content\":\"track this live claude context before startup backfill\"}}}}\n",
            ),
        )
        .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        std::fs::write(
            registry.path().join(format!("{pid}.json")),
            serde_json::json!({
                "pid": pid,
                "sessionId": session_id,
                "cwd": "/tmp/LiveClaude",
                "entrypoint": "claude-desktop"
            })
            .to_string(),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let refreshed = refresh_live_context(&store, registry.path());
        let state = recompute_radar_state(&store, registry.path());

        match old_claude_projects {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_PROJECTS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_PROJECTS"),
        }
        match old_codex_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_codex_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            refreshed > 0,
            "explicit live refresh should ingest the live Claude transcript before assembly"
        );
        let claude = state
            .agents
            .iter()
            .find(|a| a.harness == "claude_code")
            .expect("live Claude transcript should render on first recompute");
        assert_eq!(claude.status, "working");
        assert_eq!(claude.cwd.as_deref(), Some("LiveClaude"));
        assert!(
            claude
                .recent_activity
                .iter()
                .any(|a| a.label.contains("track this live claude context")),
            "freshly ingested Claude activity should drive the live log: {:?}",
            claude.recent_activity
        );
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

    #[test]
    fn recent_activity_orders_by_timestamp_not_storage_order() {
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
        let mk = |id: &str, ts: DateTime<Utc>, event: Event| {
            (
                turn.clone(),
                EventRecord {
                    id: id.into(),
                    turn_id: "t".into(),
                    session_id: "s".into(),
                    ts,
                    event,
                    raw_ref: RawRef {
                        source_path: PathBuf::from("/x.jsonl"),
                        offset: 0,
                        line: 1,
                    },
                },
            )
        };
        let events = vec![
            mk(
                "new",
                now,
                Event::AssistantText {
                    text: "newest final answer".into(),
                },
            ),
            mk(
                "old",
                now - chrono::Duration::seconds(10),
                Event::ToolCall {
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"old command"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
        ];

        let acts = recent_activity(&events);
        assert_eq!(
            acts.first().map(|a| a.label.as_str()),
            Some("newest final answer")
        );
    }

    /// Naming: a Claude ROOT agent is named by its originating task (its first
    /// non-meta user prompt), not merely the cwd basename — so several live sessions
    /// in the same repo are differentiated by what each is doing. The folder basename
    /// is still exposed (as `cwd`) for the secondary "folder · model" subtitle.
    #[test]
    fn claude_root_label_is_its_folder_with_cwd_exposed() {
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
        let a = state
            .agents
            .iter()
            .find(|a| a.id == "r")
            .expect("root present");
        assert_eq!(
            a.label, "WARDEN",
            "a Claude root is named by its project folder (B1), not its originating task"
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
        store.upsert_session_batch(&child, &[], &[], 0).unwrap();
        store.link_child_session("c-sid", "p-sid").unwrap();

        let reg = claude_registry(&[(100, "p-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let c = state
            .agents
            .iter()
            .find(|a| a.id == "c-sid")
            .expect("child present");
        assert_eq!(
            c.label, "subagent 1",
            "Claude subagent label is its per-parent ordinal (B1), not its description"
        );
        assert_eq!(
            c.role.as_deref(),
            Some("Explore"),
            "Claude subagent role is its agentType"
        );

        // Root is labeled by its project folder; its folder is still exposed as `cwd`.
        let p = state.agents.iter().find(|a| a.id == "p-sid").unwrap();
        assert_eq!(p.label, "MyRepo", "root label is its project folder (B1)");
        assert_eq!(
            p.cwd.as_deref(),
            Some("MyRepo"),
            "root still exposes its cwd"
        );
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
        assert!(
            cost < fresh_cost,
            "cache reads must be cheaper than fresh input"
        );

        // Unknown model stays nullable.
        assert_eq!(est_cost_usd(&Some("mystery".into()), &cache_only), None);
    }

    /// A session with no `TokenUsage` reports zero occupancy and a `null` estimated
    /// composition (no turn-1 baseline) — honest, never fabricated.
    #[test]
    fn assemble_session_without_usage_is_zeroed_and_unestimated() {
        let store = Store::memory().unwrap();
        seed(&store, "s", "e", Harness::Codex, Some("/tmp/proj"), None);
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            Utc::now(),
        );
        let a = &state.agents[0];
        assert_eq!(a.context_tokens, 0);
        assert_eq!(a.fill_pct, 0.0);
        assert!(
            a.composition.estimated.is_none(),
            "no baseline → null estimate"
        );
        assert_eq!(a.est_cost_usd, None, "no model → no cost");
    }

    /// THE FIX (spec §3/§5: the forest is the OPEN set). A backfilled Claude session
    /// whose external id is NOT in the live registry is EXCLUDED — the archive of
    /// every transcript ever ingested must not render. A Claude session that IS in the
    /// registry is included; its status now comes from CONVERSATION STATE (Fault B):
    /// `seed` leaves a fresh, unanswered `UserPrompt` as the last event, so the honest
    /// verdict is `working` (the operator just asked) — not the old mtime "idle".
    #[test]
    fn claude_forest_includes_only_registry_open_sessions() {
        let store = Store::memory().unwrap();
        // Historical/backfill session: ingested long ago, no live registry entry.
        seed(
            &store,
            "hist",
            "hist-ext",
            Harness::ClaudeCode,
            Some("/tmp/old"),
            None,
        );
        // Currently-open session: a live `<pid>.json` references its session id.
        seed(
            &store,
            "live",
            "live-ext",
            Harness::ClaudeCode,
            Some("/tmp/now"),
            None,
        );

        let reg = claude_registry(&[(100, "live-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        assert_eq!(
            state.agents.len(),
            1,
            "only the registry-open session is in the forest"
        );
        let a = &state.agents[0];
        assert_eq!(
            a.id, "live",
            "the open session is the live one, not the backfill"
        );
        assert_eq!(
            a.status, "working",
            "last event is a fresh unanswered UserPrompt → working (Fault B: conversation-state, not mtime)"
        );
        assert!(
            !state.agents.iter().any(|a| a.id == "hist"),
            "the historical/backfill session must be excluded"
        );
    }

    /// FAULT B end-to-end via `assemble`: a registry-open Claude session's working/idle
    /// verdict comes from its LAST ingested event, and is DETERMINISTIC across reads
    /// (the property that kills the flicker). A session whose last event is a completed
    /// `TokenUsage` turn is idle; a session whose last event is an unanswered
    /// `UserPrompt` is working; two assembles on the unchanged store at the same instant
    /// return byte-identical statuses. (The OLD mtime path, keyed on FSEvents-coalesced
    /// file writes, could flip these between reads — this test pins the fix.)
    #[test]
    fn assemble_status_from_conversation_state_is_deterministic() {
        let store = Store::memory().unwrap();
        // `seed` writes a UserPrompt then optional bookkeeping TokenUsage. Add a real
        // trailing AssistantText for idle-sess so the semantic tail is a completed turn.
        seed(
            &store,
            "idle-sess",
            "idle-ext",
            Harness::ClaudeCode,
            Some("/tmp/a"),
            Some((2, 100, 1000, 50, "claude-opus-4-8")),
        );
        let done_ts = Utc::now() + chrono::Duration::milliseconds(10);
        let done_session = Session {
            id: "idle-sess".into(),
            harness: Harness::ClaudeCode,
            external_id: "idle-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/a"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: done_ts,
            ended_at: None,
            source_path: PathBuf::from("/tmp/idle-sess.jsonl"),
            raw_hash: 1,
            ingested_at: done_ts,
            meta: serde_json::json!({}),
        };
        let done_turn = Turn {
            id: "idle-sess-done-turn".into(),
            session_id: "idle-sess".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 99,
            started_at: done_ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let done_event = EventRecord {
            id: "idle-sess-done-text".into(),
            turn_id: done_turn.id.clone(),
            session_id: "idle-sess".into(),
            ts: done_ts,
            event: Event::AssistantText {
                text: "done".into(),
            },
            raw_ref: RawRef {
                source_path: done_session.source_path.clone(),
                offset: 2,
                line: 3,
            },
        };
        store
            .upsert_session_batch(&done_session, &[done_turn], &[done_event], 0)
            .unwrap();
        // working-sess: last event is an unanswered UserPrompt (a strong working signal).
        seed(
            &store,
            "working-sess",
            "working-ext",
            Harness::ClaudeCode,
            Some("/tmp/b"),
            None,
        );

        // Both are registry-open WITHOUT an authoritative `status` field, so the
        // conversation-state fallback decides. Evaluate 60s in the FUTURE relative to the
        // seeded events: the completed AssistantText is idle while the unanswered
        // UserPrompt is still within the 180s stale backstop and remains working. A
        // FIXED clock makes the verdict exact and deterministic.
        let reg = claude_registry(&[(101, "idle-ext"), (102, "working-ext")]);
        let now = Utc::now() + chrono::Duration::seconds(60);

        let st = |state: &RadarState, id: &str| {
            state
                .agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.status.clone())
                .unwrap_or_default()
        };

        let s1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        assert_eq!(
            st(&s1, "idle-sess"),
            "idle",
            "last real action is a completed AssistantText turn → idle"
        );
        assert_eq!(
            st(&s1, "working-sess"),
            "working",
            "last event is an unanswered UserPrompt → working"
        );

        // Determinism: a second assemble on the UNCHANGED store at the SAME instant
        // yields identical statuses (no mtime, no flicker).
        let s2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        assert_eq!(
            st(&s2, "idle-sess"),
            st(&s1, "idle-sess"),
            "idle status stable across reads"
        );
        assert_eq!(
            st(&s2, "working-sess"),
            st(&s1, "working-sess"),
            "working status stable across reads"
        );
    }

    #[test]
    fn codex_stale_uningested_tail_does_not_stay_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f1111-1111-7111-8111-111111111111.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n\
             {\"timestamp\":\"2026-06-25T00:00:20Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\",\"phase\":\"final_answer\"}}\n",
        )
        .unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-stale-tail".into(),
            harness: Harness::Codex,
            external_id: "019f1111-1111-7111-8111-111111111111".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/StaleTail"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-stale-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-stale-tail-user".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::UserPrompt {
                text: "go".into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], 10)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(240),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-stale-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "idle",
            "a stale store row whose source file grew past its watermark must settle after the semantic backstop"
        );
    }

    #[test]
    fn codex_inflight_file_write_stays_working_with_uningested_tail() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f2222-2222-7222-8222-222222222222.jsonl");
        let complete_tool_call = "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"apply_patch src/app.ts\\\"}\",\"call_id\":\"call_write\"}}\n";
        let partial_tool_result =
            "{\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\"";
        std::fs::write(&path, format!("{complete_tool_call}{partial_tool_result}")).unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-write-tail".into(),
            harness: Harness::Codex,
            external_id: "019f2222-2222-7222-8222-222222222222".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WritingFiles"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-write-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-write-tail-tool-call".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::ToolCall {
                tool: "exec_command".into(),
                input: serde_json::json!({ "cmd": "apply_patch src/app.ts" }),
                call_id: "call_write".into(),
                kind: ToolKind::Unknown,
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], complete_tool_call.len() as u64)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(60),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-write-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "an in-flight Codex file-write ToolCall must stay working while its result line is still incomplete"
        );
    }

    #[test]
    fn codex_incomplete_patch_tail_after_assistant_stays_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f3333-3333-7333-8333-333333333333.jsonl");
        let assistant_line = "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"I will update the files now.\"}}\n";
        let partial_patch =
            "{\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\"";
        std::fs::write(&path, format!("{assistant_line}{partial_patch}")).unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-patch-tail".into(),
            harness: Harness::Codex,
            external_id: "019f3333-3333-7333-8333-333333333333".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/PatchTail"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-patch-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-patch-tail-assistant".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::AssistantText {
                text: "I will update the files now.".into(),
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], assistant_line.len() as u64)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(60),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-patch-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "a partial Codex patch record means file writing is in progress even when the last complete event was assistant text"
        );
    }

    #[test]
    fn codex_patch_snapshot_after_assistant_stays_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f4444-4444-7444-8444-444444444444.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"I will update the files now.\"}}\n\
             {\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"changes\":{\"/tmp/PatchDone/src/app.ts\":{\"type\":\"update\"}}}}\n",
        )
        .unwrap();

        let base = Utc::now();
        let patch_ts = base + chrono::Duration::seconds(40);
        let session = Session {
            id: "codex-patch-done".into(),
            harness: Harness::Codex,
            external_id: "019f4444-4444-7444-8444-444444444444".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/PatchDone"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-patch-done-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let assistant = EventRecord {
            id: "codex-patch-done-assistant".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::AssistantText {
                text: "I will update the files now.".into(),
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        let files = EventRecord {
            id: "codex-patch-done-files".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: patch_ts,
            event: Event::FileSnapshot {
                files: vec![FileEdit {
                    path: "/tmp/PatchDone/src/app.ts".into(),
                    ..Default::default()
                }],
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 140,
                line: 2,
            },
        };
        let watermark = std::fs::metadata(&path).unwrap().len();
        store
            .upsert_session_batch(&session, &[turn], &[assistant, files], watermark)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            patch_ts + chrono::Duration::seconds(20),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-patch-done")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "a fresh Codex FileSnapshot is a real file-write action, not idle bookkeeping"
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
        seed(
            &store,
            "open-cx",
            "open-uuid",
            Harness::Codex,
            Some("/tmp/p1"),
            None,
        );
        seed(
            &store,
            "done-cx",
            "done-uuid",
            Harness::Codex,
            Some("/tmp/p2"),
            None,
        );

        // Only `open-uuid` currently lives under sessions/ (done-uuid was archived).
        let is_codex_open = |s: &Session| s.external_id == "open-uuid";
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            Utc::now(),
        );

        assert_eq!(
            state.agents.len(),
            1,
            "only the non-archived Codex session is open"
        );
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
        seed(
            &store,
            "op-root",
            "op-root-ext",
            Harness::ClaudeCode,
            Some("/tmp/a"),
            None,
        );
        seed(
            &store,
            "op-sub",
            "op-sub-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("op-sub", "op-root").unwrap();
        // Closed tree: root `cl-root` (NOT in registry) + subagent `cl-sub`.
        seed(
            &store,
            "cl-root",
            "cl-root-ext",
            Harness::ClaudeCode,
            Some("/tmp/b"),
            None,
        );
        seed(
            &store,
            "cl-sub",
            "cl-sub-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("cl-sub", "cl-root").unwrap();

        // Only the open root is registered alive.
        let reg = claude_registry(&[(100, "op-root-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        // The open tree survives, nested and counted.
        let root = state
            .agents
            .iter()
            .find(|a| a.id == "op-root")
            .expect("open root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(
            root.child_count, 1,
            "open root counts its one open subagent"
        );
        let sub = state
            .agents
            .iter()
            .find(|a| a.id == "op-sub")
            .expect("open subagent present");
        assert_eq!(sub.depth, 1, "subagent rides on its open root, nested");
        assert_eq!(sub.parent_id.as_deref(), Some("op-root"));

        // The closed tree is gone entirely (root AND its orphaned subagent).
        assert!(
            !state.agents.iter().any(|a| a.id == "cl-root"),
            "closed root excluded"
        );
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

    /// Build a `(Turn, EventRecord)` carrying a single `ToolResult` for `call_id` at
    /// timestamp `ts` — the parent-side termination fact `subagent_terminated_at` reads.
    fn mk_tool_result_event(call_id: &str, ts: DateTime<Utc>) -> (Turn, EventRecord) {
        mk_tool_result_event_with_summary(call_id, ts, None)
    }

    fn mk_tool_result_event_with_summary(
        call_id: &str,
        ts: DateTime<Utc>,
        summary: Option<&str>,
    ) -> (Turn, EventRecord) {
        let turn = Turn {
            id: "p-t".into(),
            session_id: "parent".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: format!("res-{call_id}"),
            turn_id: "p-t".into(),
            session_id: "parent".into(),
            ts,
            event: Event::ToolResult {
                call_id: call_id.into(),
                status: ToolStatus::Ok,
                bytes: 0,
                summary: summary.map(str::to_string),
            },
            raw_ref: RawRef {
                source_path: PathBuf::from("/tmp/parent.jsonl"),
                offset: 0,
                line: 1,
            },
        };
        (turn, rec)
    }

    fn mk_user_prompt_event(text: &str, ts: DateTime<Utc>) -> (Turn, EventRecord) {
        let turn = Turn {
            id: "p-u".into(),
            session_id: "parent".into(),
            parent_id: None,
            role: Role::User,
            index: 2,
            started_at: ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: "prompt".into(),
            turn_id: "p-u".into(),
            session_id: "parent".into(),
            ts,
            event: Event::UserPrompt {
                text: text.into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: PathBuf::from("/tmp/parent.jsonl"),
                offset: 0,
                line: 1,
            },
        };
        (turn, rec)
    }

    // ── B4: pure termination decision ────────────────────────────────────────────
    #[test]
    fn subagent_terminated_at_uses_result_then_timeout() {
        let now = Utc::now();
        let result_ts = now - chrono::Duration::seconds(2);
        let parent_events = vec![mk_tool_result_event("toolu_5", result_ts)];

        // Primary: a matching tool-result → terminated at the result's ts.
        assert_eq!(
            subagent_terminated_at(Some("toolu_5"), &parent_events, Some(now), now, 90_000),
            Some(result_ts)
        );
        // No result, recently active → still live.
        assert_eq!(
            subagent_terminated_at(
                Some("toolu_x"),
                &[],
                Some(now - chrono::Duration::seconds(3)),
                now,
                90_000
            ),
            None
        );
        // No result, silent past the backstop → terminated at (last + timeout).
        let last = now - chrono::Duration::seconds(200);
        assert_eq!(
            subagent_terminated_at(Some("toolu_x"), &[], Some(last), now, 90_000),
            Some(last + chrono::Duration::milliseconds(90_000))
        );
        // No tool_use_id and no last activity → never terminated.
        assert_eq!(subagent_terminated_at(None, &[], None, now, 90_000), None);
    }

    #[test]
    fn subagent_terminated_at_ignores_async_launch_ack() {
        let now = Utc::now();
        let parent_events = vec![mk_tool_result_event_with_summary(
            "toolu_async",
            now - chrono::Duration::seconds(1),
            Some("Async agent launched successfully.\nThe agent is working in the background."),
        )];

        assert_eq!(
            subagent_terminated_at(Some("toolu_async"), &parent_events, Some(now), now, 90_000),
            None,
            "the launch acknowledgment starts a background subagent; it is not the completion signal"
        );
    }

    #[test]
    fn subagent_terminated_at_uses_async_task_completion_notification() {
        let now = Utc::now();
        let completed_ts = now - chrono::Duration::seconds(1);
        let text = "<task-notification>\n\
<task-id>a04f87f14f439d3f3</task-id>\n\
<tool-use-id>toolu_done</tool-use-id>\n\
<status>completed</status>\n\
<summary>Agent came to rest</summary>\n\
</task-notification>";
        let parent_events = vec![mk_user_prompt_event(text, completed_ts)];

        assert_eq!(
            subagent_terminated_at(Some("toolu_done"), &parent_events, Some(now), now, 90_000),
            Some(completed_ts),
            "Claude async subagents finish via the parent task-notification completion record"
        );
    }

    // ── B1: folder/subagent naming ───────────────────────────────────────────────
    #[test]
    fn display_label_names_root_by_folder_and_subagent_by_ordinal() {
        // root with a folder → the folder name
        assert_eq!(
            display_label(0, Some("WARDEN"), None, None, "fallback"),
            "WARDEN"
        );
        // a second live root in the same folder → circled disambiguator (oldest keeps bare name)
        assert_eq!(
            display_label(0, Some("WARDEN"), None, Some(2), "fallback"),
            "WARDEN ②"
        );
        assert_eq!(
            display_label(0, Some("WARDEN"), None, Some(1), "fallback"),
            "WARDEN"
        );
        // root with no folder → falls back to the identity label
        assert_eq!(
            display_label(0, None, None, None, "diagnose the bug"),
            "diagnose the bug"
        );
        // subagent → strictly "subagent N", regardless of any role/description
        assert_eq!(
            display_label(1, Some("WARDEN"), Some(1), None, "Explore"),
            "subagent 1"
        );
        assert_eq!(display_label(2, None, Some(3), None, "x"), "subagent 3");
    }

    /// Seed: a live Claude ROOT that logged a `ToolResult` for `call_id` (the
    /// subagent's completion signal) + a Claude SUBAGENT under `/subagents/` carrying
    /// `meta.toolUseId == call_id`, linked to the root. The root's tool-result is
    /// timestamped at `result_ts` (a fixed point so the test can advance `now` around
    /// the grace window). Returns nothing; the root's external id is `{root}-ext`.
    fn seed_root_with_terminated_subagent(
        store: &Store,
        root: &str,
        sub: &str,
        call_id: &str,
        result_ts: DateTime<Utc>,
    ) {
        // Root session with a ToolResult event for `call_id`.
        let root_session = Session {
            id: root.into(),
            harness: Harness::ClaudeCode,
            external_id: format!("{root}-ext"),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/Users/k/Developer/MyRepo"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: result_ts - chrono::Duration::seconds(10),
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{root}.jsonl")),
            raw_hash: 0,
            ingested_at: result_ts,
            meta: serde_json::json!({}),
        };
        let root_turn = Turn {
            id: format!("{root}-t0"),
            session_id: root.into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: result_ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let root_result = EventRecord {
            id: format!("{root}-res"),
            turn_id: format!("{root}-t0"),
            session_id: root.into(),
            ts: result_ts,
            event: Event::ToolResult {
                call_id: call_id.into(),
                status: ToolStatus::Ok,
                bytes: 0,
                summary: None,
            },
            raw_ref: RawRef {
                source_path: root_session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&root_session, &[root_turn], &[root_result], 0)
            .unwrap();

        // Subagent session under /subagents/ with meta.toolUseId == call_id.
        let sub_session = Session {
            id: sub.into(),
            harness: Harness::ClaudeCode,
            external_id: format!("{sub}-ext"),
            project: None,
            model_ids: vec![],
            started_at: result_ts - chrono::Duration::seconds(5),
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/proj/sess/subagents/agent-{sub}.jsonl")),
            raw_hash: 0,
            ingested_at: result_ts,
            meta: serde_json::json!({}),
        };
        store
            .upsert_session_batch(&sub_session, &[], &[], 0)
            .unwrap();
        store
            .merge_session_meta(sub, &serde_json::json!({ "toolUseId": call_id }))
            .unwrap();
        store.link_child_session(sub, root).unwrap();
    }

    /// Read back the timestamp of the root's `ToolResult` for `call_id` (the fixed t0
    /// the termination decision keys on).
    fn result_ts_of(store: &Store, root: &str, call_id: &str) -> DateTime<Utc> {
        store
            .session_events(root)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match &e.event {
                Event::ToolResult { call_id: c, .. } if c == call_id => Some(e.ts),
                _ => None,
            })
            .expect("root must carry the tool-result")
    }

    /// B4 end-to-end: a subagent whose parent logged its tool-result is emitted ONCE
    /// as `terminated` (within the grace window so the FACE can implode it), then
    /// DROPPED from the forest past the grace window, and stays dropped on every later
    /// recompute (a permanent fact ⇒ no resurrection).
    #[test]
    fn terminated_subagent_is_emitted_once_then_dropped_and_never_resurrects() {
        let store = Store::memory().unwrap();
        let t0 = Utc::now() - chrono::Duration::seconds(120); // a fixed past instant
        seed_root_with_terminated_subagent(&store, "root", "sub", "toolu_1", t0);
        let reg = claude_registry(&[(4242, "root-ext")]); // root is registry-open
        let t0 = result_ts_of(&store, "root", "toolu_1");

        // Within the 5s grace window → present as "terminated".
        let s1 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(1),
        );
        let sub = s1
            .agents
            .iter()
            .find(|a| a.id == "sub")
            .expect("present within grace");
        assert_eq!(sub.status, "terminated");

        // Past the grace window → dropped from the forest.
        let s2 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(30),
        );
        assert!(
            s2.agents.iter().all(|a| a.id != "sub"),
            "dropped past grace"
        );

        // Stays dropped (no resurrection) on a still-later recompute.
        let s3 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(60),
        );
        assert!(
            s3.agents.iter().all(|a| a.id != "sub"),
            "stays dropped (no resurrection)"
        );

        // The root itself is never terminated — it remains in the forest.
        assert!(s2.agents.iter().any(|a| a.id == "root"), "root persists");
    }
}
