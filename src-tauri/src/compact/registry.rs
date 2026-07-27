//! The Claude Code session registry, read as a typed idle signal.
//!
//! `~/.claude/sessions/<pid>.json` is one file per live process. RADAR already
//! reads it for bloom/implode; this module reads it for a much sharper question,
//! "has this session STOPPED WORKING", which needs the distinction RADAR does not
//! make.
//!
//! Two measured facts drive the whole design, both from the mechanism survey:
//!
//! 1. `status` has THREE values, `busy`, `idle`, and `shell`. `shell` is a
//!    long-running foreground tool call, not idleness: one real session on this
//!    machine sat in `shell` for roughly three hours running a dev server.
//!    Compacting there would land mid-command, so `shell` is explicitly NOT idle
//!    and an unrecognised value is not idle either.
//!
//! 2. The file is written ON TRANSITION ONLY, never on a timer. mtime age and
//!    `statusUpdatedAt` age matched to the second across six live sessions. So an
//!    FSEvents watch fires at the instant of the transition (no polling latency),
//!    and file freshness is NOT a liveness check: a legitimately live session can
//!    sit unwritten for hours. Liveness is `kill(pid, 0)` and nothing else.

use std::path::Path;

/// The three states the registry actually writes, plus the two WARDEN needs for
/// records whose file is missing or malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Generating or otherwise mid-turn.
    Busy,
    /// Turn finished, waiting on the operator. The ONLY state that fires.
    Idle,
    /// A foreground tool call is running. NOT idle: firing here compacts a
    /// session in the middle of a command.
    Shell,
    /// The field is absent (older Claude) or carries a value this build does not
    /// recognise. Fails closed: never idle.
    Unknown,
    /// No registry file for this session at all: the process exited.
    Gone,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::Busy => "busy",
            SessionStatus::Idle => "idle",
            SessionStatus::Shell => "shell",
            SessionStatus::Unknown => "unknown",
            SessionStatus::Gone => "gone",
        }
    }

    /// The one predicate that is allowed to gate a compaction.
    pub fn is_idle(&self) -> bool {
        matches!(self, SessionStatus::Idle)
    }
}

/// Map the registry's raw `status` string. Anything unrecognised is `Unknown`,
/// which never fires, so a future fourth state cannot be mistaken for idleness.
pub fn parse_status(raw: Option<&str>) -> SessionStatus {
    match raw {
        Some("busy") => SessionStatus::Busy,
        Some("idle") => SessionStatus::Idle,
        Some("shell") => SessionStatus::Shell,
        _ => SessionStatus::Unknown,
    }
}

/// One live Claude session as the compaction path needs it. A subset of the
/// registry's fields: the rest (`startedAt`, `version`, `peerProtocol`, ...) has
/// no bearing on arming and is deliberately not carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    /// `cli` for a terminal session, `claude-vscode` / `claude-desktop` / `sdk-*`
    /// otherwise. This is what decides whether a control channel can exist.
    pub entrypoint: String,
    /// The human label the CLI shows, for example `machine-89`.
    pub name: String,
    pub status: SessionStatus,
}

