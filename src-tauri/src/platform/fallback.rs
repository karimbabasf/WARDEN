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

/// No app-level activation concept; on other window managers raising the window
/// already raises the app, so `set_focus` alone is the whole gesture.
pub fn activate_self() {}

/// No tty lookup yet. Reported as "no controlling terminal" rather than guessed,
/// so the panel says it cannot find the window instead of raising the wrong one.
pub fn controlling_tty(pid: u32) -> Option<String> {
    let _ = pid;
    None
}

/// No process-tree walk yet, so nothing is claimed about the hosting app.
pub fn terminal_host_for_pid(pid: u32) -> super::TerminalHost {
    let _ = pid;
    super::TerminalHost::None
}

/// No window-raising surface on this platform. `Unsupported` is a distinct arm
/// from a failure, so the UI can say "not on this OS" rather than "it broke".
pub fn focus_tty(
    app: super::TerminalApp,
    tty: &str,
) -> std::result::Result<(), super::AutomationError> {
    let _ = (app, tty);
    Err(super::AutomationError::Unsupported)
}

/// No per-app automation consent model to deep-link into.
pub fn open_automation_settings() {}

