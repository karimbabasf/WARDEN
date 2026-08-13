//! "Take me there": which terminal window is this agent running in?
//!
//! The panel shows an agent; the operator wants the window it lives in. The
//! chain is `agent id -> store session -> harness session id -> registry pid ->
//! controlling tty -> the emulator tab that owns that tty`. Every hop can fail
//! for a different real reason, and the panel says which one rather than
//! offering a button that quietly does nothing.
//!
//! Two rules shape the module:
//!
//! * **The pure core is separate from the syscalls.** [`locate`] takes the three
//!   process probes as closures, so every verdict below is unit tested without a
//!   real pid, a real tty or a real emulator.
//! * **A subagent has no window of its own.** It runs inside its root's process,
//!   so the walk goes UP to the root first and the answer names that root. The
//!   alternative (reporting "no window") would be true and useless: the window
//!   the operator wants is the parent's.
//!
//! WARDEN never types into the window it raises. See `platform/macos.rs` for why
//! that boundary is drawn at the AppleScript itself.

use crate::ir::{Harness, Session};
use crate::platform::{TerminalApp, TerminalHost};
use crate::radar::RadarAgent;
use serde::Serialize;
use serde_json::Value;

/// Cycle guard for the parent walk. The forest is a tree by construction, so
/// this only ever fires on corrupt state; it exists so a bad edge cannot hang a
/// click.
const MAX_PARENT_WALK: usize = 32;

/// What the panel is told about one agent's window.
///
/// `reachable` is the ONLY thing the button keys off, so the control can never
/// claim an act the backend cannot make. When it is false, `reason` is always
/// present and is written for the operator, not for a log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTarget {
    pub reachable: bool,
    /// The emulator's display name ("Terminal"), when one was resolved.
    pub app: Option<String>,
    /// The agent that actually owns the window: the ROOT ancestor for a
    /// subagent, and `None` when it is the selected agent itself. Set so the
    /// panel can say whose window it is about to raise.
    pub via_agent_id: Option<String>,
    /// That root's display label, for the same sentence.
    pub via_label: Option<String>,
    /// Why not, in one plain line. `None` when reachable.
    pub reason: Option<String>,
}

impl TerminalTarget {
    fn blocked(reason: impl Into<String>) -> Self {
        Self {
            reachable: false,
            app: None,
            via_agent_id: None,
            via_label: None,
            reason: Some(reason.into()),
        }
    }
}

/// A resolved window, plus the handle needed to raise it.
///
/// `focus` is `Some` exactly when `target.reachable`, which is what keeps the
/// probe and the act from ever disagreeing: both commands call [`locate`] and
/// read different halves of one answer.
#[derive(Debug, Clone)]
pub struct Located {
    pub target: TerminalTarget,
    pub focus: Option<(TerminalApp, String)>,
}

impl Located {
    fn blocked(reason: impl Into<String>) -> Self {
        Self {
            target: TerminalTarget::blocked(reason),
            focus: None,
        }
    }
}

/// Walk `agent_id` up to its root, returning the root agent.
///
/// `None` when the id is not on the current radar at all, which happens
/// legitimately: the panel can outlive a globe by one poll.
pub fn root_of<'a>(agents: &'a [RadarAgent], agent_id: &str) -> Option<&'a RadarAgent> {
    let mut current = agents.iter().find(|a| a.id == agent_id)?;
    for _ in 0..MAX_PARENT_WALK {
        let Some(parent_id) = current.parent_id.as_deref() else {
            return Some(current);
        };
        match agents.iter().find(|a| a.id == parent_id) {
            // An unparented globe (the sidecar named a parent that is not on the
            // board) is its own root: better an honest miss than the wrong window.
            None => return Some(current),
            Some(parent) => current = parent,
        }
    }
    Some(current)
}

