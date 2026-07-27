//! Armed-compaction data types: the `compact_status` contract plus the small
//! enums the store, the idle tracker, and the delivery path share.
//!
//! A LEAF module (no dependency on the rest of `compact/`), so every sibling can
//! name these types without forming a cycle.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// How WARDEN will act when an armed agent goes idle.
///
/// The distinction is the whole honesty story of this feature: two of these
/// actually press the key, one of them only tells you it is time to press it
/// yourself. The UI must render the difference rather than hiding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    /// ACT: AppleScript `do script "/compact"` into the Terminal tab whose tty
    /// belongs to the session's pid. Needs macOS Automation (Apple Events).
    TerminalAppleScript,
    /// ACT: Codex app-server `thread/compact/start` over `codex app-server proxy`.
    /// Owner-only unix socket, no TCC permission involved.
    CodexAppServer,
    /// NOTIFY ONLY: no control channel exists for this harness (or Automation was
    /// denied). WARDEN still detects idle and posts a local notification; the user
    /// types `/compact`. The button stays, the label changes.
    NotifyOnly,
}

impl DeliveryMode {
    /// snake_case wire value for the `compact_status` contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryMode::TerminalAppleScript => "terminal_applescript",
            DeliveryMode::CodexAppServer => "codex_app_server",
            DeliveryMode::NotifyOnly => "notify_only",
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "terminal_applescript" => DeliveryMode::TerminalAppleScript,
            "codex_app_server" => DeliveryMode::CodexAppServer,
            _ => DeliveryMode::NotifyOnly,
        }
    }

    /// True when this mode actually delivers a compaction. `false` means WARDEN
    /// can only notify, which the frontend renders as a different button.
    pub fn acts(&self) -> bool {
        !matches!(self, DeliveryMode::NotifyOnly)
    }
}

/// Which signal will decide "this agent is done working".
///
/// These are not the same quality of evidence and the contract says so out loud:
/// the Claude registry writes on TRANSITION (an exact event), while the Codex
/// rollout only tells us how long ago the thread last wrote (a quiet window).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleSource {
    /// `~/.claude/sessions/<pid>.json` `status` moving to `idle`. Exact.
    RegistryTransition,
    /// A `~/.codex/sessions` rollout that has not been written for the quiet
    /// window and is not archived. Inferred, not observed.
    CodexQuietWindow,
}

impl IdleSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            IdleSource::RegistryTransition => "registry_transition",
            IdleSource::CodexQuietWindow => "codex_quiet_window",
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "codex_quiet_window" => IdleSource::CodexQuietWindow,
            _ => IdleSource::RegistryTransition,
        }
    }
}

/// Lifecycle of one armed record. Cancel DELETES the row, so there is no
/// `cancelled` state: a cancelled arm simply stops existing, which is what makes
/// cancel total rather than advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmState {
    /// Waiting for idle. Nothing has been sent to the agent.
    Armed,
    /// Idle fired and delivery is in flight. The only state with a race window.
    Firing,
    /// The compaction request was actually delivered.
    Delivered,
    /// Idle fired but WARDEN has no control channel, so it notified instead.
    Notified,
    /// Delivery was attempted and failed. `detail` carries the reason.
    Failed,
    /// The session went away before it ever went idle. Nothing was sent.
    Expired,
}

impl ArmState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArmState::Armed => "armed",
            ArmState::Firing => "firing",
            ArmState::Delivered => "delivered",
            ArmState::Notified => "notified",
            ArmState::Failed => "failed",
            ArmState::Expired => "expired",
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "firing" => ArmState::Firing,
            "delivered" => ArmState::Delivered,
            "notified" => ArmState::Notified,
            "failed" => ArmState::Failed,
            "expired" => ArmState::Expired,
            _ => ArmState::Armed,
        }
    }

    /// True while the record is still waiting on idle. Only these are considered
    /// by the watcher; a terminal record is history the UI can show and dismiss.
    pub fn pending(&self) -> bool {
        matches!(self, ArmState::Armed)
    }
}