/// Parse one registry JSON value. `None` when it carries no `sessionId`, since
/// without one there is nothing to arm against.
pub fn parse_entry(pid: u32, v: &serde_json::Value) -> Option<RegistryEntry> {
    let session_id = v.get("sessionId").and_then(|s| s.as_str())?.to_string();
    let str_field = |k: &str| {
        v.get(k)
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Some(RegistryEntry {
        pid: v
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .map(|p| p as u32)
            .unwrap_or(pid),
        session_id,
        cwd: str_field("cwd"),
        entrypoint: str_field("entrypoint"),
        name: str_field("name"),
        status: parse_status(v.get("status").and_then(|s| s.as_str())),
    })
}

/// Read the whole registry directory into typed entries.
///
/// Reuses RADAR's `read_claude_registry` so there is exactly one piece of code
/// that knows the directory layout, and layers the typed status parse on top.
pub fn snapshot(dir: &Path) -> Vec<RegistryEntry> {
    crate::radar::liveness::read_claude_registry(dir)
        .iter()
        .filter_map(|(pid, v)| parse_entry(*pid, v))
        .collect()
}

/// Find one session by its harness session id.
pub fn find(dir: &Path, session_id: &str) -> Option<RegistryEntry> {
    snapshot(dir)
        .into_iter()
        .find(|e| e.session_id == session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim registry lines captured from `~/.claude/sessions/*.json` on this
    /// machine (Claude Code 2.1.220), one per observed status. The session ids and
    /// pids are real, which is the point: the parser is tested against the actual
    /// on-disk shape rather than a shape we invented.
    const REAL_IDLE: &str = r#"{"pid":71015,"sessionId":"aaaaaaaa-0000-4000-8000-000000000001","cwd":"/Users/dev","startedAt":1785178361842,"procStart":"Mon Jul 27 18:52:41 2026","version":"2.1.220","peerProtocol":1,"kind":"interactive","entrypoint":"cli","name":"machine-89","nameSource":"derived","status":"idle","updatedAt":1785183890237,"statusUpdatedAt":1785183890237}"#;
    const REAL_SHELL: &str = r#"{"pid":58338,"sessionId":"bbbbbbbb-0000-4000-8000-000000000002","cwd":"/Users/dev","startedAt":1785173990885,"procStart":"Mon Jul 27 17:39:50 2026","version":"2.1.220","peerProtocol":1,"kind":"interactive","entrypoint":"cli","name":"machine-75","nameSource":"derived","status":"shell","updatedAt":1785174317806,"statusUpdatedAt":1785174317806}"#;
    const REAL_BUSY: &str = r#"{"pid":17697,"sessionId":"cccccccc-0000-4000-8000-000000000003","cwd":"/Users/dev","startedAt":1785181795382,"procStart":"Mon Jul 27 19:49:54 2026","version":"2.1.220","peerProtocol":1,"kind":"interactive","entrypoint":"cli","name":"machine-f6","nameSource":"derived","status":"busy","updatedAt":1785182626961,"statusUpdatedAt":1785182626961}"#;

    fn parse(raw: &str) -> RegistryEntry {
        let v: serde_json::Value = serde_json::from_str(raw).expect("fixture is valid JSON");
        parse_entry(0, &v).expect("fixture carries a sessionId")
    }

    #[test]
    fn parses_the_three_real_statuses_from_on_disk_fixtures() {
        let idle = parse(REAL_IDLE);
        assert_eq!(idle.status, SessionStatus::Idle);
        assert_eq!(idle.pid, 71015);
        assert_eq!(idle.name, "machine-89");
        assert_eq!(idle.entrypoint, "cli");
        assert_eq!(idle.session_id, "aaaaaaaa-0000-4000-8000-000000000001");

        assert_eq!(parse(REAL_BUSY).status, SessionStatus::Busy);
        assert_eq!(parse(REAL_SHELL).status, SessionStatus::Shell);
    }

    /// The single most dangerous confusion in this feature: `shell` reads like
    /// "not generating" but means "a foreground command is running right now".
    #[test]
    fn shell_is_never_idle_and_neither_is_anything_unrecognised() {
        assert!(!parse(REAL_SHELL).status.is_idle());
        assert!(!parse(REAL_BUSY).status.is_idle());
        assert!(parse(REAL_IDLE).status.is_idle());

        assert_eq!(parse_status(None), SessionStatus::Unknown);
        assert_eq!(parse_status(Some("compacting")), SessionStatus::Unknown);
        assert!(!SessionStatus::Unknown.is_idle());
        assert!(!SessionStatus::Gone.is_idle());
    }

    #[test]
    fn an_entry_without_a_session_id_is_not_armable() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"pid":123,"status":"idle"}"#).expect("valid JSON");
        assert!(parse_entry(123, &v).is_none());
    }

    #[test]
    fn pid_falls_back_to_the_filename_when_the_field_is_missing() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"sessionId":"abc","status":"idle"}"#).expect("valid JSON");
        let e = parse_entry(4242, &v).expect("has a sessionId");
        assert_eq!(e.pid, 4242);
    }

    #[test]
    fn snapshot_reads_a_directory_of_real_shaped_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("71015.json"), REAL_IDLE).expect("write");
        std::fs::write(dir.path().join("58338.json"), REAL_SHELL).expect("write");
        std::fs::write(dir.path().join("notjson.txt"), "garbage").expect("write");

        let mut got = snapshot(dir.path());
        got.sort_by_key(|e| e.pid);
        assert_eq!(got.len(), 2, "the non-json file must be ignored");
        assert_eq!(got[0].status, SessionStatus::Shell);
        assert_eq!(got[1].status, SessionStatus::Idle);

        let found = find(dir.path(), "aaaaaaaa-0000-4000-8000-000000000001")
            .expect("the idle session is present");
        assert_eq!(found.pid, 71015);
        assert!(find(dir.path(), "no-such-session").is_none());
    }
}
