//! macOS adapter for the platform seam (see `mod.rs`). Compiled only on macOS;
//! it is the one place that may use macOS-only Tauri APIs (`ActivationPolicy`,
//! `RunEvent::Reopen`) and the one place that speaks AppleScript.
//!
//! ## Why AppleScript is the delivery mechanism
//! Nothing local can tell a running Claude Code session to compact: the session
//! registry is a status file, not a control socket, and hooks observe or veto but
//! never trigger. The only thing that reaches an already-running interactive
//! session is the tty it is attached to, and the kernel refuses the classic route
//! (a `TIOCSTI` ioctl fails with EPERM on macOS 26 even against a pty the caller
//! just opened itself). Writing to `/dev/ttysNNN` directly is not an alternative
//! either: that writes to the terminal's OUTPUT stream and never enqueues into
//! the foreground process's input.
//!
//! What is left is the terminal emulator, which is scriptable by design.
//! `Terminal.sdef` exposes `do script ... in <tab>` plus a `tty` property per
//! tab, so a pid resolves to a tty resolves to a tab, and `do script` puts the
//! text plus a return into that tab exactly as if typed.

use super::{AutomationError, TerminalApp};
use std::process::Command;
use tauri::ActivationPolicy;

/// Show a Dock icon (`Regular`) so Minimize has a home and the window behaves
/// like a normal macOS app. The overlay is still created hidden and summoned via
/// the hotkey/tray; the daemon stays alive when the window is hidden.
pub fn apply_activation_policy(app: &mut tauri::App) {
    app.set_activation_policy(ActivationPolicy::Regular);
}

/// Clicking the Dock icon while the overlay is hidden emits `Reopen` — the
/// standard gesture for a window that closes-to-hide.
pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    matches!(event, tauri::RunEvent::Reopen { .. })
}

// ---------------------------------------------------------------------------
// Terminal automation: resolving a session's tty and its owning emulator, then
// typing into it. Used only by the armed-compaction path in `compact/`.
// ---------------------------------------------------------------------------

/// `ps -o tty=` for one pid. Returns `/dev/ttysNNN`, or `None` when the process
/// has no controlling terminal (`??`), which is exactly the IDE-hosted case.
pub fn controlling_tty(pid: u32) -> Option<String> {
    let out = Command::new("/bin/ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tty.is_empty() || tty == "??" || tty == "?" {
        return None;
    }
    Some(format!("/dev/{tty}"))
}

/// How far up the process tree to look for an emulator. A terminal session is
/// pid -> shell -> login -> emulator, so four is the real depth; twelve leaves
/// room for tmux-style intermediaries without ever looping.
const MAX_ANCESTRY_DEPTH: usize = 12;

/// Walk `pid`'s ancestry until a known emulator executable appears.
///
/// Deriving the emulator from the process tree rather than trying each installed
/// app in turn matters for permissions: a blind attempt against an app the
/// session does not belong to would raise a TCC consent dialog for a program the
/// user is not even using, and a refusal there is permanent.
pub fn terminal_app_for_pid(pid: u32) -> Option<TerminalApp> {
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        let out = Command::new("/bin/ps")
            .args(["-o", "ppid=,comm=", "-p", &current.to_string()])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (ppid_str, comm) = line.split_once(char::is_whitespace)?;
        if let Some(app) = terminal_app_from_comm(comm.trim()) {
            return Some(app);
        }
        let ppid: u32 = ppid_str.trim().parse().ok()?;
        if ppid <= 1 {
            return None;
        }
        current = ppid;
    }
    None
}

/// Classify one `ps -o comm=` executable path. Split out so the matching is unit
/// testable against the real strings `ps` prints on this machine.
pub(crate) fn terminal_app_from_comm(comm: &str) -> Option<TerminalApp> {
    if comm.ends_with("Terminal.app/Contents/MacOS/Terminal") {
        return Some(TerminalApp::Apple);
    }
    if comm.ends_with("iTerm.app/Contents/MacOS/iTerm2") || comm.ends_with("/iTerm2") {
        return Some(TerminalApp::ITerm2);
    }
    None
}

/// Escape a Rust string for embedding in an AppleScript string literal.
/// Backslash first, then quote, or the quote's own escape would be re-escaped.
pub(crate) fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The script that finds the tab or session owning `tty` and types `text` into
/// it. Returns `ok` on a hit and `notab` when nothing owns that tty, so a missed
/// target is distinguishable from a permission failure.
pub(crate) fn send_script(app: TerminalApp, tty: &str, text: &str) -> String {
    let tty = applescript_escape(tty);
    let text = applescript_escape(text);
    match app {
        // `do script ... in <tab>` writes the text plus a return into that tab's
        // tty. Without the `in` parameter it would open a NEW window instead,
        // which is why the tab lookup is not optional.
        TerminalApp::Apple => format!(
            r#"on run
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if (tty of t) is "{tty}" then
          do script "{text}" in t
          return "ok"
        end if
      end repeat
    end repeat
  end tell
  return "notab"
end run"#
        ),
        // iTerm2 addresses a SESSION rather than a tab, so a split pane resolves
        // exactly instead of landing in whichever pane happens to be focused.
        TerminalApp::ITerm2 => format!(
            r#"on run
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if (tty of s) is "{tty}" then
            tell s to write text "{text}"
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "notab"
end run"#
        ),
    }
}

