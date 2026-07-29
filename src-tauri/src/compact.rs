//! Armed compaction: "when this agent is done working, compact it".
//!
//! The user clicks a button on an agent's context meter. From that moment WARDEN
//! watches for the agent to finish its turn and then sends `/compact` on their
//! behalf. Cancelling is instant and total.
//!
//! ## The one design decision everything else follows from
//! Arming is WARDEN-SIDE STATE AND NOTHING ELSE. The click writes a row in
//! WARDEN's own database ([`arm`]) and sends the agent nothing. The alternative,
//! pre-delivering a request and asking the agent to hold it until it finishes,
//! would make cancellation a negotiation with a process that may be mid-turn or
//! may have already acted. Keeping the intent local means cancel is a `DELETE`.
//!
//! ## Why the delivery mechanisms look so uneven
//! There is no local RPC that tells a running Claude Code session to compact. Its
//! session registry is a status file, not a control socket; its hooks observe and
//! veto but never trigger; `autoCompactEnabled` only permits a
//! threshold-triggered compaction rather than causing one; and the kernel refuses
//! `TIOCSTI`. The only thing that reaches a running interactive session is the
//! terminal it is attached to. Codex is the mirror image: a real JSON-RPC control
//! plane with a first-class `thread/compact/start`, but only for threads the
//! app-server daemon owns. So:
//!
//! | harness | channel |
//! |---|---|
//! | Claude Code in Terminal.app or iTerm2 | AppleScript into the tab owning its tty |
//! | Claude Code in an IDE panel | none: NOTIFY ONLY |
//! | Codex desktop or IDE thread | `thread/compact/start` over `codex app-server proxy` |
//! | Codex standalone TUI | none that WARDEN can address: NOTIFY ONLY |
//! | Cursor | none: NOTIFY ONLY |
//!
//! Where there is no channel the button does not disappear and does not pretend.
//! It switches from ACT to NOTIFY: WARDEN still detects idle and says so, and the
//! UI states which mode it is in. See [`deliver::claude_mode`].
//!
//! ## Where the module boundaries are
//! * [`model`] the wire contract plus the shared enums (leaf).
//! * [`registry`] the Claude session registry, read as a three-state signal.
//! * [`arm`] the armed set, the only code that touches `compact_arms`.
//! * [`deliver`] deciding the channel (pure) and using it (effects).
//! * [`watch`] the idle-transition detector (pure) and the watcher thread.

pub mod arm;
pub mod deliver;
pub mod model;
pub mod registry;
pub mod watch;

pub use model::{ArmedRow, AutomationStatus, CompactStatus, DeliveryMode, IdleSource};
pub use watch::{spawn, CompactWatcher, StatusSink};

use crate::ir::Harness;
use crate::platform::{self, AutomationError, TerminalApp};
use crate::store::Store;
use anyhow::{anyhow, Result};
use chrono::Utc;
use model::{ArmRecord, ArmState};
use once_cell::sync::Lazy;
use registry::SessionStatus;
use std::path::Path;
use std::sync::RwLock;

/// Cached Automation (Apple Events) permission.
///
/// Cached rather than probed per call because a DENIAL IS STICKY: macOS records
/// the refusal and never shows the dialog again, so re-probing cannot recover it
/// and would only add a subprocess per arm. The cache is populated on the first
/// arm that actually needs the permission, which is when the consent dialog has
/// a visible reason on screen.
static AUTOMATION: Lazy<RwLock<AutomationStatus>> =
    Lazy::new(|| RwLock::new(AutomationStatus::Unknown));

pub fn automation_status() -> AutomationStatus {
    AUTOMATION
        .read()
        .map(|g| *g)
        .unwrap_or(AutomationStatus::Unknown)
}

fn set_automation(status: AutomationStatus) {
    if let Ok(mut g) = AUTOMATION.write() {
        *g = status;
    }
}

/// Record a denial observed at delivery time, so every other armed terminal
/// session immediately re-reads as notify-only instead of queueing up a second
/// failure.
pub(crate) fn set_automation_denied() {
    set_automation(AutomationStatus::Denied);
}

