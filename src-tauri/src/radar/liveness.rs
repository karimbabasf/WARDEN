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
///   (`busy` ⇒ Working, anything else ⇒ Idle) when present (newer Claude), and
///   falls back to "transcript written within `working_threshold_secs`" otherwise.
///
/// `transcript_mtime_secs_ago(session_id)` returns seconds since the transcript's
/// last write, or `None` when it cannot be determined (treated as Idle — open but
/// no fresh activity). Entries missing a `sessionId`/`pid`/`cwd` are skipped.
pub fn partition_claude(
    session_files: &[(u32, serde_json::Value)],
    is_alive: &dyn Fn(u32) -> bool,
    transcript_mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    working_threshold_secs: u64,
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
        // The newer Claude registry (v2.1.187+) writes a live-updated `status`
        // ("busy"/"idle") — it is authoritative because it reflects the agent's real
        // state even mid-generation or during a long tool run (when the transcript
        // is not being written). Older versions omit it → fall back to the
        // transcript-mtime heuristic.
        let status = match v.get("status").and_then(|s| s.as_str()) {
            Some("busy") => AgentStatus::Working,
            Some(_) => AgentStatus::Idle,
            None => match transcript_mtime_secs_ago(session_id) {
                Some(secs) if secs < working_threshold_secs => AgentStatus::Working,
                _ => AgentStatus::Idle,
            },
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
        let mtime = |sid: &str| match sid {
            "s-working" => Some(2u64), // 2s ago → Working
            "s-idle" => Some(60u64),   // 60s ago → Idle
            _ => None,
        };

        let live = partition_claude(&files, &is_alive, &mtime, 5);
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
    /// `"status":"busy"|"idle"` that is AUTHORITATIVE over the transcript-mtime
    /// heuristic: a `busy` agent is Working even when its transcript is stale (it is
    /// mid-generation / running a long tool), and an `idle` agent is Idle even when
    /// its transcript was just written. A registry entry with no `status` field
    /// (older Claude) falls back to the mtime heuristic.
    #[test]
    fn partition_claude_prefers_registry_status_over_mtime() {
        let files = vec![
            (10u32, json!({"sessionId":"busy-stale","cwd":"/a","pid":10,"status":"busy"})),
            (20u32, json!({"sessionId":"idle-fresh","cwd":"/b","pid":20,"status":"idle"})),
            (30u32, json!({"sessionId":"nostatus-fresh","cwd":"/c","pid":30})),
        ];
        let is_alive = |_pid: u32| true;
        let mtime = |sid: &str| match sid {
            "busy-stale" => Some(999u64),   // stale, but status=busy → Working
            "idle-fresh" => Some(0u64),     // just written, but status=idle → Idle
            "nostatus-fresh" => Some(1u64), // no status → mtime fallback → Working
            _ => None,
        };
        let live = partition_claude(&files, &is_alive, &mtime, 5);
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

    /// A registry entry whose mtime is unknown (None) defaults to Idle, not Working.
    #[test]
    fn partition_claude_unknown_mtime_is_idle() {
        let files = vec![(1u32, json!({"sessionId":"s","cwd":"/x"}))];
        let live = partition_claude(&files, &|_| true, &|_| None, 5);
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
}
