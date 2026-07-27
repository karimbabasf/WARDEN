//! Deciding HOW a compaction would reach an agent, and then sending it.
//!
//! Two real control channels exist and they are mirror images of each other:
//!
//! * Claude Code has no local RPC at all, so a TERMINAL session is reachable
//!   (AppleScript into the tab that owns its tty) and an IDE-hosted one is not
//!   (the VS Code extension contributes 22 commands, none of which run a slash
//!   command or compact).
//! * Codex is the opposite: a thread owned by the app-server daemon (desktop app
//!   or IDE extension) has a first-class `thread/compact/start` method, while a
//!   standalone `codex` TUI is not an app-server thread and falls back to the
//!   same AppleScript path.
//!
//! Where neither applies the answer is NOT to hide the feature. `resolve_*`
//! returns `NotifyOnly` plus a sentence saying why, and that sentence is rendered
//! verbatim in the UI. A button that quietly does nothing is worse than a button
//! that says what it cannot do.
//!
//! Everything that decides is a pure function; everything that acts is behind the
//! platform seam or a subprocess, and both are skippable with `WARDEN_COMPACT_DRY_RUN`.

use super::model::{ArmRecord, ArmState, AutomationStatus, DeliveryMode};
use crate::platform::{self, AutomationError, TerminalApp};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The text typed into an armed session. A literal slash command, nothing else.
pub const COMPACT_COMMAND: &str = "/compact";

/// Decide how `/compact` would reach a Claude Code session (pure).
///
/// Returns the mode plus the human sentence that justifies it. The order of the
/// checks is deliberate: the reason shown should be the FIRST thing that blocks,
/// so an IDE session is told about its harness rather than about a permission it
/// would never have needed.
pub fn claude_mode(
    entrypoint: &str,
    tty: Option<&str>,
    terminal: Option<TerminalApp>,
    automation: AutomationStatus,
) -> (DeliveryMode, String) {
    if entrypoint != "cli" {
        let ep = if entrypoint.is_empty() {
            "unknown"
        } else {
            entrypoint
        };
        return (
            DeliveryMode::NotifyOnly,
            format!(
                "notify only: no control channel for a {ep} session (an IDE-hosted Claude Code panel exposes no compaction command)"
            ),
        );
    }
    let Some(tty) = tty else {
        return (
            DeliveryMode::NotifyOnly,
            "notify only: this session has no controlling terminal to type into".to_string(),
        );
    };
    let Some(terminal) = terminal else {
        return (
            DeliveryMode::NotifyOnly,
            "notify only: the session's terminal is not one WARDEN can drive (Terminal.app or iTerm2)"
                .to_string(),
        );
    };
    match automation {
        AutomationStatus::Denied => (
            DeliveryMode::NotifyOnly,
            "notify only: Automation permission was denied, so WARDEN cannot press the key. Re-enable it under Privacy and Security, Automation"
                .to_string(),
        ),
        AutomationStatus::Unsupported => (
            DeliveryMode::NotifyOnly,
            "notify only: this platform has no terminal automation surface".to_string(),
        ),
        _ => (
            DeliveryMode::TerminalAppleScript,
            format!("{} tab on {}", terminal.app_name(), tty),
        ),
    }
}

/// Decide how a compaction would reach a Codex thread (pure).
///
/// `originator` is the harness's own word for where the thread came from, carried
/// verbatim on session meta. The real values on this machine are `Codex Desktop`
/// and `codex_vscode`, both app-server owned; a terminal TUI writes
/// `codex_cli_rs` and is NOT reachable this way.
pub fn codex_mode(originator: Option<&str>) -> (DeliveryMode, String) {
    let Some(origin) = originator else {
        return (
            DeliveryMode::NotifyOnly,
            "notify only: this thread records no originator, so WARDEN cannot tell whether the app-server owns it"
                .to_string(),
        );
    };
    let lower = origin.to_ascii_lowercase();
    let app_server_owned =
        lower.contains("desktop") || lower.contains("vscode") || lower.contains("ide");
    if app_server_owned {
        (
            DeliveryMode::CodexAppServer,
            format!("codex app-server thread/compact/start ({origin})"),
        )
    } else {
        (
            DeliveryMode::NotifyOnly,
            format!(
                "notify only: {origin} is a standalone TUI, not an app-server thread, and WARDEN tracks no pid for it"
            ),
        )
    }
}