/// The live pid for a harness session id, from the Claude session registry
/// (`~/.claude/sessions/<pid>.json`, already parsed by
/// `radar::liveness::read_claude_registry`).
///
/// The registry's own `sessionId` field is matched rather than the filename, and
/// the filename pid is the fallback the reader already applies, so a rotated
/// file cannot point at the wrong process.
pub fn pid_for_session(registry: &[(u32, Value)], external_id: &str) -> Option<u32> {
    registry
        .iter()
        .find(|(_, v)| v.get("sessionId").and_then(Value::as_str) == Some(external_id))
        .map(|(pid, _)| *pid)
}

/// The whole resolution, with the store lookup and the three process probes
/// injected.
///
/// `alive`, `tty` and `host` are `platform::process_alive`, `controlling_tty`
/// and `terminal_host_for_pid` in production and plain closures in tests.
/// `session_of` is a closure rather than a slice so the store is queried once,
/// by primary key, and only for the ROOT the parent walk actually lands on.
pub fn locate<S, A, T, H>(
    agents: &[RadarAgent],
    session_of: S,
    registry: &[(u32, Value)],
    agent_id: &str,
    alive: A,
    tty: T,
    host: H,
) -> Located
where
    S: Fn(&str) -> Option<Session>,
    A: Fn(u32) -> bool,
    T: Fn(u32) -> Option<String>,
    H: Fn(u32) -> TerminalHost,
{
    let Some(root) = root_of(agents, agent_id) else {
        return Located::blocked("that agent has left the radar");
    };
    // Named only when the window belongs to an ancestor, so the common case (a
    // root agent) gets a plain button with no extra sentence to read.
    let (via_id, via_label) = if root.id == agent_id {
        (None, None)
    } else {
        (
            Some(root.id.clone()),
            Some(root.label.clone()).filter(|l| !l.is_empty()),
        )
    };
    let attribute = |mut t: TerminalTarget| {
        t.via_agent_id = via_id.clone();
        t.via_label = via_label.clone();
        t
    };

    let Some(session) = session_of(&root.id) else {
        return Located {
            target: attribute(TerminalTarget::blocked(
                "no record of that session",
            )),
            focus: None,
        };
    };

    if !matches!(session.harness, Harness::ClaudeCode) {
        // Codex publishes no pid anywhere (the same gap that makes its liveness
        // time-based), so there is nothing to resolve a window from. Saying so
        // beats a button that fails on every click.
        return Located {
            target: attribute(TerminalTarget::blocked(
                "Codex publishes no process id to find its window by",
            )),
            focus: None,
        };
    }

    let Some(pid) = pid_for_session(registry, &session.external_id).filter(|p| alive(*p)) else {
        return Located {
            target: attribute(TerminalTarget::blocked("that session is not running")),
            focus: None,
        };
    };

    let host = host(pid);
    let Some(tty) = tty(pid) else {
        let reason = match &host {
            TerminalHost::Unscriptable(name) => {
                format!("no terminal window: it runs inside {name}")
            }
            _ => "no terminal window: it runs in an editor or a daemon".into(),
        };
        return Located {
            target: attribute(TerminalTarget::blocked(reason)),
            focus: None,
        };
    };

    match host {
        TerminalHost::Scriptable(app) => Located {
            target: attribute(TerminalTarget {
                reachable: true,
                app: Some(app.app_name().to_string()),
                via_agent_id: None,
                via_label: None,
                reason: None,
            }),
            focus: Some((app, tty)),
        },
        TerminalHost::Unscriptable(name) => Located {
            target: attribute(TerminalTarget::blocked(format!("WARDEN cannot address a tab in {name}"))),
            focus: None,
        },
        TerminalHost::None => Located {
            target: attribute(TerminalTarget::blocked(
                "no app owns that session: it is running detached",
            )),
            focus: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Harness, ProjectRef, Session};
    use crate::radar::{RadarComposition, RadarContextBreakdown, RadarExact};
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;

    fn agent(id: &str, parent: Option<&str>) -> RadarAgent {
        RadarAgent {
            id: id.into(),
            harness: "claude_code".into(),
            origin: None,
            parent_id: parent.map(Into::into),
            depth: if parent.is_some() { 1 } else { 0 },
            label: format!("{id} label"),
            nickname: None,
            cwd: None,
            repo: None,
            role: None,
            model: None,
            title: None,
            current_action: None,
            team: None,
            status: "working".into(),
            awaiting_reason: None,
            context_tokens: 0,
            max_tokens: 200_000,
            fill_pct: 0.0,
            context_breakdown: RadarContextBreakdown {
                used_tokens: 0,
                max_tokens: 200_000,
                fill_pct: 0.0,
                rows: vec![],
            },
            composition: RadarComposition {
                exact: RadarExact {
                    cache_read: 0,
                    fresh: 0,
                    cache_write: 0,
                    output: 0,
                },
                estimated: None,
            },
            recent_activity: vec![],
            child_count: 0,
            started_at: "2026-01-01T00:00:00Z".into(),
            est_cost_usd: None,
        }
    }

    fn session(id: &str, external: &str, harness: Harness) -> Session {
        Session {
            id: id.into(),
            harness,
            external_id: external.into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/work"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: Utc::now(),
            ended_at: None,
            source_path: PathBuf::from("/tmp/x.jsonl"),
            raw_hash: 0,
            ingested_at: Utc::now(),
            meta: json!({}),
        }
    }

    /// The whole happy path, with a real registry row shape.
    #[test]
    fn a_live_claude_session_in_terminal_is_reachable() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::ClaudeCode)];
        let registry = [(12046, json!({"pid":12046,"sessionId":"uuid-1","cwd":"/work"}))];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &registry,
            "s1",
            |_| true,
            |_| Some("/dev/ttys008".into()),
            |_| TerminalHost::Scriptable(TerminalApp::Apple),
        );

        assert!(out.target.reachable);
        assert_eq!(out.target.app.as_deref(), Some("Terminal"));
        assert_eq!(out.target.reason, None);
        assert_eq!(out.focus, Some((TerminalApp::Apple, "/dev/ttys008".into())));
    }

    /// A subagent has no process of its own, so the answer is the ROOT's window
    /// and it says whose window that is.
    #[test]
    fn a_subagent_resolves_to_its_roots_window_and_names_the_root() {
        let agents = [
            agent("root", None),
            agent("mid", Some("root")),
            agent("leaf", Some("mid")),
        ];
        let sessions = [
            session("root", "uuid-root", Harness::ClaudeCode),
            // The subagent's own row carries no registry entry, which is the
            // point: matching on it would report "not running".
            session("leaf", "uuid-leaf", Harness::ClaudeCode),
        ];
        let registry = [(900, json!({"pid":900,"sessionId":"uuid-root"}))];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &registry,
            "leaf",
            |_| true,
            |_| Some("/dev/ttys001".into()),
            |_| TerminalHost::Scriptable(TerminalApp::ITerm2),
        );

        assert!(out.target.reachable);
        assert_eq!(out.target.via_agent_id.as_deref(), Some("root"));
        assert_eq!(out.target.via_label.as_deref(), Some("root label"));
        assert_eq!(out.target.app.as_deref(), Some("iTerm2"));
    }

    /// A dead pid is the everyday case (the operator closed the window), and it
    /// must not read like a bug.
    #[test]
    fn a_dead_process_blocks_the_button_with_a_plain_reason() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::ClaudeCode)];
        let registry = [(1, json!({"pid":1,"sessionId":"uuid-1"}))];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &registry,
            "s1",
            |_| false,
            |_| Some("/dev/ttys001".into()),
            |_| TerminalHost::Scriptable(TerminalApp::Apple),
        );

        assert!(!out.target.reachable);
        assert!(out.focus.is_none());
        assert_eq!(
            out.target.reason.as_deref(),
            Some("that session is not running")
        );
    }

    /// No registry row at all is the same verdict as a dead pid: nothing to raise.
    #[test]
    fn an_unregistered_session_blocks_the_button() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::ClaudeCode)];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &[],
            "s1",
            |_| true,
            |_| Some("/dev/ttys001".into()),
            |_| TerminalHost::Scriptable(TerminalApp::Apple),
        );
        assert!(!out.target.reachable);
    }

    /// An IDE-hosted session has a live pid and no tty. The unscriptable host is
    /// NAMED, because "no terminal window" alone reads as a failure rather than
    /// as a fact about where the agent is running.
    #[test]
    fn an_ide_hosted_session_names_the_app_it_runs_in() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::ClaudeCode)];
        let registry = [(42, json!({"pid":42,"sessionId":"uuid-1"}))];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &registry,
            "s1",
            |_| true,
            |_| None,
            |_| TerminalHost::Unscriptable("Visual Studio Code".into()),
        );

        assert!(!out.target.reachable);
        assert_eq!(
            out.target.reason.as_deref(),
            Some("no terminal window: it runs inside Visual Studio Code")
        );
    }

    /// A terminal WARDEN cannot address (Ghostty, Warp) is a different answer
    /// from a missing window, and degrades without ever guessing an emulator.
    #[test]
    fn an_unscriptable_emulator_is_refused_by_name() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::ClaudeCode)];
        let registry = [(42, json!({"pid":42,"sessionId":"uuid-1"}))];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &registry,
            "s1",
            |_| true,
            |_| Some("/dev/ttys003".into()),
            |_| TerminalHost::Unscriptable("Ghostty".into()),
        );

        assert!(!out.target.reachable);
        assert!(
            out.target
                .reason
                .as_deref()
                .expect("a blocked target always states a reason")
                .contains("Ghostty")
        );
    }

    /// Codex has no pid signal at all. The reason names the gap rather than
    /// implying the session is gone.
    #[test]
    fn a_codex_session_says_why_it_cannot_be_located() {
        let agents = [agent("s1", None)];
        let sessions = [session("s1", "uuid-1", Harness::Codex)];

        let out = locate(
            &agents,
            |id| sessions.iter().find(|s| s.id == id).cloned(),
            &[],
            "s1",
            |_| true,
            |_| Some("/dev/ttys001".into()),
            |_| TerminalHost::Scriptable(TerminalApp::Apple),
        );

        assert!(!out.target.reachable);
        assert!(
            out.target
                .reason
                .as_deref()
                .expect("a blocked target always states a reason")
                .contains("process id")
        );
    }

    /// The panel can outlive a globe by one poll, so an unknown id is normal.
    #[test]
    fn an_agent_that_left_the_radar_is_not_an_error() {
        let out = locate(
            &[],
            |_| None,
            &[],
            "gone",
            |_| true,
            |_| None,
            |_| TerminalHost::None,
        );
        assert!(!out.target.reachable);
        assert_eq!(
            out.target.reason.as_deref(),
            Some("that agent has left the radar")
        );
    }

    /// A corrupt parent edge must not hang the walk.
    #[test]
    fn a_parent_cycle_terminates() {
        let mut a = agent("a", Some("b"));
        a.parent_id = Some("b".into());
        let mut b = agent("b", Some("a"));
        b.parent_id = Some("a".into());
        let agents = [a, b];
        assert!(root_of(&agents, "a").is_some());
    }

    /// The registry is matched on its `sessionId` field, never on the pid in the
    /// filename, so a rotated file cannot point at the wrong process.
    #[test]
    fn the_pid_comes_from_the_matching_session_id() {
        let registry = [
            (100, json!({"pid":100,"sessionId":"other"})),
            (200, json!({"pid":200,"sessionId":"wanted"})),
        ];
        assert_eq!(pid_for_session(&registry, "wanted"), Some(200));
        assert_eq!(pid_for_session(&registry, "missing"), None);
    }
}
