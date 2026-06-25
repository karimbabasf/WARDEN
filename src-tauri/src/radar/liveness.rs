//! RADAR liveness: which agents are open, and working vs idle vs closed.
//!
//! The decision logic is a **pure core** with injectable predicates so it is unit
//! testable without real PIDs or a real clock:
//! * [`partition_claude`] takes an `is_alive` and a `transcript_mtime_secs_ago`
//!   closure and returns only the LIVE sessions (Working/Idle); a dead PID is
//!   Closed and dropped from the live set (the globe implodes).
//! * [`codex_status`] is a pure function of "in sessions/ vs archived/" + mtime.
//!
//! The thin syscall wrappers ([`read_claude_registry`], [`pid_alive`]) are kept
//! out of the tested path — `pid_alive` calls `libc::kill(pid, 0)` and is only
//! invoked by the live collector, never by [`partition_claude`].

use crate::ir::{Event, EventRecord, Turn};
use chrono::{DateTime, Utc};
use std::path::Path;

/// An agent's live status. `Working` = generating now (recent transcript write),
/// `Idle` = open but quiet, `Closed` = gone (dead PID / archived) → imploded away,
/// `Terminated` = a finished subagent (its parent logged the tool-result, or it fell
/// silent past the backstop) → imploded once, never resurrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Idle,
    Closed,
    Terminated,
}

impl AgentStatus {
    /// snake_case wire value for the `radar_state` contract (`status` field).
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Idle => "idle",
            AgentStatus::Closed => "closed",
            AgentStatus::Terminated => "terminated",
        }
    }
}

/// One currently-open Claude session, distilled from a `~/.claude/sessions/<pid>.json`
/// registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    pub pid: u32,
    pub cwd: String,
    pub model: Option<String>,
}

/// Partition Claude registry entries into the LIVE working/idle set (pure core).
///
/// For each `(pid, registry_json)`:
/// * if `is_alive(pid)` is false the session is Closed and is DROPPED (a stale
///   `sessions/*.json` whose process crashed must not render as open);
/// * otherwise status prefers the registry's own live-updated `status` field
///   (`busy` ⇒ Working, anything else ⇒ Idle) when present (newer Claude — it is
///   authoritative because it reflects the agent's real state even mid-generation
///   or during a long tool run); when ABSENT (older Claude — 4 of 5 live sessions on
///   this machine) it delegates to the injected `fallback_status(session_id)`.
///
/// FAULT B: the fallback is now the caller's CONVERSATION-STATE decision
/// (`status_from_last_event` over the store, then mtime), not a racy file-mtime
/// heuristic baked in here. Keeping it injected preserves this function as a pure
/// router and lets the store-backed rule live in `assemble`. Entries missing a
/// `sessionId` are skipped.
pub fn partition_claude(
    session_files: &[(u32, serde_json::Value)],
    is_alive: &dyn Fn(u32) -> bool,
    fallback_status: &dyn Fn(&str) -> AgentStatus,
) -> Vec<(LiveSession, AgentStatus)> {
    let mut out = Vec::new();
    for (pid, v) in session_files {
        let Some(session_id) = v.get("sessionId").and_then(|s| s.as_str()) else {
            continue;
        };
        // A dead PID (zombie / crashed) is Closed → excluded from the live set.
        if !is_alive(*pid) {
            continue;
        }
        let cwd = v
            .get("cwd")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let model = v.get("model").and_then(|s| s.as_str()).map(str::to_string);
        let status = match v.get("status").and_then(|s| s.as_str()) {
            Some("busy") => AgentStatus::Working,
            Some(_) => AgentStatus::Idle,
            None => fallback_status(session_id),
        };
        out.push((
            LiveSession {
                session_id: session_id.to_string(),
                pid: *pid,
                cwd,
                model,
            },
            status,
        ));
    }
    out
}

/// Codex liveness from the file's location + freshness (pure).
///
/// A rollout in `archived_sessions/` is finished → Closed (the archive move is the
/// "done" signal). A rollout still in `sessions/` is Working when written within
/// `working_threshold_secs`, else Idle. A file in neither is Closed (gone).
pub fn codex_status(
    in_sessions: bool,
    in_archived: bool,
    mtime_secs_ago: Option<u64>,
    working_threshold_secs: u64,
) -> AgentStatus {
    if in_archived {
        return AgentStatus::Closed;
    }
    if !in_sessions {
        return AgentStatus::Closed;
    }
    match mtime_secs_ago {
        Some(secs) if secs < working_threshold_secs => AgentStatus::Working,
        _ => AgentStatus::Idle,
    }
}