/// True when sends are suppressed. Set `WARDEN_COMPACT_DRY_RUN=1` to exercise the
/// whole arm/idle/fire path against real sessions without typing into any of them.
pub fn dry_run() -> bool {
    matches!(
        std::env::var("WARDEN_COMPACT_DRY_RUN").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// How long to wait for the app-server to answer `thread/compact/start`.
fn codex_timeout() -> Duration {
    Duration::from_millis(
        std::env::var("WARDEN_CODEX_RPC_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10_000),
    )
}

/// Locate the `codex` binary. It ships INSIDE ChatGPT.app and is not on PATH
/// there, so the bundle path is a first-class fallback rather than a guess.
pub fn codex_binary() -> Option<String> {
    if let Ok(explicit) = std::env::var("WARDEN_CODEX_BIN") {
        if !explicit.is_empty() {
            return Some(explicit);
        }
    }
    const BUNDLED: &str = "/Applications/ChatGPT.app/Contents/Resources/codex";
    if std::path::Path::new(BUNDLED).exists() {
        return Some(BUNDLED.to_string());
    }
    let out = Command::new("/usr/bin/which").arg("codex").output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Request ids for the two-step app-server handshake. The protocol requires an
/// `initialize` request and an `initialized` notification before any other
/// method, so the compaction is always request 2.
const REQ_INITIALIZE: u64 = 1;
const REQ_COMPACT: u64 = 2;

/// Build the three newline-delimited messages of a compaction exchange.
///
/// Shapes come from the app-server's own generated JSON Schema
/// (`codex app-server generate-json-schema`): `InitializeParams` requires
/// `clientInfo{name,version}`, `ClientNotification` has exactly one member
/// (`initialized`), and `ThreadCompactStartParams` requires `threadId`. The
/// envelope is `{id, method, params}` with no `jsonrpc` version tag.
pub fn codex_compact_messages(thread_id: &str) -> Vec<String> {
    vec![
        serde_json::json!({
            "id": REQ_INITIALIZE,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "WARDEN", "version": env!("CARGO_PKG_VERSION") }
            }
        })
        .to_string(),
        serde_json::json!({ "method": "initialized" }).to_string(),
        serde_json::json!({
            "id": REQ_COMPACT,
            "method": "thread/compact/start",
            "params": { "threadId": thread_id }
        })
        .to_string(),
    ]
}

/// Classify one app-server stdout line: `Some(Ok(()))` when it is the answer to
/// our compaction request, `Some(Err(msg))` when that answer is an error, `None`
/// for the stream of unrelated notifications the daemon interleaves.
pub fn codex_response_outcome(line: &str) -> Option<Result<(), String>> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("id").and_then(serde_json::Value::as_u64) != Some(REQ_COMPACT) {
        return None;
    }
    match v.get("error") {
        Some(e) => Some(Err(e.to_string())),
        None => Some(Ok(())),
    }
}

/// Send `thread/compact/start` for `thread_id` over `codex app-server proxy`.
///
/// `proxy` is the supported way to speak the protocol over stdio without
/// reimplementing the control-socket handshake. The blocking read runs on its own
/// thread behind a `recv_timeout` so a daemon that never answers cannot wedge the
/// watcher thread forever.
fn codex_compact(thread_id: &str) -> anyhow::Result<()> {
    let bin = codex_binary()
        .ok_or_else(|| anyhow::anyhow!("codex binary not found (set WARDEN_CODEX_BIN)"))?;
    let mut child = Command::new(&bin)
        .args(["app-server", "proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn {bin} app-server proxy: {e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("app-server stdin unavailable"))?;
        for msg in codex_compact_messages(thread_id) {
            writeln!(stdin, "{msg}")?;
        }
        stdin.flush()?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("app-server stdout unavailable"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(outcome) = codex_response_outcome(&line) {
                let _ = tx.send(outcome);
                return;
            }
        }
        let _ = tx.send(Err("app-server closed without answering".to_string()));
    });

    let outcome = rx.recv_timeout(codex_timeout());
    // The proxy is a one-shot stdio conduit for this exchange; the compaction
    // itself runs in the daemon, so closing the conduit after the answer arrives
    // does not cancel it.
    let _ = child.kill();
    let _ = child.wait();
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(anyhow::anyhow!("app-server refused the compaction: {e}")),
        Err(_) => Err(anyhow::anyhow!(
            "app-server did not answer within the timeout"
        )),
    }
}

