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
    let (last_status, alive) = observe_one(store, &rec, sessions_dir, codex_sessions_dir);
    Ok(model::to_row(&rec, last_status.as_str(), alive))
}

/// Resolve what arming `agent_id` WOULD do, without writing or sending anything.
///
/// Exists for `examples/compact_probe.rs`, which walks the real board and reports the
/// decision for every live agent. The bug this feature shipped with (the radar's store
/// id being looked up in a registry keyed by harness id) could not be seen from any
/// fixture, only from real ids, so the probe is part of how this path stays honest.
pub fn resolve_for_probe(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    agent_id: &str,
) -> Result<ArmRecord> {
    resolve(
        store,
        sessions_dir,
        codex_sessions_dir,
        agent_id,
        &Probe::real(),
    )
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
            let (last_status, alive) = observe_one(store, &rec, sessions_dir, codex_sessions_dir);
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
    store: &Store,
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
        IdleSource::TranscriptQuietWindow => {
            // The transcript path is not carried on the record (it can move), so it
            // is re-resolved from the store on every observation.
            let status = transcript_path(store, &rec.agent_id)
                .map(|p| watch::transcript_status_now(&p))
                .unwrap_or(SessionStatus::Gone);
            (status, Some(status != SessionStatus::Gone))
        }
    }
}

/// Where an armed agent's transcript lives right now, for the generic quiet
/// window. `None` once the store no longer knows the session.
fn transcript_path(store: &Store, agent_id: &str) -> Option<std::path::PathBuf> {
    store
        .sessions()
        .ok()?
        .into_iter()
        .find(|s| s.id == agent_id)
        .map(|s| s.source_path)
}

/// Walk `agent_id` up to the root session it belongs to.
///
/// A subagent has no process and no control channel of its own, so everything
/// about DELIVERY belongs to the root that spawned it; only the row's identity
/// stays with the globe that was clicked. The walk is bounded so a cycle in the
/// parent table cannot hang the arm command.
fn root_ancestor(store: &Store, agent_id: &str) -> Result<String> {
    let mut id = agent_id.to_string();
    for _ in 0..64 {
        match store.parent_of(&id)? {
            Some(parent) => id = parent,
            None => return Ok(id),
        }
    }
    Ok(id)
}

