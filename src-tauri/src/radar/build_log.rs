//! The Brickyard build log: one agent's stored events, folded into the ordered list of
//! build actions the brick view plays back.
//!
//! The view's one promise is that a placed brick is a real thing the agent did, so this
//! fold is the whole contract and it stays pure: the same events always produce the same
//! log, which is also what makes a past session replay identically.
//!
//! * `place`: a write-type call (Edit/Write, a shell run, a Codex patch). One brick.
//! * `pull`: that call came back as an error. The brick it placed comes off again.
//! * `browse`: reads, searches, lookups, MCP and skill calls. Pages flip, no brick.
//! * `page`: a plan or todo update. The builder turns to the next page.
//! * `spawn`: a subagent launch. A helper walks in.
//! * `ask`: a call that blocks on the operator.

use std::collections::HashMap;

use serde::Serialize;

use super::agent::{short_tool_name, tool_activity, tool_call_abs_path};
use crate::ir::{Event, EventRecord, ToolStatus};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildAction {
    Place,
    Pull,
    Browse,
    Page,
    Spawn,
    Ask,
}

/// One entry of the log. `seq` is the entry's index in the full log and is stable: the
/// log only ever grows at the end, so a client can ask for everything after a cursor.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BuildStep {
    pub seq: u32,
    pub ts: String,
    pub action: BuildAction,
    pub tool: String,
    pub label: String,
    pub target: Option<String>,
    /// On a `pull`, the `seq` of the `place` it undoes.
    pub undoes: Option<u32>,
}

/// Plan and todo tools: they move the builder to the next page rather than place a brick.
fn is_plan_tool(tool: &str) -> bool {
    matches!(
        short_tool_name(tool).as_str(),
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "update_plan"
    )
}

