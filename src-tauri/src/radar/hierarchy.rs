//! Pure subagent→parent linkage resolvers for the RADAR forest.
//!
//! Both resolvers are deterministic, side-effect-free, and unit-tested without a
//! store: the caller (the ingest path) persists the returned `(child, parent)`
//! pairs via `Store::link_child_session`.

use crate::ingest::claude_code::{subagent_agent_id, SubagentMeta};
use crate::ingest::SessionBatch;
use crate::ir::{Event, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Link Claude subagents to their parents (Task 3).
///
/// The link is deterministic: a subagent's sidecar `meta.toolUseId` equals the
/// `id` of the parent assistant `tool_use` block (Claude dispatches subagents via
/// the `Agent`/`Task` tool). We:
/// 1. index every parent `ToolCall` `call_id` → that batch's session id;
/// 2. index every subagent batch's `agent_id` (derived from its
///    `subagents/agent-<id>.jsonl` path) → its session id;
/// 3. for each meta, map `meta.tool_use_id → parent_sid` and
///    `meta.agent_id → child_sid`, emitting the `(child_sid, parent_sid)` pair.
///
/// Returns one pair per meta that resolves to BOTH a known parent tool-call and a
/// known child transcript; unmatched metas are silently skipped (a child whose
/// parent is not currently ingested simply renders as a root). Order follows the
/// input `metas`.
pub fn link_claude_subagents(
    batches: &[SessionBatch],
    metas: &[SubagentMeta],
) -> Vec<(String, String)> {
    // call_id → parent session id (the session that issued the Agent/Task call).
    let mut call_to_parent: HashMap<&str, &str> = HashMap::new();
    // agent_id (from the subagent transcript filename) → child session id.
    let mut agent_to_child: HashMap<String, &str> = HashMap::new();

    for b in batches {
        let is_subagent = b
            .session
            .source_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "subagents")
            .unwrap_or(false);
        if is_subagent {
            agent_to_child.insert(subagent_agent_id(&b.session.source_path), b.session.id.as_str());
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
        if let (Some(parent), Some(child)) = (
            call_to_parent.get(m.tool_use_id.as_str()),
            agent_to_child.get(&m.agent_id),
        ) {
            pairs.push((child.to_string(), parent.to_string()));
        }
    }
    pairs
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
        let meta = SubagentMeta {
            agent_type: "Explore".into(),
            description: "map frontend".into(),
            tool_use_id: "toolu_01".into(),
            agent_id: "abc".into(),
        };

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
        let meta = SubagentMeta {
            agent_type: "Explore".into(),
            description: "x".into(),
            tool_use_id: "toolu_missing".into(),
            agent_id: "abc".into(),
        };
        assert!(link_claude_subagents(&[child], &[meta]).is_empty());
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
