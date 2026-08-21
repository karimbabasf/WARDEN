//! macOS adapter for the platform seam (see `mod.rs`). Compiled only on macOS;
//! it is the one place that may use macOS-only Tauri APIs (`ActivationPolicy`,
//! `RunEvent::Reopen`) and the one place that speaks AppleScript.
//!
//! ## Why AppleScript, and what it is allowed to do here
//! A session's process knows its controlling tty, and a scriptable emulator can
//! map a tty back to the tab that owns it. That lookup is the only reliable way
//! to answer "which window is this agent in": window titles lie (they follow the
//! foreground command), and there is no other index from a pid to a tab.
//!
//! This adapter previously also typed INTO that tab (`do script`) for the
//! armed-compaction feature, and that went out with it. What is here now reads
//! the process tree and RAISES a window. Nothing is written into another
//! program, so the read-only posture in CLAUDE.md stays unconditional: raising a
//! terminal is the same class of act as `reveal_path` opening Finder.

use super::{AutomationError, TerminalApp, TerminalHost};
use std::process::Command;
use tauri::ActivationPolicy;

/// Show a Dock icon (`Regular`) so Minimize has a home and the window behaves
/// like a normal macOS app. The overlay is still created hidden and summoned via
/// the hotkey/tray; the daemon stays alive when the window is hidden.
pub fn apply_activation_policy(app: &mut tauri::App) {
    app.set_activation_policy(ActivationPolicy::Regular);
}

/// Clicking the Dock icon while the overlay is hidden emits `Reopen`, the
/// standard gesture for a window that closes-to-hide.
pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    matches!(event, tauri::RunEvent::Reopen { .. })
}

/// Bring WARDEN itself to the front, as the ACTIVE application.
///
/// This is not the same act as focusing a window, and the difference is the whole
/// reason the function exists. `WebviewWindow::set_focus` ends in tao's
/// `makeKeyAndOrderFront` plus `activateIgnoringOtherApps:`. The first half orders the
/// window front WITHIN WARDEN; the second half is what is supposed to put WARDEN in
/// front of everything else, and Apple deprecated it in macOS 14 in favour of a
/// cooperative model that a background app cannot force. On macOS 14 and later it is
/// effectively ignored, so a window raised from the menu-bar HUD became key inside an
/// app that was still behind the user's terminal: the panel closed and the terminal
/// was simply revealed, which read as "it took me to the terminal instead".
///
/// `NSApplication.activate` is the supported replacement. Must run on the main thread
/// (callers use `AppHandle::run_on_main_thread`); off it, `MainThreadMarker::new`
/// returns `None` and this is a no-op rather than a crash.
pub fn activate_self() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("activate_self called off the main thread; ignoring");
        return;
    };
    NSApplication::sharedApplication(mtm).activate();
}

// ---------------------------------------------------------------------------
// Locating a session's window.
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

/// How far up the process tree to look for the hosting app. A terminal session is
/// pid -> shell -> login -> emulator, so four is the real depth; twelve leaves
/// room for wrappers and multiplexers without ever looping.
const MAX_ANCESTRY_DEPTH: usize = 12;

/// Walk `pid`'s ancestry until a GUI application bundle appears.
///
/// Deriving the host from the process tree rather than trying each installed app
/// in turn matters for permissions: a blind attempt against an app the session
/// does not belong to would raise a consent dialog for a program the user is not
/// even using, and a refusal there is permanent.
pub fn terminal_host_for_pid(pid: u32) -> TerminalHost {
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        let Ok(out) = Command::new("/bin/ps")
            .args(["-o", "ppid=,comm=", "-p", &current.to_string()])
            .output()
        else {
            return TerminalHost::None;
        };
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let Some((ppid_str, comm)) = line.split_once(char::is_whitespace) else {
            return TerminalHost::None;
        };
        if let Some(host) = host_from_comm(comm.trim()) {
            return host;
        }
        let Ok(ppid) = ppid_str.trim().parse::<u32>() else {
            return TerminalHost::None;
        };
        if ppid <= 1 {
            return TerminalHost::None;
        }
        current = ppid;
    }
    TerminalHost::None
}

/// Classify one `ps -o comm=` executable path. Split out so the matching is unit
/// testable against the real strings `ps` prints on this machine.
///
/// Returns `None` for "keep walking" and `Some(host)` for a verdict, so an
/// unrecognised GUI bundle STOPS the walk: the first app above the shell is the
/// one hosting the window, and continuing past it would eventually reach
/// something like `launchd` and report a window that does not exist.
pub(crate) fn host_from_comm(comm: &str) -> Option<TerminalHost> {
    if comm.ends_with("Terminal.app/Contents/MacOS/Terminal") {
        return Some(TerminalHost::Scriptable(TerminalApp::Apple));
    }
    if comm.ends_with("iTerm.app/Contents/MacOS/iTerm2") || comm.ends_with("/iTerm2") {
        return Some(TerminalHost::Scriptable(TerminalApp::ITerm2));
    }
    bundle_name(comm).map(TerminalHost::Unscriptable)
}