/// Run one AppleScript and classify the outcome.
fn run_osascript(script: &str) -> Result<String, AutomationError> {
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| AutomationError::Failed(format!("osascript did not start: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Err(classify_osascript_error(&String::from_utf8_lossy(
        &out.stderr,
    )))
}

/// Map osascript's stderr to a typed failure.
///
/// The numeric codes are matched rather than the English text, because the text
/// is localised and the codes are not. `-1743` is the one that must never be
/// treated as retryable.
pub(crate) fn classify_osascript_error(stderr: &str) -> AutomationError {
    if stderr.contains("-1743") {
        return AutomationError::NotPermitted;
    }
    if stderr.contains("-600") {
        return AutomationError::TargetNotRunning;
    }
    if stderr.contains("-10004") {
        // errAEPrivilegeError: a privilege violation, same practical remedy as
        // an outright denial.
        return AutomationError::NotPermitted;
    }
    AutomationError::Failed(stderr.trim().to_string())
}

pub fn send_text_to_tty(
    app: TerminalApp,
    tty: &str,
    text: &str,
) -> Result<(), AutomationError> {
    match run_osascript(&send_script(app, tty, text))?.as_str() {
        "ok" => Ok(()),
        _ => Err(AutomationError::NoMatchingTab),
    }
}

/// Probe (and on the first call, PROMPT for) Automation access to `app`.
///
/// A property read is the cheapest event that still crosses the TCC boundary, so
/// consent is requested with no side effect on the terminal. Only ever called for
/// an emulator already proven to be running, so it cannot launch an app the user
/// did not open.
pub fn request_automation(app: TerminalApp) -> Result<(), AutomationError> {
    let name = app.app_name();
    run_osascript(&format!(
        r#"tell application "{name}" to get name of every window"#
    ))
    .map(|_| ())
}

/// Deep-link into Privacy and Security, Automation. A denial is sticky and never
/// re-prompts, so this pane is the only place the user can undo it.
pub fn open_automation_settings() {
    let _ = Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        .status();
}

/// Post a local notification through osascript's standard additions. Chosen over
/// a notification crate because it adds no dependency and no entitlement: this is
/// the fallback path, and it must not be the thing that fails.
pub fn post_notification(title: &str, body: &str) {
    let script = format!(
        r#"display notification "{}" with title "{}""#,
        applescript_escape(body),
        applescript_escape(title)
    );
    if let Err(e) = run_osascript(&script) {
        tracing::warn!(error=%e, "local notification failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `comm` strings below are verbatim `ps -o comm=` output captured while
    /// walking a real Claude session's ancestry on this machine.
    #[test]
    fn recognises_the_real_terminal_executable_path() {
        assert_eq!(
            terminal_app_from_comm("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
            Some(TerminalApp::Apple)
        );
        assert_eq!(
            terminal_app_from_comm("/Applications/iTerm.app/Contents/MacOS/iTerm2"),
            Some(TerminalApp::ITerm2)
        );
        // The intermediate hops of that same real walk must NOT match, or the
        // resolver would stop at the shell and address the wrong app.
        assert_eq!(terminal_app_from_comm("claude"), None);
        assert_eq!(terminal_app_from_comm("-zsh"), None);
        assert_eq!(terminal_app_from_comm("login"), None);
    }

    #[test]
    fn escapes_quotes_and_backslashes_before_they_reach_applescript() {
        assert_eq!(applescript_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(applescript_escape(r"a\b"), r"a\\b");
        // Backslash-then-quote must not double-escape the quote.
        assert_eq!(applescript_escape(r#"\""#), r#"\\\""#);
    }

    #[test]
    fn the_send_script_targets_the_tty_and_never_opens_a_new_window() {
        let s = send_script(TerminalApp::Apple, "/dev/ttys001", "/compact");
        assert!(s.contains(r#"if (tty of t) is "/dev/ttys001""#));
        // `in t` is what keeps `do script` inside the existing tab.
        assert!(s.contains(r#"do script "/compact" in t"#));
        assert!(s.contains(r#"return "notab""#));

        let i = send_script(TerminalApp::ITerm2, "/dev/ttys002", "/compact");
        assert!(i.contains("sessions of t"));
        assert!(i.contains(r#"tell s to write text "/compact""#));
    }

    #[test]
    fn a_sticky_permission_denial_is_classified_by_code_not_by_english() {
        assert_eq!(
            classify_osascript_error(
                "execution error: Not authorized to send Apple events to Terminal. (-1743)"
            ),
            AutomationError::NotPermitted
        );
        assert_eq!(
            classify_osascript_error("execution error: Application isn't running. (-600)"),
            AutomationError::TargetNotRunning
        );
        assert_eq!(
            classify_osascript_error("execution error: A privilege violation occurred. (-10004)"),
            AutomationError::NotPermitted
        );
        assert!(matches!(
            classify_osascript_error("syntax error: Expected end of line. (-2741)"),
            AutomationError::Failed(_)
        ));
    }
}
