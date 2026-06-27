//! macOS adapter for the platform seam (see `mod.rs`). Compiled only on macOS;
//! it is the one place that may use macOS-only Tauri APIs (`ActivationPolicy`,
//! `RunEvent::Reopen`).

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