/// The display name of the `.app` bundle an executable path sits inside, e.g.
/// `Ghostty` for `/Applications/Ghostty.app/Contents/MacOS/ghostty`. `None` when
/// the path is not inside a bundle, which is every shell, wrapper and daemon on
/// the way up.
pub(crate) fn bundle_name(comm: &str) -> Option<String> {
    let idx = comm.find(".app/")?;
    let name = comm[..idx].rsplit('/').next()?;
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Escape a Rust string for embedding in an AppleScript string literal.
/// Backslash first, then quote, or the quote's own escape would be re-escaped.
pub(crate) fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The script that finds the tab or session owning `tty`, selects it, un-hides
/// its window and brings the app to the front. Returns `ok` on a hit and `notab`
/// when nothing owns that tty, so a closed window is distinguishable from a
/// permission failure.
pub(crate) fn focus_script(app: TerminalApp, tty: &str) -> String {
    let tty = applescript_escape(tty);
    match app {
        // A miniaturized window ignores `frontmost`, so it is restored first.
        // Otherwise "take me there" reports success against a window still in
        // the Dock, which reads as the button doing nothing.
        TerminalApp::Apple => format!(
            r#"on run
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if (tty of t) is "{tty}" then
          set selected of t to true
          if miniaturized of w then set miniaturized of w to false
          set frontmost of w to true
          activate
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
        // Its window, tab and session all answer `select`, innermost last so the
        // final selection is the pane itself.
        TerminalApp::ITerm2 => format!(
            r#"on run
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if (tty of s) is "{tty}" then
            tell w to select
            tell t to select
            tell s to select
            activate
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

/// Select the tab attached to `tty` and bring its window forward.
pub fn focus_tty(app: TerminalApp, tty: &str) -> Result<(), AutomationError> {
    match run_osascript(&focus_script(app, tty))?.as_str() {
        "ok" => Ok(()),
        _ => Err(AutomationError::NoMatchingTab),
    }
}

/// Deep-link into Privacy and Security, Automation. A denial is sticky and never
/// re-prompts, so this pane is the only place the user can undo it.
pub fn open_automation_settings() {
    let _ = Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `comm` strings below are verbatim `ps -o comm=` output captured while
    /// walking a real Claude session's ancestry on this machine.
    #[test]
    fn recognises_the_real_terminal_executable_path() {
        assert_eq!(
            host_from_comm("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
            Some(TerminalHost::Scriptable(TerminalApp::Apple))
        );
        assert_eq!(
            host_from_comm("/Applications/iTerm.app/Contents/MacOS/iTerm2"),
            Some(TerminalHost::Scriptable(TerminalApp::ITerm2))
        );
        // The intermediate hops of that same real walk must NOT match, or the
        // walk would stop at the shell and address the wrong app.
        assert_eq!(host_from_comm("claude"), None);
        assert_eq!(host_from_comm("-zsh"), None);
        assert_eq!(host_from_comm("login"), None);
    }

    #[test]
    fn an_unknown_emulator_is_named_rather_than_ignored() {
        assert_eq!(
            host_from_comm("/Applications/Ghostty.app/Contents/MacOS/ghostty"),
            Some(TerminalHost::Unscriptable("Ghostty".into()))
        );
        assert_eq!(
            host_from_comm("/Applications/Visual Studio Code.app/Contents/MacOS/Electron"),
            Some(TerminalHost::Unscriptable("Visual Studio Code".into()))
        );
        assert_eq!(bundle_name("/usr/bin/login"), None);
    }

    #[test]
    fn escapes_quotes_and_backslashes_before_they_reach_applescript() {
        assert_eq!(applescript_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(applescript_escape(r"a\b"), r"a\\b");
        // Backslash-then-quote must not double-escape the quote.
        assert_eq!(applescript_escape(r#"\""#), r#"\\\""#);
    }

    #[test]
    fn the_focus_script_raises_the_matching_tab_and_types_nothing() {
        let s = focus_script(TerminalApp::Apple, "/dev/ttys001");
        assert!(s.contains(r#"if (tty of t) is "/dev/ttys001""#));
        assert!(s.contains("set selected of t to true"));
        assert!(s.contains("set miniaturized of w to false"));
        assert!(s.contains("activate"));
        assert!(s.contains(r#"return "notab""#));

        let i = focus_script(TerminalApp::ITerm2, "/dev/ttys002");
        assert!(i.contains("sessions of t"));
        assert!(i.contains("tell s to select"));

        // The read-only invariant, asserted on the generated script itself: no
        // AppleScript verb here may put text into another program.
        for script in [s, i] {
            assert!(!script.contains("do script"));
            assert!(!script.contains("write text"));
            assert!(!script.contains("keystroke"));
        }
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
