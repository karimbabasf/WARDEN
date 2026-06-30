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