/// Ask for Automation access if it has not been established yet.
///
/// Called at ARM time rather than at install, and only once the session has been
/// shown to live in a terminal WARDEN can drive, so the prompt never appears for
/// a user whose sessions could not be driven anyway.
fn ensure_automation(app: TerminalApp) -> AutomationStatus {
    let current = automation_status();
    if current != AutomationStatus::Unknown {
        return current;
    }
    let resolved = match platform::request_automation(app) {
        Ok(()) => AutomationStatus::Granted,
        Err(AutomationError::NotPermitted) => AutomationStatus::Denied,
        Err(AutomationError::TargetNotRunning) => AutomationStatus::TargetNotRunning,
        Err(AutomationError::Unsupported) => AutomationStatus::Unsupported,
        Err(e) => {
            tracing::warn!(error=%e, "automation probe failed");
            AutomationStatus::Unknown
        }
    };
    set_automation(resolved);
    resolved
}

/// Open System Settings at the pane where a sticky denial is undone.
pub fn reveal_automation_settings() {
    platform::open_automation_settings();
}

/// The machine facts arming needs, injected rather than called directly.
///
/// Two reasons, both load-bearing. It keeps [`resolve`] testable against real
/// registry fixtures without a live session, and it means a test can never raise
/// the Automation consent dialog or send an Apple Event as a side effect of
/// running the suite.
pub(crate) struct Probe<'a> {
    pub tty: &'a dyn Fn(u32) -> Option<String>,
    pub terminal: &'a dyn Fn(u32) -> Option<TerminalApp>,
    /// Resolves (and on the first call, requests) Automation access.
    pub automation: &'a dyn Fn(TerminalApp) -> AutomationStatus,
}

impl Probe<'static> {
    /// The real machine: `ps` for the tty and ancestry, and a real TCC request.
    fn real() -> Self {
        Probe {
            tty: &platform::controlling_tty,
            terminal: &platform::terminal_app_for_pid,
            automation: &ensure_automation,
        }
    }
}

/// Arm `agent_id`: persist the intent, and nothing else.
///
/// The returned row is exactly what `compact_status` would report for it, so the
/// frontend can render the new state without a second round trip. Errors are for
/// agents that cannot be armed at all (not running, or a subagent), which is
/// information the UI should show rather than a silent no-op.
pub fn arm_agent(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    agent_id: &str,
) -> Result<ArmedRow> {
    arm_with(
        store,
        sessions_dir,
        codex_sessions_dir,
        agent_id,
        &Probe::real(),
    )
}

pub(crate) fn arm_with(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    agent_id: &str,
    probe: &Probe<'_>,
) -> Result<ArmedRow> {
    let rec = resolve(store, sessions_dir, codex_sessions_dir, agent_id, probe)?;
    arm::put(store, &rec)?;
    let (last_status, alive) = observe_one(&rec, sessions_dir, codex_sessions_dir);
    Ok(model::to_row(&rec, last_status.as_str(), alive))
}

/// Disarm `agent_id`. Returns false when nothing was armed.
///
/// This is the whole cancel path: the row is deleted and no request was ever in
/// flight, so there is nothing to retract from the agent. If the fire has already
/// claimed the record the `DELETE` still removes it, and the outcome lands on a
/// row that no longer exists, which is the correct end state either way.
pub fn cancel_agent(store: &Store, agent_id: &str) -> Result<bool> {
    arm::delete(store, agent_id)
}

/// The armed set plus the permission state, freshly observed.
pub fn status(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
) -> Result<CompactStatus> {
    let automation = automation_status();
    let armed = arm::all(store)?
        .into_iter()
        .map(|rec| {
            let (last_status, alive) = observe_one(&rec, sessions_dir, codex_sessions_dir);
            model::to_row(&rec, last_status.as_str(), alive)
        })
        .collect();
    Ok(CompactStatus {
        automation: automation.as_str().to_string(),
        automation_recoverable_in_settings: automation == AutomationStatus::Denied,
        armed,
    })
}

/// Current status and liveness for one record, read from the world right now
/// rather than from whatever the watcher last saw.
fn observe_one(
    rec: &ArmRecord,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
) -> (SessionStatus, Option<bool>) {
    match rec.idle_source {
        IdleSource::RegistryTransition => {
            let status = registry::find(sessions_dir, &rec.session_id)
                .map(|e| e.status)
                .unwrap_or(SessionStatus::Gone);
            // The registry file is not a heartbeat, so liveness is the process
            // check and never the file's freshness.
            let alive = rec.pid.map(platform::process_alive);
            (status, alive)
        }
        IdleSource::CodexQuietWindow => {
            let status = watch::codex_status_now(&rec.session_id, codex_sessions_dir);
            (status, Some(status != SessionStatus::Gone))
        }
    }
}

