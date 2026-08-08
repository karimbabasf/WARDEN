//! Pure subagent→parent linkage resolvers for the RADAR forest.
//!
//! Both resolvers are deterministic, side-effect-free, and unit-tested without a
//! store: the caller (the ingest path) persists the returned `(child, parent)`
//! pairs via `Store::link_child_session`.

use crate::ingest::claude_code::{
    is_subagent_session_path, subagent_agent_id, subagent_root_external_id, SubagentMeta,
};
use crate::ingest::SessionBatch;
use crate::ir::{Event, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Link Claude subagents to their parents (Task 3).
///
/// Three resolvers, strongest first. The order is what makes the forest as deep as
/// the real agent tree instead of two levels flat.
///
/// 1. **`meta.parentAgentId`** names the spawning SUBAGENT outright. This is the only
///    signal that can express nesting, because the transcript path cannot: Claude
///    writes every subagent of a session into one flat `<root>/subagents/` directory
///    whether it was spawned by the root or by another subagent five levels down.
/// 2. **`meta.toolUseId`** equals the `id` of the `tool_use` block that dispatched it
///    (Claude spawns via the `Agent`/`Task` tool). Parent tool-calls are indexed from
///    EVERY batch, subagent batches included, so a subagent that dispatches its own
///    subagent is a parent here like any other.
/// 3. **The root of its transcript directory**, the last-resort fallback, and only for
///    an agent that does not claim to be nested. A sidecar reporting `spawnDepth >= 2`
///    was demonstrably spawned by a subagent, so hanging it off the root would not be
///    a degraded answer but a WRONG tree; it renders as an unparented agent instead,
///    which is the same thing we already do for a child whose parent is not ingested.
///
/// Returns one pair per meta that resolves to both a known parent and a known child
/// transcript; unmatched metas are silently skipped. Order follows the input `metas`.
pub fn link_claude_subagents(
    batches: &[SessionBatch],
    metas: &[SubagentMeta],
) -> Vec<(String, String)> {
    // call_id → parent session id (the session that issued the Agent/Task call).
    let mut call_to_parent: HashMap<&str, &str> = HashMap::new();
    // agent_id (from the subagent transcript filename) → child session id.
    let mut agent_to_child: HashMap<String, &str> = HashMap::new();
    // root external session id → parent session id, used by Claude workflow subagents
    // whose sidecar has no toolUseId.
    let mut root_external_to_parent: HashMap<&str, &str> = HashMap::new();

    for b in batches {
        let is_subagent = is_subagent_session_path(&b.session.source_path);
        if is_subagent {
            agent_to_child.insert(subagent_agent_id(&b.session.source_path), b.session.id.as_str());
        } else {
            root_external_to_parent.insert(b.session.external_id.as_str(), b.session.id.as_str());
        }
        for e in &b.events {
            if let Event::ToolCall { tool, call_id, .. } = &e.event {
                if tool == "Agent" || tool == "Task" {
                    call_to_parent.insert(call_id.as_str(), b.session.id.as_str());
                }
            }
        }
    }

    let mut pairs = Vec::new();
    for m in metas {
        let Some(child) = agent_to_child.get(&m.agent_id) else {
            continue;
        };
        let parent = agent_to_child
            .get(m.parent_agent_id.as_str())
            .copied()
            .filter(|p| p != child)
            .or_else(|| call_to_parent.get(m.tool_use_id.as_str()).copied())
            .or_else(|| {
                if claims_nested(m) {
                    return None;
                }
                batches
                    .iter()
                    .find(|b| b.session.id == **child)
                    .and_then(|b| subagent_root_external_id(&b.session.source_path))
                    .and_then(|root| root_external_to_parent.get(root.as_str()).copied())
            });
        if let Some(parent) = parent {
            pairs.push((child.to_string(), parent.to_string()));
        }
    }
    pairs
}

/// Does this sidecar claim to have been spawned by another SUBAGENT rather than by a
/// root session? Either it names the spawner, or it reports a depth that only a
/// nested spawn reaches. Such an agent must never fall back onto the root: the root
/// is its ancestor, not its parent, and a fabricated edge is worse than no edge.
pub(crate) fn claims_nested(m: &SubagentMeta) -> bool {
    !m.parent_agent_id.is_empty() || m.spawn_depth.is_some_and(|d| d >= 2)
}

/// Link Codex Desktop subagents to their parents (Task 5).
///
/// Codex Desktop subagent rollouts carry `thread_source == "subagent"` and a
/// `parent_thread_id` in their `session_meta` (preserved into `Session.meta` by
/// Task 4). The parent is the session whose `external_id == parent_thread_id`.
/// Returns `(child_external_id, parent_external_id)` pairs — the caller maps these
/// to session ids for persistence.
///
/// Honest viz: a child is paired ONLY when a matching parent `external_id` exists
/// among the inputs. A VS Code Codex session (`originator == "codex_vscode"`, no
/// `parent_thread_id`) is never `thread_source == "subagent"`, so it yields no
/// pair and stays a flat solo globe — children are never fabricated.
pub fn link_codex_subagents(sessions: &[Session]) -> Vec<(String, String)> {
    let known_externals: HashSet<&str> =
        sessions.iter().map(|s| s.external_id.as_str()).collect();
    let mut pairs = Vec::new();
    for s in sessions {
        let is_subagent = s.meta.get("thread_source").and_then(Value::as_str) == Some("subagent");
        if !is_subagent {
            continue;
        }
        let Some(parent) = s.meta.get("parent_thread_id").and_then(Value::as_str) else {
            continue;
        };
        if known_externals.contains(parent) {
            pairs.push((s.external_id.clone(), parent.to_string()));
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use chrono::Utc;
    use std::path::PathBuf;

    /// Build a minimal `SessionBatch` for `sid` with the given source path and a
    /// single optional `Agent` tool-call carrying `call_id`.
    fn batch(sid: &str, source: PathBuf, agent_call_id: Option<&str>) -> SessionBatch {
        let now = Utc::now();
        let tid = format!("{sid}-t0");
        let mut events = Vec::new();
        if let Some(cid) = agent_call_id {
            events.push(EventRecord {
                id: format!("{sid}-call"),
                turn_id: tid.clone(),
                session_id: sid.into(),
                ts: now,
                event: Event::ToolCall {
                    tool: "Agent".into(),
                    input: serde_json::Value::Null,
                    call_id: cid.into(),
                    kind: ToolKind::SubagentTask,
                },
                raw_ref: RawRef {
                    source_path: source.clone(),
                    offset: 0,
                    line: 1,
                },
            });
        }
        SessionBatch {
            session: Session {
                id: sid.into(),
                harness: Harness::ClaudeCode,
                external_id: sid.into(),
                project: None,
                model_ids: vec![],
                started_at: now,
                ended_at: None,
                source_path: source,
                raw_hash: 0,
                ingested_at: now,
                meta: serde_json::json!({}),
            },
            turns: vec![Turn {
                id: tid,
                session_id: sid.into(),
                parent_id: None,
                role: Role::Assistant,
                index: 1,
                started_at: now,
                duration_ms: None,
                is_sidechain: false,
            }],
            events,
            offset: 0,
        }
    }

    /// A parent batch issuing `Agent`(call_id=toolu_01) and a child batch whose
    /// transcript is `…/subagents/agent-abc.jsonl` link via the meta whose
    /// `tool_use_id == toolu_01` and `agent_id == abc`.
    #[test]
    fn links_child_to_parent_by_tool_use_id() {
        let parent = batch(
            "parent-sid",
            PathBuf::from("/proj/session-1/session-1.jsonl"),
            Some("toolu_01"),
        );
        let child = batch(
            "child-sid",
            PathBuf::from("/proj/session-1/subagents/agent-abc.jsonl"),
            None,
        );
        let meta = meta("abc", "toolu_01");

        let pairs = link_claude_subagents(&[parent, child], &[meta]);
        assert_eq!(
            pairs,
            vec![("child-sid".to_string(), "parent-sid".to_string())]
        );
    }

    /// A meta whose `tool_use_id` matches no parent tool-call yields no pair (the
    /// child renders as a root, never fabricated).
    #[test]
    fn unmatched_meta_yields_no_pair() {
        let child = batch(
            "child-sid",
            PathBuf::from("/proj/session-1/subagents/agent-abc.jsonl"),
            None,
        );
        let meta = meta("abc", "toolu_missing");
        assert!(link_claude_subagents(&[child], &[meta]).is_empty());
    }

    /// A sub-subagent: its sidecar names the SUBAGENT that spawned it, and that is
    /// the only place the nesting is recorded, because both transcripts sit side by
    /// side in the same flat `<root>/subagents/` directory.
    #[test]
    fn parent_agent_id_links_a_subagent_under_another_subagent() {
        let root = batch(
            "root-sid",
            PathBuf::from("/proj/session-1/session-1.jsonl"),
            Some("toolu_01"),
        );
        let mid = batch(
            "mid-sid",
            PathBuf::from("/proj/session-1/subagents/agent-mid.jsonl"),
            None,
        );
        let deep = batch(
            "deep-sid",
            PathBuf::from("/proj/session-1/subagents/agent-deep.jsonl"),
            None,
        );
        let mut deep_meta = meta("deep", "toolu_99");
        deep_meta.parent_agent_id = "mid".into();
        deep_meta.spawn_depth = Some(2);

        let pairs = link_claude_subagents(&[root, mid, deep], &[meta("mid", "toolu_01"), deep_meta]);
        assert_eq!(
            pairs,
            vec![
                ("mid-sid".to_string(), "root-sid".to_string()),
                ("deep-sid".to_string(), "mid-sid".to_string()),
            ],
            "the deep agent must hang off the subagent that spawned it, not off the root"
        );
    }

    /// The same nesting, expressed the other way: the spawning SUBAGENT logged the
    /// `Task` call itself, so the tool-use id resolves to it rather than to a root.
    #[test]
    fn tool_use_id_issued_by_a_subagent_links_under_that_subagent() {
        let root = batch(
            "root-sid",
            PathBuf::from("/proj/session-1/session-1.jsonl"),
            None,
        );
        let mid = batch(
            "mid-sid",
            PathBuf::from("/proj/session-1/subagents/agent-mid.jsonl"),
            Some("toolu_deep"),
        );
        let deep = batch(
            "deep-sid",
            PathBuf::from("/proj/session-1/subagents/agent-deep.jsonl"),
            None,
        );
        let pairs = link_claude_subagents(&[root, mid, deep], &[meta("deep", "toolu_deep")]);
        assert_eq!(
            pairs,
            vec![("deep-sid".to_string(), "mid-sid".to_string())]
        );
    }

    /// A sidecar that CLAIMS nesting but whose parent is not ingested renders
    /// unparented. Falling back to the root would not be a coarser answer, it would
    /// be a wrong one: the root is its ancestor, never its parent.
    #[test]
    fn a_nested_claim_never_falls_back_onto_the_root() {
        // The root is named so the fallback WOULD resolve: only the nesting claim
        // stops it, which is the whole point of the case.
        let root = batch(
            "session-1",
            PathBuf::from("/proj/session-1/session-1.jsonl"),
            None,
        );
        let deep = batch(
            "deep-sid",
            PathBuf::from("/proj/session-1/subagents/agent-deep.jsonl"),
            None,
        );
        let mut deep_meta = meta("deep", "");
        deep_meta.spawn_depth = Some(2);
        assert!(link_claude_subagents(&[root, deep], &[deep_meta]).is_empty());
    }

    /// A first-level subagent with no tool-use id at all (the common shape: 411 of
    /// the 555 sidecars on the dev machine carry none) still lands on its root.
    #[test]
    fn a_first_level_subagent_without_a_tool_use_id_still_lands_on_its_root() {
        let root = batch(
            "session-1",
            PathBuf::from("/proj/session-1/session-1.jsonl"),
            None,
        );
        let child = batch(
            "child-sid",
            PathBuf::from("/proj/session-1/subagents/agent-abc.jsonl"),
            None,
        );
        let mut m = meta("abc", "");
        m.spawn_depth = Some(1);
        let pairs = link_claude_subagents(&[root, child], &[m]);
        assert_eq!(
            pairs,
            vec![("child-sid".to_string(), "session-1".to_string())]
        );
    }

    /// A sidecar carrying just the two ids the linkage reads; the caller sets
    /// `parent_agent_id` / `spawn_depth` on top when the case is about nesting.
    fn meta(agent_id: &str, tool_use_id: &str) -> SubagentMeta {
        SubagentMeta {
            agent_type: "Explore".into(),
            description: "map frontend".into(),
            tool_use_id: tool_use_id.into(),
            agent_id: agent_id.into(),
            parent_agent_id: String::new(),
            spawn_depth: None,
            team_name: String::new(),
            member_name: String::new(),
            task_kind: String::new(),
        }
    }

    /// Build a minimal Codex `Session` with the given external id and `meta` JSON.
    fn codex_session(external_id: &str, meta: serde_json::Value) -> Session {
        let now = Utc::now();
        Session {
            id: format!("sid-{external_id}"),
            harness: Harness::Codex,
            external_id: external_id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/codex/rollout-{external_id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta,
        }
    }

    /// A Codex Desktop subagent (`thread_source:"subagent"`, `parent_thread_id:"P"`)
    /// pairs with the parent whose `external_id == "P"`.
    #[test]
    fn codex_subagent_links_to_parent_thread() {
        let parent = codex_session("P", serde_json::json!({ "thread_source": "user" }));
        let child = codex_session(
            "C",
            serde_json::json!({ "thread_source": "subagent", "parent_thread_id": "P", "originator": "Codex Desktop" }),
        );
        let pairs = link_codex_subagents(&[parent, child]);
        assert_eq!(pairs, vec![("C".to_string(), "P".to_string())]);
    }

    /// A VS Code Codex session (`codex_vscode` originator, no `parent_thread_id`)
    /// is flat: it is not a subagent, so it yields no pair (no fabricated child).
    #[test]
    fn codex_vscode_session_stays_flat() {
        let flat = codex_session(
            "V",
            serde_json::json!({ "originator": "codex_vscode" }),
        );
        assert!(link_codex_subagents(&[flat]).is_empty());
    }

    /// A subagent whose `parent_thread_id` is not among the inputs yields no pair
    /// (parent not currently open → child renders as a root, never fabricated).
    #[test]
    fn codex_subagent_with_unknown_parent_yields_no_pair() {
        let orphan = codex_session(
            "C",
            serde_json::json!({ "thread_source": "subagent", "parent_thread_id": "missing" }),
        );
        assert!(link_codex_subagents(&[orphan]).is_empty());
    }
}