/// Build the record for one agent id: which harness it is, whether a control
/// channel exists, and what its status is right now.
///
/// THE ID TRANSLATION, which is the whole reason this is not a one-liner. The
/// radar hands us a `RadarAgent.id`, which is the STORE's session id (a hash),
/// while the Claude registry keys on the harness's own `sessionId` (a uuid). The
/// two are never equal, so looking the agent id up in the registry directly
/// matched nothing and EVERY live Claude session fell through to the
/// "not running" arm below. The store row's `external_id` is the bridge, and it
/// is resolved from the ROOT of the agent's tree so a subagent arms the session
/// that actually owns its context.
///
/// The direct registry lookup is kept as a fallback for callers that already
/// hold a harness session id (the tests, and any future non-radar caller).
fn resolve(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    agent_id: &str,
    probe: &Probe<'_>,
) -> Result<ArmRecord> {
    let now = Utc::now();

    let sessions = store.sessions()?;
    let session = sessions.iter().find(|s| s.id == agent_id);
    let root_id = match session {
        Some(_) => root_ancestor(store, agent_id)?,
        None => agent_id.to_string(),
    };
    let root = sessions.iter().find(|s| s.id == root_id);
    // The harness id to look the registry up by: the root row's external id when
    // the store knows this agent, else whatever the caller passed.
    let external = root.map(|s| s.external_id.as_str()).unwrap_or(agent_id);
    // True when the click landed on a subagent and the arm was redirected to its
    // root. The user is told this in `mode_reason` rather than being refused.
    let delegated = root.is_some() && root_id != agent_id;

    if let Some(entry) = registry::find(sessions_dir, external) {
        let tty = (probe.tty)(entry.pid);
        let terminal = (probe.terminal)(entry.pid);
        // Only ask for the permission when this session could actually use it:
        // the consent dialog should never appear for a session WARDEN could not
        // have driven anyway.
        let automation = match terminal {
            Some(app) if entry.entrypoint == "cli" => (probe.automation)(app),
            _ => automation_status(),
        };
        let (mode, channel_reason) =
            deliver::claude_mode(&entry.entrypoint, tty.as_deref(), terminal, automation);
        let parent_label = if entry.name.is_empty() {
            short_id(&entry.session_id)
        } else {
            entry.name.clone()
        };
        let mode_reason = if delegated {
            format!(
                "a subagent shares its parent's context, so this arms the parent session ({parent_label}) and compacts when the PARENT finishes: {channel_reason}"
            )
        } else {
            channel_reason
        };
        // The row is labelled with the globe that was clicked, so a delegated arm
        // still reads as belonging to the subagent it was armed from.
        let label = session
            .and_then(session_label)
            .unwrap_or_else(|| parent_label.clone());
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

    let session = session
        .cloned()
        .ok_or_else(|| anyhow!("no agent with id {agent_id}"))?;

    match &session.harness {
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
        // A Claude session whose ROOT has no registry entry has no live process:
        // the registry is written by the process itself, so its absence is the
        // exit. There is genuinely nothing to compact, and saying so is better
        // than arming something that can never fire.
        Harness::ClaudeCode => Err(anyhow!(
            "that session is not running, so there is nothing to compact"
        )),
        // Every other harness (Cursor, and anything a future adapter adds). WARDEN
        // has no control channel and no status file for these, but it does have the
        // transcript it is already tailing, so it can still watch the agent stop and
        // tell you. Refusing here was the wrong call: the button exists to say "tell
        // me when this is done", and that half always works.
        other => {
            let harness = other.as_str().to_string();
            let baseline = watch::transcript_status_now(&session.source_path);
            Ok(ArmRecord {
                agent_id: agent_id.to_string(),
                session_id: session.external_id.clone(),
                pid: None,
                harness: harness.clone(),
                mode: DeliveryMode::NotifyOnly,
                mode_reason: format!(
                    "notify only: WARDEN has no control channel into {harness}, so it watches the transcript and tells you the moment this agent stops"
                ),
                idle_source: IdleSource::TranscriptQuietWindow,
                label: session_label(&session).unwrap_or_else(|| short_id(agent_id)),
                baseline_status: baseline.as_str().to_string(),
                armed_at: now,
                state: ArmState::Armed,
                fired_at: None,
                detail: None,
            })
        }
    }
}

/// The display name a session carries, if it has one worth showing.
fn session_label(s: &crate::ir::Session) -> Option<String> {
    s.meta
        .get("agent_nickname")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
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

    /// Persist a session row shaped the way the real pipeline writes one: the id is
    /// the STORE's hash, the external id is the harness's own session id. That gap
    /// is the whole point of these fixtures.
    fn put_session(store: &Store, external_id: &str, harness: crate::ir::Harness) -> String {
        let source_path = std::path::PathBuf::from(format!("/tmp/warden-test/{external_id}.jsonl"));
        let id = crate::util::stable_id(&[
            harness.as_str(),
            external_id,
            &source_path.to_string_lossy(),
        ]);
        let session = crate::ir::Session {
            id: id.clone(),
            harness,
            external_id: external_id.to_string(),
            project: None,
            model_ids: vec![],
            started_at: Utc::now(),
            ended_at: None,
            source_path,
            raw_hash: 0,
            ingested_at: Utc::now(),
            meta: serde_json::json!({}),
        };
        store
            .upsert_session_batch(&session, &[], &[], 0)
            .expect("persist session");
        id
    }

    /// THE BUG THIS FEATURE SHIPPED WITH, pinned.
    ///
    /// The radar hands `compact_arm` a `RadarAgent.id`, which is the store's sha256
    /// session id. The Claude registry keys on the harness uuid. Looking the agent id
    /// up in the registry directly therefore matched nothing, so every live Claude
    /// session on the board answered "that session is not running" and the button was
    /// dead for the entire harness. Arming by the store id must resolve the registry
    /// entry through the row's `external_id`.
    #[test]
    fn arming_by_the_radar_agent_id_resolves_the_live_registry_session() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let agent_id = put_session(
            &store,
            "aaaaaaaa-0000-4000-8000-000000000001",
            Harness::ClaudeCode,
        );
        assert_ne!(
            agent_id, "aaaaaaaa-0000-4000-8000-000000000001",
            "precondition: the radar id is NOT the harness session id"
        );

        let row = arm(&store, dir.path(), codex.path(), &agent_id).expect("arm");

        assert_eq!(row.agent_id, agent_id, "the row belongs to the clicked globe");
        assert_eq!(
            row.session_id, "aaaaaaaa-0000-4000-8000-000000000001",
            "delivery targets the harness session"
        );
        assert_eq!(row.pid, Some(990015));
        assert_eq!(row.mode, "terminal_applescript");
        assert!(row.acts, "a terminal session must really compact");
        assert_eq!(row.idle_source, "registry_transition");
    }

    /// A subagent has no process of its own, so it used to be refused outright. It now
    /// arms against the session that owns its context, and the reason says so instead
    /// of the user being told to go find the parent themselves.
    #[test]
    fn arming_a_subagent_delegates_to_its_parent_instead_of_refusing() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let root = put_session(
            &store,
            "aaaaaaaa-0000-4000-8000-000000000001",
            Harness::ClaudeCode,
        );
        let child = put_session(&store, "agent-aExplore-c0ffee", Harness::ClaudeCode);
        store.link_child_session(&child, &root).expect("link");

        let row = arm(&store, dir.path(), codex.path(), &child).expect("arm");

        assert_eq!(row.agent_id, child);
        assert_eq!(
            row.session_id, "aaaaaaaa-0000-4000-8000-000000000001",
            "the parent is what actually gets compacted"
        );
        assert!(row.acts);
        assert!(
            row.mode_reason.contains("parent session"),
            "the delegation must be stated, not hidden: {}",
            row.mode_reason
        );
    }

    /// "Works for every agent on the board" includes the harnesses WARDEN has no
    /// channel into. Those arm as notify-only against their own transcript rather
    /// than erroring, because "tell me when this is done" always works.
    #[test]
    fn a_harness_with_no_control_channel_still_arms_as_notify_only() {
        let store = Store::memory().expect("store");
        let dir = fixture_dir();
        let codex = tempfile::tempdir().expect("tempdir");
        let agent_id = put_session(&store, "cursor-thread-1", Harness::Cursor);

        let row = arm(&store, dir.path(), codex.path(), &agent_id).expect("arm");

        assert_eq!(row.harness, "cursor");
        assert_eq!(row.mode, "notify_only");
        assert!(!row.acts);
        assert_eq!(row.idle_source, "transcript_quiet_window");
        assert!(
            row.mode_reason.starts_with("notify only:"),
            "reason: {}",
            row.mode_reason
        );
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