/// One armed agent, as persisted and as rendered. This is WARDEN-side state in
/// its entirety: arming writes THIS and touches nothing else on the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmRecord {
    /// The `RadarAgent.id` the user clicked. For a Claude root agent this is also
    /// the harness session id; the two are kept separate so a future non-1:1
    /// harness does not have to lie about one of them.
    pub agent_id: String,
    pub session_id: String,
    /// `None` for a Codex thread, which WARDEN observes through rollout files and
    /// never through a process.
    pub pid: Option<u32>,
    /// `claude_code` or `codex`, matching the IR harness slug.
    pub harness: String,
    pub mode: DeliveryMode,
    /// Human-readable justification for `mode`, shown verbatim in the UI. For a
    /// notify-only arm this is the sentence that explains WHY WARDEN cannot act.
    pub mode_reason: String,
    pub idle_source: IdleSource,
    /// The agent's display name at arm time (registry `name`, or the radar label),
    /// so a notification can say which session it means.
    pub label: String,
    /// The status observed at arm time. Seeds the idle tracker so an agent that is
    /// ALREADY idle does not instantly fire: arming means "next time you finish".
    pub baseline_status: String,
    pub armed_at: DateTime<Utc>,
    pub state: ArmState,
    pub fired_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

/// Whether WARDEN may drive Terminal.app via Apple Events right now.
///
/// `Denied` is a PERMANENT state, not a retryable error: the TCC dialog does not
/// re-prompt once refused, so the only recovery is the user visiting System
/// Settings. Treating it as transient would produce a button that silently does
/// nothing forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationStatus {
    /// Never asked. The first arm that needs it will prompt.
    Unknown,
    Granted,
    /// `errAEEventNotPermitted` (-1743). Sticky until changed in System Settings.
    Denied,
    /// Terminal.app is not running (`procNotFound`, -600), so nothing to drive.
    TargetNotRunning,
    /// Not macOS: there is no Apple Events surface at all.
    Unsupported,
}

impl AutomationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutomationStatus::Unknown => "unknown",
            AutomationStatus::Granted => "granted",
            AutomationStatus::Denied => "denied",
            AutomationStatus::TargetNotRunning => "target_not_running",
            AutomationStatus::Unsupported => "unsupported",
        }
    }
}

/// One row of `compact_status()`, serialized camelCase for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArmedRow {
    pub agent_id: String,
    pub session_id: String,
    pub pid: Option<u32>,
    pub harness: String,
    /// `terminal_applescript` | `codex_app_server` | `notify_only`.
    pub mode: String,
    /// True when `mode` will really deliver. The frontend should label the button
    /// differently when this is false rather than hiding it.
    pub acts: bool,
    pub mode_reason: String,
    /// `registry_transition` | `codex_quiet_window`.
    pub idle_source: String,
    pub label: String,
    /// RFC3339 UTC.
    pub armed_at: String,
    /// `armed` | `firing` | `delivered` | `notified` | `failed` | `expired`.
    pub state: String,
    /// The status read from the harness on the last observation:
    /// `busy` | `idle` | `shell` | `unknown` | `gone`.
    pub last_status: String,
    /// `kill(pid, 0)` for a Claude session; for Codex, whether the rollout is
    /// still in the live sessions root. `null` when it cannot be determined.
    pub alive: Option<bool>,
    pub fired_at: Option<String>,
    pub detail: Option<String>,
}

/// The full `compact_status()` payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompactStatus {
    /// `unknown` | `granted` | `denied` | `target_not_running` | `unsupported`.
    pub automation: String,
    /// True when the frontend should offer the "open System Settings" affordance,
    /// which is exactly the sticky-denial case.
    pub automation_recoverable_in_settings: bool,
    pub armed: Vec<ArmedRow>,
}