/// Build the record for one agent id: which harness it is, whether a control
/// channel exists, and what its status is right now.
///
/// A live Claude session is identified by the registry, which is also the only
/// place a pid can come from. Everything else is resolved from the store.
fn resolve(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    agent_id: &str,
    probe: &Probe<'_>,
) -> Result<ArmRecord> {
    let now = Utc::now();

    if let Some(entry) = registry::find(sessions_dir, agent_id) {
        let tty = (probe.tty)(entry.pid);
        let terminal = (probe.terminal)(entry.pid);
        // Only ask for the permission when this session could actually use it:
        // the consent dialog should never appear for a session WARDEN could not
        // have driven anyway.
        let automation = match terminal {
            Some(app) if entry.entrypoint == "cli" => (probe.automation)(app),
            _ => automation_status(),
        };
        let (mode, mode_reason) =
            deliver::claude_mode(&entry.entrypoint, tty.as_deref(), terminal, automation);
        let label = if entry.name.is_empty() {
            short_id(agent_id)
        } else {
            entry.name.clone()
        };
        return Ok(ArmRecord {
            agent_id: agent_id.to_string(),
            session_id: entry.session_id,
            pid: Some(entry.pid),
            harness: Harness::ClaudeCode.as_str().to_string(),
            mode,
            mode_reason,
            idle_source: IdleSource::RegistryTransition,
            label,
            baseline_status: entry.status.as_str().to_string(),
            armed_at: now,
            state: ArmState::Armed,
            fired_at: None,
            detail: None,
        });
    }

    let session = store
        .sessions()?
        .into_iter()
        .find(|s| s.id == agent_id)
        .ok_or_else(|| anyhow!("no agent with id {agent_id}"))?;

    match session.harness {
        Harness::Codex => {
            let originator = session.meta.get("originator").and_then(|v| v.as_str());
            let (mode, mode_reason) = deliver::codex_mode(originator);
            // The app-server addresses a thread by its own id, which is the
            // rollout uuid the adapter stored as `external_id`.
            let thread_id = session.external_id.clone();
            let baseline = watch::codex_status_now(&thread_id, codex_sessions_dir);
            Ok(ArmRecord {
                agent_id: agent_id.to_string(),
                session_id: thread_id,
                pid: None,
                harness: Harness::Codex.as_str().to_string(),
                mode,
                mode_reason,
                idle_source: IdleSource::CodexQuietWindow,
                label: session
                    .meta
                    .get("agent_nickname")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&short_id(agent_id))
                    .to_string(),
                baseline_status: baseline.as_str().to_string(),
                armed_at: now,
                state: ArmState::Armed,
                fired_at: None,
                detail: None,
            })
        }
        // A Claude session the store knows but the registry does not is either a
        // subagent (no process of its own, so it cannot be compacted separately)
        // or a session that has already exited. Say which.
        Harness::ClaudeCode => {
            if store.parent_of(agent_id)?.is_some() {
                Err(anyhow!(
                    "this is a subagent and shares its parent's context: arm the parent session instead"
                ))
            } else {
                Err(anyhow!(
                    "that session is not running, so there is nothing to compact"
                ))
            }
        }
        other => Err(anyhow!(
            "{} sessions expose no compaction channel",
            other.as_str()
        )),
    }
}

