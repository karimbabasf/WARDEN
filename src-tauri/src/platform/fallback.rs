//! Default adapter for the platform seam (see `mod.rs`) — compiled on every
//! non-macOS target. These are deliberate no-ops: WARDEN's window/Dock UX is
//! macOS-shaped today. Replace them (or add a dedicated `linux.rs`/`windows.rs`
//! adapter and a `#[cfg]` arm in `mod.rs`) when bringing up another platform.

/// No Dock/activation-policy concept to apply; the window manager decides.
pub fn apply_activation_policy(app: &mut tauri::App) {
    let _ = app;
}

/// No Dock-style "reopen" gesture; the tray menu and global hotkey re-summon.
pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    let _ = event;
    false
}

// ---------------------------------------------------------------------------
// Terminal automation. Every function here reports "no channel" rather than
// pretending, which is what makes the armed-compaction feature degrade to
// notify-only on this platform instead of silently doing nothing.
// ---------------------------------------------------------------------------

use super::{AutomationError, TerminalApp};

/// No portable way to ask for a pid's controlling terminal here yet.
pub fn controlling_tty(pid: u32) -> Option<String> {
    let _ = pid;
    None
}

/// No emulator resolution, so the caller degrades to notify-only.
pub fn terminal_app_for_pid(pid: u32) -> Option<TerminalApp> {
    let _ = pid;
    None
}

pub fn send_text_to_tty(
    app: TerminalApp,
    tty: &str,
    text: &str,
) -> Result<(), AutomationError> {
    let _ = (app, tty, text);
    Err(AutomationError::Unsupported)
}

pub fn request_automation(app: TerminalApp) -> Result<(), AutomationError> {
    let _ = app;
    Err(AutomationError::Unsupported)
}

/// No Automation pane to open.
pub fn open_automation_settings() {}

/// Logged rather than shown: better a line in the journal than a silent drop,
/// and it keeps the notify-only path from being the thing that panics.
pub fn post_notification(title: &str, body: &str) {
    tracing::info!(title, body, "notification (no platform notifier)");
}