/// Working/idle from the session's CONVERSATION STATE — the LAST real action in the
/// transcript, read SEMANTICALLY (not by a recency timer).
///
/// Why semantic, not a recency window: the transcript is written per step, and the
/// SHAPE of the last step says exactly what the agent is doing right now. An actively
/// working agent's transcript ends mid tool-loop — on a `ToolCall` (a tool is in
/// flight) or a `ToolResult` (a tool just returned and the next assistant turn is
/// coming). A finished agent's transcript ends on an `AssistantText`: a Claude
/// `end_turn` message is text-only (a message that will CONTINUE carries `tool_use`,
/// which maps to a `ToolCall`, never a trailing `AssistantText`), so a trailing
/// assistant text means the turn COMPLETED and the agent is waiting on the operator.
///
/// This reads "is it working right now?" with ~one filesystem event of latency, and is
/// also DETERMINISTIC across reads (stable on an unchanged store → no flicker). The old
/// recency window was a guess that flipped a busy agent to idle during any long
/// generation / tool run that wrote nothing for a few seconds, and (worse) leaned on a
/// Claude registry `status` field the DESKTOP app does not write — so desktop agents
/// fell through to that timer and read idle while hard at work.
///
/// Rule on the LAST action event — bookkeeping (`TokenUsage`/`Thinking`/`SystemNotice`/
/// `ModeChange`/`FileSnapshot`) is skipped because it trails or accompanies the real
/// action and would mask it (e.g. `TokenUsage` is emitted right AFTER each assistant
/// message):
/// * `AssistantText` ⇒ Idle  (completed `end_turn`; the agent stopped, waiting on the operator);
/// * `UserPrompt`    ⇒ Working (the operator asked; the agent owes its answer);
/// * `ToolCall`      ⇒ Working (a tool is in flight — its result is not written yet);
/// * `ToolResult`    ⇒ Working (a tool just returned; the next assistant turn is coming).
///
/// Backstop: a Working verdict whose last action is older than `stale_secs` is a wedged
/// step (the agent fell silent mid-tool and never continued) → Idle, so a stuck globe
/// does not glow forever. A live agent rewrites its transcript far inside this window,
/// so the backstop never fires on real work.
///
/// Returns `None` when there is no usable action event at all — the caller then falls
/// back to mtime. The process-alive check is the caller's responsibility (this is a
/// pure function of the transcript + clock).
pub fn status_from_last_event(
    events: &[(Turn, EventRecord)],
    now: DateTime<Utc>,
    stale_secs: u64,
) -> Option<AgentStatus> {
    // The newest real ACTION decides working vs idle. Use timestamp/offset rather
    // than Vec order: Claude incremental tail parses can reset turn indexes, and the
    // store's `(turn.idx, ts)` ordering may put an older tool-result after a newer
    // final assistant message.
    let last = events
        .iter()
        .filter(|(_, e)| action_priority(&e.event).is_some())
        .max_by(|(_, a), (_, b)| {
            a.ts.cmp(&b.ts)
                .then(a.raw_ref.offset.cmp(&b.raw_ref.offset))
                .then(action_priority(&a.event).cmp(&action_priority(&b.event)))
                .then(a.id.cmp(&b.id))
        })?;

    // A completed assistant turn (text-only `end_turn`) is the ONLY idle tail; every
    // other action (operator prompt, tool in flight, tool just returned) is mid-step.
    if matches!(last.1.event, Event::AssistantText { .. })
        || matches!(&last.1.event, Event::UserPrompt { text, .. } if is_completed_task_notification(text))
    {
        return Some(AgentStatus::Idle);
    }

    // Backstop: a Working tail older than the stale window is a wedged step → Idle.
    let age_secs = now.signed_duration_since(last.1.ts).num_seconds().max(0) as u64;
    Some(if age_secs > stale_secs {
        AgentStatus::Idle
    } else {
        AgentStatus::Working
    })
}