/// What actually happened when an armed record fired. Kept separate from the
/// stored `ArmState` so the caller does the persisting in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: ArmState,
    pub detail: Option<String>,
    /// True when the Automation denial was observed here, so the caller can flip
    /// the cached permission status and stop offering ACT to other sessions.
    pub automation_denied: bool,
}

impl Outcome {
    fn new(state: ArmState, detail: Option<String>) -> Self {
        Outcome {
            state,
            detail,
            automation_denied: false,
        }
    }
}

/// Deliver the compaction described by `rec`, or notify when it cannot be
/// delivered.
///
/// Honest reporting is the contract: `Delivered` is returned ONLY when the send
/// really landed. A dry run reports `Notified` with the reason spelled out,
/// because in a dry run WARDEN genuinely did not press the key.
pub fn fire(rec: &ArmRecord) -> Outcome {
    let notify = |body: String| {
        platform::post_notification(&format!("{} is idle", rec.label), &body);
    };

    match rec.mode {
        DeliveryMode::NotifyOnly => {
            notify(format!(
                "{}. Type /compact when you are ready.",
                rec.mode_reason
            ));
            Outcome::new(ArmState::Notified, Some(rec.mode_reason.clone()))
        }
        DeliveryMode::TerminalAppleScript => {
            let Some(pid) = rec.pid else {
                return Outcome::new(
                    ArmState::Failed,
                    Some("no pid recorded for this session".to_string()),
                );
            };
            let (Some(tty), Some(app)) = (
                platform::controlling_tty(pid),
                platform::terminal_app_for_pid(pid),
            ) else {
                notify("the session's terminal could not be resolved at fire time".to_string());
                return Outcome::new(
                    ArmState::Notified,
                    Some("the terminal that owned this session is gone".to_string()),
                );
            };
            if dry_run() {
                tracing::info!(
                    pid,
                    tty,
                    app = app.app_name(),
                    "dry run: would send /compact"
                );
                notify(format!("dry run: would send /compact to {tty}"));
                return Outcome::new(
                    ArmState::Notified,
                    Some(format!(
                        "dry run (WARDEN_COMPACT_DRY_RUN): /compact was NOT sent to {tty}"
                    )),
                );
            }
            match platform::send_text_to_tty(app, &tty, COMPACT_COMMAND) {
                Ok(()) => Outcome::new(
                    ArmState::Delivered,
                    Some(format!("/compact typed into {tty}")),
                ),
                Err(AutomationError::NotPermitted) => {
                    notify(
                        "WARDEN is not permitted to control the terminal. Type /compact yourself, and re-enable it under Privacy and Security, Automation."
                            .to_string(),
                    );
                    Outcome {
                        state: ArmState::Notified,
                        detail: Some(AutomationError::NotPermitted.to_string()),
                        automation_denied: true,
                    }
                }
                Err(e) => {
                    notify(format!("could not send /compact: {e}"));
                    Outcome::new(ArmState::Failed, Some(e.to_string()))
                }
            }
        }
        DeliveryMode::CodexAppServer => {
            if dry_run() {
                tracing::info!(
                    thread = %rec.session_id,
                    "dry run: would call thread/compact/start"
                );
                return Outcome::new(
                    ArmState::Notified,
                    Some(
                        "dry run (WARDEN_COMPACT_DRY_RUN): thread/compact/start was NOT called"
                            .to_string(),
                    ),
                );
            }
            match codex_compact(&rec.session_id) {
                Ok(()) => Outcome::new(
                    ArmState::Delivered,
                    Some("thread/compact/start accepted".to_string()),
                ),
                Err(e) => {
                    let msg = format!("{e:#}");
                    notify(format!("could not compact this thread: {msg}"));
                    Outcome::new(ArmState::Failed, Some(msg))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_cli_session_with_a_tty_can_be_acted_on() {
        let (mode, why) = claude_mode(
            "cli",
            Some("/dev/ttys001"),
            Some(TerminalApp::Apple),
            AutomationStatus::Granted,
        );
        assert_eq!(mode, DeliveryMode::TerminalAppleScript);
        assert!(why.contains("/dev/ttys001"), "reason names the tty: {why}");
        assert!(why.contains("Terminal"));
    }

    /// The honest-degradation requirement, checked at its four real causes. Each
    /// one must produce notify-only AND a reason a human can act on.
    #[test]
    fn every_dead_end_degrades_to_notify_only_with_a_stated_reason() {
        let cases = [
            // Claude Code inside an IDE panel: the extension has no such command.
            claude_mode(
                "claude-vscode",
                Some("/dev/ttys001"),
                Some(TerminalApp::Apple),
                AutomationStatus::Granted,
            ),
            // A session with no controlling terminal.
            claude_mode("cli", None, Some(TerminalApp::Apple), AutomationStatus::Granted),
            // A terminal WARDEN cannot script.
            claude_mode("cli", Some("/dev/ttys001"), None, AutomationStatus::Granted),
            // Automation refused: sticky, so this is a permanent mode, not a retry.
            claude_mode(
                "cli",
                Some("/dev/ttys001"),
                Some(TerminalApp::Apple),
                AutomationStatus::Denied,
            ),
        ];
        for (mode, why) in cases {
            assert_eq!(mode, DeliveryMode::NotifyOnly);
            assert!(
                why.starts_with("notify only:"),
                "the UI label must say notify only: {why}"
            );
            assert!(why.len() > 30, "the reason must be a sentence: {why}");
        }
    }

    #[test]
    fn the_denied_reason_points_at_the_settings_pane_that_undoes_it() {
        let (_, why) = claude_mode(
            "cli",
            Some("/dev/ttys001"),
            Some(TerminalApp::Apple),
            AutomationStatus::Denied,
        );
        assert!(why.contains("Privacy and Security, Automation"), "{why}");
    }

    /// Both originator strings are real values read out of this machine's Codex
    /// rollouts; `codex_cli_rs` is the standalone TUI that has no app-server.
    #[test]
    fn codex_app_server_threads_act_and_the_standalone_tui_does_not() {
        assert_eq!(
            codex_mode(Some("Codex Desktop")).0,
            DeliveryMode::CodexAppServer
        );
        assert_eq!(
            codex_mode(Some("codex_vscode")).0,
            DeliveryMode::CodexAppServer
        );
        let (mode, why) = codex_mode(Some("codex_cli_rs"));
        assert_eq!(mode, DeliveryMode::NotifyOnly);
        assert!(why.contains("codex_cli_rs"), "{why}");
        assert_eq!(codex_mode(None).0, DeliveryMode::NotifyOnly);
    }

    /// The wire shapes are generated from the app-server's own schema, so this
    /// test is what catches a drift between that schema and what we send.
    #[test]
    fn the_app_server_exchange_is_initialize_then_initialized_then_compact() {
        let msgs = codex_compact_messages("019873f0-dead-beef-0000-000000000001");
        assert_eq!(msgs.len(), 3);

        let init: serde_json::Value = serde_json::from_str(&msgs[0]).expect("valid JSON");
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["clientInfo"]["name"], "WARDEN");
        assert!(init["params"]["clientInfo"]["version"].is_string());

        let ready: serde_json::Value = serde_json::from_str(&msgs[1]).expect("valid JSON");
        assert_eq!(ready["method"], "initialized");
        assert!(ready.get("id").is_none(), "a notification carries no id");

        let compact: serde_json::Value = serde_json::from_str(&msgs[2]).expect("valid JSON");
        assert_eq!(compact["method"], "thread/compact/start");
        assert_eq!(
            compact["params"]["threadId"],
            "019873f0-dead-beef-0000-000000000001"
        );
        assert_eq!(compact["id"], 2);
    }

    #[test]
    fn only_the_answer_to_our_own_request_is_treated_as_the_outcome() {
        // Unrelated notifications stream past constantly and must be ignored.
        assert!(codex_response_outcome(r#"{"method":"thread/compacted"}"#).is_none());
        assert!(codex_response_outcome(r#"{"id":1,"result":{}}"#).is_none());
        assert!(codex_response_outcome("not json at all").is_none());

        assert_eq!(codex_response_outcome(r#"{"id":2,"result":{}}"#), Some(Ok(())));
        let err = codex_response_outcome(r#"{"id":2,"error":{"code":-32602,"message":"no thread"}}"#);
        assert!(matches!(err, Some(Err(m)) if m.contains("no thread")));
    }
}