/// Fold one session's events (any order) into its build log.
pub fn build_log(events: &[EventRecord]) -> Vec<BuildStep> {
    let mut ordered: Vec<&EventRecord> = events.iter().collect();
    ordered.sort_by(|a, b| {
        a.ts.cmp(&b.ts)
            .then(a.raw_ref.offset.cmp(&b.raw_ref.offset))
            .then(a.id.cmp(&b.id))
    });

    let mut out: Vec<BuildStep> = Vec::new();
    // call id -> seq of the brick it placed, so an error result can pull that brick.
    let mut placed_by_call: HashMap<String, u32> = HashMap::new();
    let push = |out: &mut Vec<BuildStep>, e: &EventRecord, action, tool: String, label, target| {
        let seq = out.len() as u32;
        out.push(BuildStep {
            seq,
            ts: e.ts.to_rfc3339(),
            action,
            tool,
            label,
            target,
            undoes: None,
        });
        seq
    };

    for e in ordered {
        match &e.event {
            Event::ToolCall { tool, input, call_id, .. } => {
                let target = tool_call_abs_path(tool, input)
                    .as_deref()
                    .map(crate::util::display_path);
                let action = if is_plan_tool(tool) {
                    Some((BuildAction::Page, format!("{} (plan)", short_tool_name(tool))))
                } else {
                    // `None` is a Codex apply_patch: its FileSnapshot below places the brick.
                    tool_activity(tool, input).map(|(kind, label)| {
                        let action = match kind {
                            "write" | "run" => BuildAction::Place,
                            "spawn" => BuildAction::Spawn,
                            "ask" => BuildAction::Ask,
                            _ => BuildAction::Browse,
                        };
                        (action, label)
                    })
                };
                if let Some((action, label)) = action {
                    let seq = push(&mut out, e, action, short_tool_name(tool), label, target);
                    if action == BuildAction::Place {
                        placed_by_call.insert(call_id.clone(), seq);
                    }
                }
            }
            Event::ToolResult { call_id, status: ToolStatus::Error, summary, .. } => {
                let Some(placed) = placed_by_call.remove(call_id) else {
                    continue;
                };
                let tool = out[placed as usize].tool.clone();
                let label = summary
                    .as_deref()
                    .map(|s| crate::util::truncate_chars(s, 80))
                    .unwrap_or_else(|| format!("{tool} failed"));
                let seq = push(&mut out, e, BuildAction::Pull, tool, label, None);
                out[seq as usize].undoes = Some(placed);
            }
            // Only a snapshot that COMPLETES a call is a write (a Codex patch). Claude's
            // file-history snapshots are checkpoints of edits already counted above.
            Event::FileSnapshot { files } if files.iter().any(|f| f.call_id.is_some()) => {
                let target = match files.as_slice() {
                    [one] => Some(crate::util::display_path(&one.path)),
                    _ => None,
                };
                let label = match files.as_slice() {
                    [one] => format!("Edit {}", one.path.rsplit('/').next().unwrap_or(&one.path)),
                    many => format!("Edit {} files", many.len()),
                };
                push(&mut out, e, BuildAction::Place, "apply_patch".into(), label, target);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FileEdit, RawRef, ToolKind};
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn rec(n: u64, event: Event) -> EventRecord {
        EventRecord {
            id: format!("e{n}"),
            turn_id: "t".into(),
            session_id: "s".into(),
            ts: Utc.timestamp_opt(1_700_000_000 + n as i64, 0).single().expect("valid ts"),
            event,
            raw_ref: RawRef { source_path: "x.jsonl".into(), offset: n, line: n as u32 },
        }
    }

    fn call(n: u64, tool: &str, input: serde_json::Value) -> EventRecord {
        rec(n, Event::ToolCall {
            tool: tool.into(),
            input,
            call_id: format!("c{n}"),
            kind: ToolKind::Builtin,
        })
    }

    fn result(n: u64, call: u64, status: ToolStatus) -> EventRecord {
        rec(n, Event::ToolResult {
            call_id: format!("c{call}"),
            status,
            bytes: 0,
            summary: Some("exit code 1".into()),
        })
    }

    fn actions(log: &[BuildStep]) -> Vec<BuildAction> {
        log.iter().map(|s| s.action).collect()
    }

    #[test]
    fn writes_and_runs_place_bricks_and_reads_do_not() {
        let log = build_log(&[
            call(1, "Read", json!({"file_path": "/r/a.rs"})),
            call(2, "Edit", json!({"file_path": "/r/a.rs"})),
            call(3, "Grep", json!({"pattern": "x"})),
            call(4, "Bash", json!({"command": "cargo test"})),
            call(5, "Write", json!({"file_path": "/r/b.rs"})),
        ]);
        use BuildAction::*;
        assert_eq!(actions(&log), vec![Browse, Place, Browse, Place, Place]);
        assert_eq!(log.iter().filter(|s| s.action == Place).count(), 3);
        assert_eq!(log.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn an_error_result_pulls_exactly_the_brick_its_call_placed() {
        let log = build_log(&[
            call(1, "Edit", json!({"file_path": "/r/a.rs"})),
            call(2, "Bash", json!({"command": "cargo test"})),
            result(3, 1, ToolStatus::Ok),
            result(4, 2, ToolStatus::Error),
            // An error on a read places nothing, so it pulls nothing.
            call(5, "Read", json!({"file_path": "/r/a.rs"})),
            result(6, 5, ToolStatus::Error),
        ]);
        use BuildAction::*;
        assert_eq!(actions(&log), vec![Place, Place, Pull, Browse]);
        assert_eq!(log[2].undoes, Some(1));
        assert_eq!(log[2].label, "exit code 1");
    }

    #[test]
    fn plans_turn_pages_and_spawns_and_questions_have_their_own_actions() {
        let log = build_log(&[
            call(1, "TodoWrite", json!({"todos": []})),
            call(2, "Agent", json!({"description": "map the repo"})),
            call(3, "AskUserQuestion", json!({"questions": []})),
            call(4, "mcp__phosphor__wallet", json!({})),
        ]);
        use BuildAction::*;
        assert_eq!(actions(&log), vec![Page, Spawn, Ask, Browse]);
        assert_eq!(log[3].tool, "wallet");
    }

    #[test]
    fn only_a_snapshot_that_completes_a_call_places_a_brick() {
        let claude_checkpoint = rec(1, Event::FileSnapshot {
            files: vec![FileEdit { path: "/r/a.rs".into(), ..Default::default() }],
        });
        let codex_patch = rec(2, Event::FileSnapshot {
            files: vec![FileEdit {
                path: "/r/b.rs".into(),
                call_id: Some("p1".into()),
                ..Default::default()
            }],
        });
        let log = build_log(&[claude_checkpoint, codex_patch]);
        assert_eq!(actions(&log), vec![BuildAction::Place]);
        assert_eq!(log[0].label, "Edit b.rs");
    }

    #[test]
    fn the_log_is_ordered_by_time_whatever_order_the_events_arrive_in() {
        let a = build_log(&[
            call(2, "Edit", json!({"file_path": "/r/a.rs"})),
            call(1, "Read", json!({"file_path": "/r/a.rs"})),
        ]);
        assert_eq!(actions(&a), vec![BuildAction::Browse, BuildAction::Place]);
    }
}