/// Render one record plus its freshly observed liveness into a wire row.
pub fn to_row(rec: &ArmRecord, last_status: &str, alive: Option<bool>) -> ArmedRow {
    ArmedRow {
        agent_id: rec.agent_id.clone(),
        session_id: rec.session_id.clone(),
        pid: rec.pid,
        harness: rec.harness.clone(),
        mode: rec.mode.as_str().to_string(),
        acts: rec.mode.acts(),
        mode_reason: rec.mode_reason.clone(),
        idle_source: rec.idle_source.as_str().to_string(),
        label: rec.label.clone(),
        armed_at: rfc3339(rec.armed_at),
        state: rec.state.as_str().to_string(),
        last_status: last_status.to_string(),
        alive,
        fired_at: rec.fired_at.map(rfc3339),
        detail: rec.detail.clone(),
    }
}

pub(crate) fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_mode_round_trips_through_its_wire_value() {
        for m in [
            DeliveryMode::TerminalAppleScript,
            DeliveryMode::CodexAppServer,
            DeliveryMode::NotifyOnly,
        ] {
            assert_eq!(DeliveryMode::from_wire(m.as_str()), m);
        }
        // An unknown mode read back from an older DB must fail CLOSED: never
        // resurrect as something that presses keys.
        assert_eq!(DeliveryMode::from_wire("who_knows"), DeliveryMode::NotifyOnly);
        assert!(!DeliveryMode::NotifyOnly.acts());
        assert!(DeliveryMode::TerminalAppleScript.acts());
    }

    /// The `compact_status` wire contract, pinned.
    ///
    /// The frontend keys off these exact names, so a rename here is a breaking
    /// change to the UI and should have to break this test first.
    #[test]
    fn compact_status_serializes_to_the_documented_camel_case_shape() {
        let rec = ArmRecord {
            agent_id: "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            session_id: "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            pid: Some(71015),
            harness: "claude_code".to_string(),
            mode: DeliveryMode::TerminalAppleScript,
            mode_reason: "Terminal tab on /dev/ttys001".to_string(),
            idle_source: IdleSource::RegistryTransition,
            label: "machine-89".to_string(),
            baseline_status: "busy".to_string(),
            armed_at: DateTime::from_timestamp(1_785_183_890, 0).expect("valid"),
            state: ArmState::Armed,
            fired_at: None,
            detail: None,
        };
        let status = CompactStatus {
            automation: AutomationStatus::Granted.as_str().to_string(),
            automation_recoverable_in_settings: false,
            armed: vec![to_row(&rec, "busy", Some(true))],
        };
        let v = serde_json::to_value(&status).expect("serializes");

        assert_eq!(v["automation"], "granted");
        assert_eq!(v["automationRecoverableInSettings"], false);
        let row = &v["armed"][0];
        for key in [
            "agentId",
            "sessionId",
            "pid",
            "harness",
            "mode",
            "acts",
            "modeReason",
            "idleSource",
            "label",
            "armedAt",
            "state",
            "lastStatus",
            "alive",
            "firedAt",
            "detail",
        ] {
            assert!(row.get(key).is_some(), "missing contract key {key}");
        }
        assert_eq!(row["mode"], "terminal_applescript");
        assert_eq!(row["acts"], true);
        assert_eq!(row["idleSource"], "registry_transition");
        assert_eq!(row["state"], "armed");
        assert_eq!(row["armedAt"], "2026-07-27T20:24:50Z");
        assert!(row["firedAt"].is_null());
        assert_eq!(
            row.as_object().expect("object").len(),
            15,
            "a new field is a contract change: update the frontend with it"
        );
    }

    #[test]
    fn arm_state_round_trips_and_only_armed_is_pending() {
        for s in [
            ArmState::Armed,
            ArmState::Firing,
            ArmState::Delivered,
            ArmState::Notified,
            ArmState::Failed,
            ArmState::Expired,
        ] {
            assert_eq!(ArmState::from_wire(s.as_str()), s);
        }
        assert!(ArmState::Armed.pending());
        // Firing is NOT pending: the watcher must not fire the same record twice.
        assert!(!ArmState::Firing.pending());
        assert!(!ArmState::Delivered.pending());
    }
}
