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
        let model = v
            .get("model")
            .and_then(|s| s.as_str())
            .map(str::to_string);
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

/// FAULT B: working/idle from the session's CONVERSATION STATE, not file mtime.
///
/// mtime ("file touched recently") is a racy proxy — FSEvents coalesces writes, so the
/// same idle session's mtime moves independently of real activity, flipping it
/// working↔idle between two reads seconds apart. The ingested events ARE the real
/// activity, and they are stable across reads on an unchanged store, so deriving status
/// from the last event is BOTH more honest and deterministic (this kills the flicker).
///
/// Rule on the LAST event (events are pre-sorted by `(turn idx, ts, id)`):
/// * `UserPrompt` (operator just asked, not yet answered) ⇒ Working (a "strong"
///   signal: the agent owes a reply).
/// * a `ToolCall` whose `call_id` has no following `ToolResult` (a tool is still
///   running) ⇒ Working (strong signal).
/// * anything else — a completed `AssistantText`/`TokenUsage` turn, or an answered
///   `ToolResult` — is a "quiet" tail: the agent is Working only if that event is
///   RECENT (written within `working_secs`), else Idle.
///
/// Why recency for the quiet tail: an actively-generating agent spends almost all of
/// its time with its transcript ending on `AssistantText`/`TokenUsage`/answered
/// `ToolResult` — it only momentarily shows an unanswered `ToolCall`. Classifying
/// every quiet tail as Idle therefore collapses a whole forest of busy agents to
/// Idle (observed: 14/14 live globes read idle). Recency is the honest discriminator:
/// a quiet tail 45s old is mid-stream work; one 5000s old is a session that went home.
///
/// Backstop: a "strong" Working signal whose event is older than `stale_secs` (the
/// agent fell silent mid-step and never finished) downgrades to Idle so a stuck-forever
/// session does not glow Working indefinitely. (`stale_secs >= working_secs`, so a
/// quiet tail is bounded by the tighter `working_secs` window.)
///
/// Returns `None` when there is no usable event at all (no `UserPrompt`/`ToolCall`/
/// `AssistantText`/`TokenUsage`/`ToolResult`) — the caller then falls back to mtime.
/// The process-alive check is the caller's responsibility (this is a pure function of
/// the transcript + clock).
pub fn status_from_last_event(
    events: &[(Turn, EventRecord)],
    now: DateTime<Utc>,
    working_secs: u64,
    stale_secs: u64,
) -> Option<AgentStatus> {
    // The last conversational event decides "working vs idle". Skip events that are not
    // a real activity signal (e.g. SystemNotice/ModeChange/FileSnapshot/Thinking) so a
    // trailing system record never masks the true last action.
    let last = events.iter().rev().find(|(_, e)| {
        matches!(
            e.event,
            Event::UserPrompt { .. }
                | Event::ToolCall { .. }
                | Event::ToolResult { .. }
                | Event::AssistantText { .. }
                | Event::TokenUsage { .. }
        )
    })?;

    let age_secs = now
        .signed_duration_since(last.1.ts)
        .num_seconds()
        .max(0) as u64;

    // "Strong" Working signals: the agent visibly owes work right now.
    let strong_working = match &last.1.event {
        // Operator asked; the agent has not produced its answering turn yet.
        Event::UserPrompt { .. } => true,
        // A tool call is in flight iff no later ToolResult answers its call_id. (The
        // matching result, when present, sorts after the call, so it would BE the last
        // event — but check explicitly to be robust to interleaving.)
        Event::ToolCall { call_id, .. } => !events.iter().any(|(_, e)| {
            matches!(&e.event, Event::ToolResult { call_id: c, .. } if c == call_id)
        }),
        // A completed assistant turn / token-usage / answered tool-result is a "quiet"
        // tail — not a strong signal; recency decides it below.
        _ => false,
    };

    if strong_working {
        // Stale backstop: a strong "working" verdict on an event older than the window
        // is a stuck-forever agent — settle it to idle.
        return Some(if age_secs > stale_secs {
            AgentStatus::Idle
        } else {
            AgentStatus::Working
        });
    }

    // Quiet tail: the agent is mid-stream (Working) only if the last event is RECENT.
    // A busy agent almost always ends on AssistantText/TokenUsage/answered ToolResult
    // between steps, so without this a working forest reads entirely idle.
    Some(if age_secs <= working_secs {
        AgentStatus::Working
    } else {
        AgentStatus::Idle
    })
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
            (100u32, json!({"sessionId":"s-working","cwd":"/a","pid":100})),
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
            (10u32, json!({"sessionId":"busy-stale","cwd":"/a","pid":10,"status":"busy"})),
            (20u32, json!({"sessionId":"idle-fresh","cwd":"/b","pid":20,"status":"idle"})),
            (30u32, json!({"sessionId":"nostatus-fresh","cwd":"/c","pid":30})),
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

    /// FAULT B core: status is derived from the LAST conversational event, not file
    /// mtime. Strong signals: (b) trailing UserPrompt ⇒ working, (c) a ToolCall with no
    /// answering ToolResult ⇒ working. Quiet tails (AssistantText / TokenUsage / answered
    /// ToolResult) are decided by RECENCY: (a) an old completed turn ⇒ idle, (a2) a recent
    /// completed turn ⇒ working (the agent is mid-stream between steps), (d) an old
    /// answered ToolResult ⇒ idle, (d2) a recent answered ToolResult ⇒ working. (e) no
    /// usable events ⇒ None (mtime fallback).
    #[test]
    fn status_from_last_event_classifies_by_conversation_state() {
        let now = Utc::now();
        let t = |secs: i64| now - chrono::Duration::seconds(secs);
        let working = 15u64;
        let stale = 180u64;

        // (a) last event is a completed assistant turn, OLD (past the working window) → idle.
        let done = vec![
            ev(1, t(60), Event::UserPrompt { text: "go".into(), attachments: vec![], is_meta: false }),
            ev(2, t(40), Event::AssistantText { text: "done".into() }),
        ];
        assert_eq!(status_from_last_event(&done, now, working, stale), Some(AgentStatus::Idle));

        // (a2) THE FIX: the SAME completed-turn tail, but RECENT → working. A busy agent
        // almost always ends on AssistantText between steps; without recency this collapses
        // a live forest to all-idle (the observed 14/14 idle-globes bug).
        let done_recent = vec![
            ev(1, t(8), Event::UserPrompt { text: "go".into(), attachments: vec![], is_meta: false }),
            ev(2, t(4), Event::AssistantText { text: "working on it".into() }),
        ];
        assert_eq!(
            status_from_last_event(&done_recent, now, working, stale),
            Some(AgentStatus::Working),
            "a recent quiet tail is an actively-generating agent, not idle"
        );

        // (b) last event is a UserPrompt (asked, not yet answered) → working.
        let asked = vec![
            ev(1, t(6), Event::AssistantText { text: "hi".into() }),
            ev(2, t(2), Event::UserPrompt { text: "now do X".into(), attachments: vec![], is_meta: false }),
        ];
        assert_eq!(status_from_last_event(&asked, now, working, stale), Some(AgentStatus::Working));

        // (c) a ToolCall with NO following ToolResult → working (tool in flight).
        let in_flight = vec![
            ev(1, t(3), Event::ToolCall {
                tool: "Bash".into(),
                input: serde_json::json!({"command":"cargo build"}),
                call_id: "c1".into(),
                kind: ToolKind::Builtin,
            }),
        ];
        assert_eq!(status_from_last_event(&in_flight, now, working, stale), Some(AgentStatus::Working));

        // (d) the SAME call, answered by a ToolResult and OLD (past the window) → idle.
        let answered_old = vec![
            ev(1, t(50), Event::ToolCall {
                tool: "Bash".into(),
                input: serde_json::json!({"command":"cargo build"}),
                call_id: "c1".into(),
                kind: ToolKind::Builtin,
            }),
            ev(2, t(40), Event::ToolResult {
                call_id: "c1".into(),
                status: ToolStatus::Ok,
                bytes: 10,
                summary: None,
            }),
        ];
        assert_eq!(status_from_last_event(&answered_old, now, working, stale), Some(AgentStatus::Idle));

        // (d2) the SAME call answered RECENTLY → working (a tool just finished mid-step).
        let answered_recent = vec![
            in_flight[0].clone(),
            ev(2, t(1), Event::ToolResult {
                call_id: "c1".into(),
                status: ToolStatus::Ok,
                bytes: 10,
                summary: None,
            }),
        ];
        assert_eq!(status_from_last_event(&answered_recent, now, working, stale), Some(AgentStatus::Working));

        // (e) no usable events → None (caller falls back to mtime).
        let none = vec![ev(1, t(1), Event::SystemNotice { subtype: "x".into(), data: serde_json::json!({}) })];
        assert_eq!(status_from_last_event(&none, now, working, stale), None);
    }

    /// FAULT B backstop: a "working" verdict (e.g. a UserPrompt) on an event older than
    /// `stale_secs` is a stuck-forever agent → downgraded to idle. The same UserPrompt
    /// within the window stays working.
    #[test]
    fn status_from_last_event_stale_backstop_downgrades_to_idle() {
        let now = Utc::now();
        let working = 15u64;
        let stale = 180u64;
        // A UserPrompt 10 minutes old → working-by-rule but past the backstop → idle.
        let stuck = vec![ev(
            1,
            now - chrono::Duration::seconds(600),
            Event::UserPrompt { text: "go".into(), attachments: vec![], is_meta: false },
        )];
        assert_eq!(status_from_last_event(&stuck, now, working, stale), Some(AgentStatus::Idle));
        // The same prompt 5s ago → still working.
        let fresh = vec![ev(
            1,
            now - chrono::Duration::seconds(5),
            Event::UserPrompt { text: "go".into(), attachments: vec![], is_meta: false },
        )];
        assert_eq!(status_from_last_event(&fresh, now, working, stale), Some(AgentStatus::Working));
    }

    /// FAULT B determinism: the same events evaluated at the same instant return the
    /// SAME status across repeated calls (no mtime, no clock drift between reads). This
    /// is the property that kills the working↔idle flicker.
    #[test]
    fn status_from_last_event_is_deterministic_across_reads() {
        let now = Utc::now();
        let events = vec![
            ev(1, now - chrono::Duration::seconds(3), Event::ToolCall {
                tool: "Read".into(),
                input: serde_json::json!({"file_path":"/x.rs"}),
                call_id: "c1".into(),
                kind: ToolKind::Builtin,
            }),
        ];
        let a = status_from_last_event(&events, now, 15, 180);
        let b = status_from_last_event(&events, now, 15, 180);
        assert_eq!(a, b, "identical inputs must yield identical status");
        assert_eq!(a, Some(AgentStatus::Working));
    }
}