fn action_priority(event: &Event) -> Option<u8> {
    match event {
        Event::UserPrompt { .. } => Some(3),
        Event::ToolCall { .. } => Some(3),
        Event::ToolResult { .. } => Some(2),
        Event::AssistantText { .. } => Some(1),
        _ => None,
    }
}

fn is_completed_task_notification(text: &str) -> bool {
    text.contains("<task-notification>") && text.contains("<status>completed</status>")
}

/// Read every `<pid>.json` in the Claude liveness registry dir into `(pid, json)`
/// pairs (thin filesystem wrapper, not in the tested decision path). The `pid` is
/// taken from the JSON `pid` field, falling back to the filename stem. A missing
/// dir yields an empty vec (the collector then falls back to transcript-mtime
/// liveness — see the design spec's version-dependence note).
pub fn read_claude_registry(dir: &Path) -> Vec<(u32, serde_json::Value)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let pid = v
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .map(|p| p as u32)
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u32>().ok())
            });
        if let Some(pid) = pid {
            out.push((pid, v));
        }
    }
    out
}

/// True when `pid` is a live process. Uses `kill(pid, 0)` — sends no signal, just
/// probes existence/permission. Thin syscall wrapper kept OUT of the unit-tested
/// path (callers inject a predicate into [`partition_claude`] instead).
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 performs only the error checking and never
    // delivers a signal; it cannot corrupt memory.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The pure partition: an alive pid with a fresh transcript is Working, an
    /// alive pid with a stale transcript is Idle, and a dead pid is dropped
    /// (Closed → excluded from the live set). No real PIDs or clock involved.
    #[test]
    fn partition_claude_classifies_working_idle_and_drops_dead() {
        let files = vec![
            (
                100u32,
                json!({"sessionId":"s-working","cwd":"/a","pid":100}),
            ),
            (200u32, json!({"sessionId":"s-idle","cwd":"/b","pid":200})),
            (300u32, json!({"sessionId":"s-dead","cwd":"/c","pid":300})),
        ];
        let is_alive = |pid: u32| pid != 300; // 300 is dead
                                              // The injected fallback decides status when the registry has no `status` field.
        let fallback = |sid: &str| match sid {
            "s-working" => AgentStatus::Working,
            "s-idle" => AgentStatus::Idle,
            _ => AgentStatus::Idle,
        };

        let live = partition_claude(&files, &is_alive, &fallback);
        // Dead pid 300 excluded → exactly two live sessions.
        assert_eq!(live.len(), 2, "dead pid must be dropped from the live set");

        let working = live
            .iter()
            .find(|(s, _)| s.session_id == "s-working")
            .expect("working session present");
        assert_eq!(working.1, AgentStatus::Working);
        assert_eq!(working.0.cwd, "/a");
        assert_eq!(working.0.pid, 100);

        let idle = live
            .iter()
            .find(|(s, _)| s.session_id == "s-idle")
            .expect("idle session present");
        assert_eq!(idle.1, AgentStatus::Idle);

        assert!(
            !live.iter().any(|(s, _)| s.session_id == "s-dead"),
            "dead session must not appear"
        );
    }

    /// The newer Claude registry (v2.1.187+) writes a live-updated
    /// `"status":"busy"|"idle"` that is AUTHORITATIVE over the injected fallback: a
    /// `busy` agent is Working even when the fallback would say idle (it is
    /// mid-generation / running a long tool), and an `idle` agent is Idle even when the
    /// fallback would say working. A registry entry with no `status` field (older
    /// Claude) defers to the injected fallback (conversation-state in production).
    #[test]
    fn partition_claude_prefers_registry_status_over_mtime() {
        let files = vec![
            (
                10u32,
                json!({"sessionId":"busy-stale","cwd":"/a","pid":10,"status":"busy"}),
            ),
            (
                20u32,
                json!({"sessionId":"idle-fresh","cwd":"/b","pid":20,"status":"idle"}),
            ),
            (
                30u32,
                json!({"sessionId":"nostatus-fresh","cwd":"/c","pid":30}),
            ),
        ];
        let is_alive = |_pid: u32| true;
        // Fallback would call busy-stale/idle-fresh Working, but the registry `status`
        // overrides both; nostatus-fresh has no status → the fallback decides (Working).
        let fallback = |sid: &str| match sid {
            "busy-stale" => AgentStatus::Working,
            "idle-fresh" => AgentStatus::Working,
            "nostatus-fresh" => AgentStatus::Working,
            _ => AgentStatus::Idle,
        };
        let live = partition_claude(&files, &is_alive, &fallback);
        let st = |sid: &str| {
            live.iter()
                .find(|(s, _)| s.session_id == sid)
                .map(|(_, st)| *st)
        };
        assert_eq!(
            st("busy-stale"),
            Some(AgentStatus::Working),
            "status=busy is authoritative over a stale transcript mtime"
        );
        assert_eq!(
            st("idle-fresh"),
            Some(AgentStatus::Idle),
            "status=idle is authoritative over a fresh transcript mtime"
        );
        assert_eq!(
            st("nostatus-fresh"),
            Some(AgentStatus::Working),
            "no status field falls back to the mtime heuristic"
        );
    }

    /// A registry entry with no `status` field defers to the injected fallback; an
    /// Idle fallback yields Idle (the conservative default when there is no signal).
    #[test]
    fn partition_claude_unknown_mtime_is_idle() {
        let files = vec![(1u32, json!({"sessionId":"s","cwd":"/x"}))];
        let live = partition_claude(&files, &|_| true, &|_| AgentStatus::Idle);
        assert_eq!(live[0].1, AgentStatus::Idle);
    }

    /// Codex: archived → Closed; live + fresh → Working; live + stale → Idle;
    /// in neither → Closed.
    #[test]
    fn codex_status_maps_location_and_freshness() {
        // Archived (the "done" move) → Closed regardless of mtime.
        assert_eq!(codex_status(false, true, Some(0), 5), AgentStatus::Closed);
        assert_eq!(codex_status(true, true, Some(0), 5), AgentStatus::Closed);
        // Live + written 1s ago → Working.
        assert_eq!(codex_status(true, false, Some(1), 5), AgentStatus::Working);
        // Live + written 30s ago → Idle.
        assert_eq!(codex_status(true, false, Some(30), 5), AgentStatus::Idle);
        // In neither dir → Closed (gone).
        assert_eq!(codex_status(false, false, None, 5), AgentStatus::Closed);
    }

    /// `read_claude_registry` parses real `<pid>.json` files from a temp dir and
    /// recovers the pid from the JSON `pid` field.
    #[test]
    fn read_claude_registry_parses_pid_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1478.json"),
            r#"{"pid":1478,"sessionId":"abc","cwd":"/work","entrypoint":"claude-desktop"}"#,
        )
        .unwrap();
        // A non-json file is ignored.
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();

        let entries = read_claude_registry(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, 1478);
        assert_eq!(
            entries[0].1.get("sessionId").and_then(|v| v.as_str()),
            Some("abc")
        );
    }

    /// A missing registry dir yields an empty vec (the collector then falls back to
    /// transcript-mtime liveness).
    #[test]
    fn read_claude_registry_missing_dir_is_empty() {
        assert!(read_claude_registry(Path::new("/no/such/dir/warden-x")).is_empty());
    }

    /// B4: a finished subagent's wire status. New snake_case value in the
    /// `radar_state` contract; existing values are unchanged.
    #[test]
    fn agent_status_terminated_wire_value() {
        assert_eq!(AgentStatus::Terminated.as_str(), "terminated");
    }

    // ── FAULT B: last-event status helpers ───────────────────────────────────────
    use crate::ir::{EventRecord, RawRef, Role, ToolKind, ToolStatus, Turn};

    fn ev(i: u32, ts: DateTime<Utc>, event: Event) -> (Turn, EventRecord) {
        let turn = Turn {
            id: format!("t{i}"),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: i,
            started_at: ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: format!("e{i}"),
            turn_id: format!("t{i}"),
            session_id: "s".into(),
            ts,
            event,
            raw_ref: RawRef {
                source_path: std::path::PathBuf::from("/x.jsonl"),
                offset: i as u64,
                line: i,
            },
        };
        (turn, rec)
    }

    /// Semantic liveness: status comes from the SHAPE of the last action event, not a
    /// recency timer. (a) a trailing completed `AssistantText` (`end_turn`) ⇒ idle, even
    /// when (a2) it is RECENT — a finished turn means the agent stopped. (b) a trailing
    /// `UserPrompt` ⇒ working (the agent owes an answer). (c) a `ToolCall` in flight ⇒
    /// working. (d) a trailing `ToolResult` ⇒ working (a tool just returned; the next
    /// assistant turn is coming). (d2) a bookkeeping `TokenUsage` after the action is
    /// SKIPPED, so the real action still decides. (e) no usable action ⇒ None (mtime fallback).
    #[test]
    fn status_from_last_event_classifies_by_conversation_state() {
        let now = Utc::now();
        let t = |secs: i64| now - chrono::Duration::seconds(secs);
        let stale = 180u64;

        // (a) trailing completed AssistantText (end_turn) → idle (the agent finished).
        let done = vec![
            ev(
                1,
                t(60),
                Event::UserPrompt {
                    text: "go".into(),
                    attachments: vec![],
                    is_meta: false,
                },
            ),
            ev(
                2,
                t(40),
                Event::AssistantText {
                    text: "done".into(),
                },
            ),
        ];
        assert_eq!(
            status_from_last_event(&done, now, stale),
            Some(AgentStatus::Idle)
        );

        // (a2) the SAME completed tail but RECENT → STILL idle. A completed assistant turn
        // means the agent stopped and is waiting on the operator, regardless of how recent.
        let done_recent = vec![
            ev(
                1,
                t(8),
                Event::UserPrompt {
                    text: "go".into(),
                    attachments: vec![],
                    is_meta: false,
                },
            ),
            ev(
                2,
                t(4),
                Event::AssistantText {
                    text: "all done".into(),
                },
            ),
        ];
        assert_eq!(
            status_from_last_event(&done_recent, now, stale),
            Some(AgentStatus::Idle),
            "a completed assistant turn is the agent stopping → idle, even if recent"
        );

        // (b) last event is a UserPrompt (asked, not yet answered) → working.
        let asked = vec![
            ev(1, t(6), Event::AssistantText { text: "hi".into() }),
            ev(
                2,
                t(2),
                Event::UserPrompt {
                    text: "now do X".into(),
                    attachments: vec![],
                    is_meta: false,
                },
            ),
        ];
        assert_eq!(
            status_from_last_event(&asked, now, stale),
            Some(AgentStatus::Working)
        );

        // (c) a ToolCall with NO following ToolResult → working (tool in flight).
        let in_flight = vec![ev(
            1,
            t(3),
            Event::ToolCall {
                tool: "Bash".into(),
                input: serde_json::json!({"command":"cargo build"}),
                call_id: "c1".into(),
                kind: ToolKind::Builtin,
            },
        )];
        assert_eq!(
            status_from_last_event(&in_flight, now, stale),
            Some(AgentStatus::Working)
        );

        // (d) a trailing ToolResult (a tool just returned; the agent is about to continue)
        // → working. Even an older one (50s) is mid-step — only the stale backstop (180s)
        // settles it. (The old recency rule wrongly called this idle.)
        let tool_returned = vec![
            ev(
                1,
                t(55),
                Event::ToolCall {
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"cargo build"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            ev(
                2,
                t(50),
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: ToolStatus::Ok,
                    bytes: 10,
                    summary: None,
                },
            ),
        ];
        assert_eq!(
            status_from_last_event(&tool_returned, now, stale),
            Some(AgentStatus::Working)
        );

        // (d2) a bookkeeping TokenUsage trailing the action is SKIPPED — the ToolResult
        // still decides (without this skip every assistant message would mask its action).
        let tool_then_usage = vec![
            ev(
                1,
                t(5),
                Event::ToolCall {
                    tool: "Read".into(),
                    input: serde_json::json!({"file_path":"/x.rs"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            ev(
                2,
                t(3),
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: ToolStatus::Ok,
                    bytes: 10,
                    summary: None,
                },
            ),
            ev(
                3,
                t(3),
                Event::TokenUsage {
                    input: 5,
                    output: 9,
                    cache_creation: 0,
                    cache_read: 0,
                    model: "claude-opus-4-8".into(),
                    orchestration: None,
                },
            ),
        ];
        assert_eq!(
            status_from_last_event(&tool_then_usage, now, stale),
            Some(AgentStatus::Working),
            "trailing TokenUsage is bookkeeping; the ToolResult action decides → working"
        );

        // (e) no usable action events → None (caller falls back to mtime).
        let none = vec![ev(
            1,
            t(1),
            Event::SystemNotice {
                subtype: "x".into(),
                data: serde_json::json!({}),
            },
        )];
        assert_eq!(status_from_last_event(&none, now, stale), None);
    }

    /// Claude incremental tail parses can carry reset/partial turn indexes, so store
    /// order is not always chronological. A finished assistant message newer than an
    /// older tool-result must settle the agent to idle even if the older result sorts
    /// later in storage order.
    #[test]
    fn status_from_last_event_uses_newest_action_timestamp_not_storage_order() {
        let now = Utc::now();
        let older = now - chrono::Duration::seconds(20);
        let newer = now - chrono::Duration::seconds(5);
        let events = vec![
            ev(
                2,
                newer,
                Event::AssistantText {
                    text: "all done".into(),
                },
            ),
            ev(
                3,
                older,
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: ToolStatus::Ok,
                    bytes: 10,
                    summary: None,
                },
            ),
        ];

        assert_eq!(
            status_from_last_event(&events, now, 180),
            Some(AgentStatus::Idle),
            "newer AssistantText wins over an older ToolResult even when storage order is inverted"
        );
    }

    #[test]
    fn status_from_last_event_treats_completed_task_notification_as_idle() {
        let now = Utc::now();
        let completed = "<task-notification>\n\
<tool-use-id>toolu_done</tool-use-id>\n\
<status>completed</status>\n\
</task-notification>";
        let events = vec![
            ev(
                1,
                now - chrono::Duration::seconds(20),
                Event::ToolResult {
                    call_id: "toolu_done".into(),
                    status: ToolStatus::Ok,
                    bytes: 20,
                    summary: Some("Async agent launched successfully.".into()),
                },
            ),
            ev(
                2,
                now - chrono::Duration::seconds(1),
                Event::UserPrompt {
                    text: completed.into(),
                    attachments: vec![],
                    is_meta: false,
                },
            ),
        ];

        assert_eq!(
            status_from_last_event(&events, now, 180),
            Some(AgentStatus::Idle),
            "a completed async subagent notification is not a new operator prompt"
        );
    }

    /// Backstop: a Working verdict (e.g. a trailing UserPrompt) on an action older than
    /// `stale_secs` is a wedged/abandoned step → downgraded to idle. The same UserPrompt
    /// within the window stays working.
    #[test]
    fn status_from_last_event_stale_backstop_downgrades_to_idle() {
        let now = Utc::now();
        let stale = 180u64;
        // A UserPrompt 10 minutes old → working-by-rule but past the backstop → idle.
        let stuck = vec![ev(
            1,
            now - chrono::Duration::seconds(600),
            Event::UserPrompt {
                text: "go".into(),
                attachments: vec![],
                is_meta: false,
            },
        )];
        assert_eq!(
            status_from_last_event(&stuck, now, stale),
            Some(AgentStatus::Idle)
        );
        // The same prompt 5s ago → still working.
        let fresh = vec![ev(
            1,
            now - chrono::Duration::seconds(5),
            Event::UserPrompt {
                text: "go".into(),
                attachments: vec![],
                is_meta: false,
            },
        )];
        assert_eq!(
            status_from_last_event(&fresh, now, stale),
            Some(AgentStatus::Working)
        );
    }

    /// Determinism: the same events evaluated at the same instant return the SAME status
    /// across repeated calls — the property that kills the working↔idle flicker.
    #[test]
    fn status_from_last_event_is_deterministic_across_reads() {
        let now = Utc::now();
        let events = vec![ev(
            1,
            now - chrono::Duration::seconds(3),
            Event::ToolCall {
                tool: "Read".into(),
                input: serde_json::json!({"file_path":"/x.rs"}),
                call_id: "c1".into(),
                kind: ToolKind::Builtin,
            },
        )];
        let a = status_from_last_event(&events, now, 180);
        let b = status_from_last_event(&events, now, 180);
        assert_eq!(a, b, "identical inputs must yield identical status");
        assert_eq!(a, Some(AgentStatus::Working));
    }
}