/// First segment of a uuid, for a label when the harness gave us no name.
fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real registry line from `~/.claude/sessions` with the pid changed to one
    /// that is not running. The shape is the machine's, the pid is synthetic on
    /// purpose: the suite must never probe, signal, or address a live session.
    const REAL_IDLE: &str = r#"{"pid":990015,"sessionId":"aaaaaaaa-0000-4000-8000-000000000001","cwd":"/Users/dev","version":"2.1.220","kind":"interactive","entrypoint":"cli","name":"machine-89","status":"idle","statusUpdatedAt":1785183890237}"#;
    /// Same shape, but hosted in an IDE panel: the case with no control channel.
    const REAL_IDE: &str = r#"{"pid":990016,"sessionId":"11111111-2222-3333-4444-555555555555","cwd":"/Users/dev","version":"2.1.220","kind":"interactive","entrypoint":"claude-vscode","name":"machine-ide","status":"busy","statusUpdatedAt":1785183890237}"#;

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("990015.json"), REAL_IDLE).expect("write");
        std::fs::write(dir.path().join("990016.json"), REAL_IDE).expect("write");
        dir
    }

    /// A terminal session WARDEN can drive, with the permission already settled.
    /// No `ps`, no `osascript`, no consent dialog: the suite decides the machine.
    fn granted_probe() -> Probe<'static> {
        Probe {
            tty: &|_| Some("/dev/ttys001".to_string()),
            terminal: &|_| Some(TerminalApp::Apple),
            automation: &|_| AutomationStatus::Granted,
        }
    }

    fn arm(store: &Store, dir: &Path, codex: &Path, id: &str) -> Result<ArmedRow> {
        arm_with(store, dir, codex, id, &granted_probe())
    }

    #[test]
    fn arming_a_live_session_persists_it_and_sends_nothing() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");

        let row = arm(
            &store,
            dir.path(),
            codex.path(),
            "aaaaaaaa-0000-4000-8000-000000000001",
        )
        .expect("arm");

        assert_eq!(row.state, "armed");
        assert_eq!(row.harness, "claude_code");
        assert_eq!(row.pid, Some(990015));
        assert_eq!(row.label, "machine-89");
        assert_eq!(row.idle_source, "registry_transition");
        // The status read back is the live one, and the baseline it was seeded
        // with is what stops an already-idle session firing at once.
        assert_eq!(row.last_status, "idle");

        let stored = arm::get(&store, "aaaaaaaa-0000-4000-8000-000000000001")
            .expect("get")
            .expect("row");
        assert_eq!(stored.baseline_status, "idle");
    }

    /// The honest-degradation requirement at the command boundary: an IDE-hosted
    /// session still arms, and the row says plainly that WARDEN will only notify.
    #[test]
    fn an_ide_hosted_session_arms_as_notify_only_rather_than_being_refused() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");

        let row = arm(
            &store,
            dir.path(),
            codex.path(),
            "11111111-2222-3333-4444-555555555555",
        )
        .expect("arm");

        assert_eq!(row.mode, "notify_only");
        assert!(
            !row.acts,
            "the frontend must be able to see it will not act"
        );
        assert!(
            row.mode_reason.starts_with("notify only:"),
            "reason: {}",
            row.mode_reason
        );
        assert!(row.mode_reason.contains("claude-vscode"));
    }

    #[test]
    fn cancelling_removes_the_row_entirely() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let id = "aaaaaaaa-0000-4000-8000-000000000001";

        arm(&store, dir.path(), codex.path(), id).expect("arm");
        assert_eq!(
            status(&store, dir.path(), codex.path())
                .expect("status")
                .armed
                .len(),
            1
        );

        assert!(cancel_agent(&store, id).expect("cancel"));
        assert!(status(&store, dir.path(), codex.path())
            .expect("status")
            .armed
            .is_empty());
        assert!(!cancel_agent(&store, id).expect("cancel again"));
    }

    #[test]
    fn arming_an_unknown_agent_says_so_instead_of_silently_doing_nothing() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let err = arm(&store, dir.path(), codex.path(), "not-a-real-agent").expect_err("must fail");
        assert!(format!("{err:#}").contains("no agent with id"));
    }

    #[test]
    fn status_reports_a_session_that_has_since_exited_as_gone() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let id = "aaaaaaaa-0000-4000-8000-000000000001";
        arm(&store, dir.path(), codex.path(), id).expect("arm");

        std::fs::remove_file(dir.path().join("990015.json")).expect("remove");
        let st = status(&store, dir.path(), codex.path()).expect("status");
        assert_eq!(st.armed[0].last_status, "gone");
    }

    #[test]
    fn short_id_takes_the_first_uuid_segment() {
        assert_eq!(short_id("aaaaaaaa-0000-4000"), "aaaaaaaa");
        assert_eq!(short_id("plain"), "plain");
    }
}
