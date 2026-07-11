//! Platform seam — the single place OS-specific runtime behavior lives.
//!
//! WARDEN ships for macOS today. Everything that is not portable is isolated
//! here so that bringing up Linux/Windows is "implement one adapter", not "hunt
//! `#[cfg]` blocks across the tree". The rest of the codebase calls these
//! functions and never branches on the OS itself.
//!
//! Ports & adapters:
//! * this file is the PORT — the stable interface the app calls;
//! * [`macos`](macos.rs)/[`fallback`](fallback.rs) are ADAPTERS, selected by
//!   `#[cfg]` below and aliased to `imp`;
//! * process liveness is split on the unix/windows axis (macOS is unix), so it
//!   lives here directly rather than in a per-OS adapter.
//!
//! ## Adding a platform (e.g. Linux or Windows)
//! 1. give `fallback.rs` a real `apply_activation_policy` / `is_reopen_event`
//!    for that OS, or add a dedicated `linux.rs` / `windows.rs` adapter and a
//!    `#[cfg]` arm below;
//! 2. extend [`process_alive`] with that OS's check (Windows needs
//!    `OpenProcess`/`GetExitCodeProcess`);
//! 3. add the bundle target + platform block in `tauri.conf.json`, and gate the
//!    macOS-only `macos-private-api` Cargo feature.

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
#[path = "fallback.rs"]
mod imp;

/// Apply the platform's preferred window activation policy at setup time.
/// macOS: `Regular` (a Dock icon, so Minimize has a home); other OSes: no-op.
pub fn apply_activation_policy(app: &mut tauri::App) {
    imp::apply_activation_policy(app);
}

/// True if `event` is the OS "reopen" gesture (macOS Dock-icon click on a
/// hidden window). The caller decides what to do with it (re-summon the
/// overlay). Always false on platforms without such a gesture.
pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    imp::is_reopen_event(event)
}

/// The global summon/dismiss chord. Currently ⌘⌥⌃M on every platform (the
/// `SUPER` modifier maps to Cmd on macOS, the Win/Super key elsewhere). Kept in
/// one place so a future platform can pick a more idiomatic chord.
pub fn primary_hotkey() -> Shortcut {
    Shortcut::new(
        Some(Modifiers::SUPER | Modifiers::ALT | Modifiers::CONTROL),
        Code::KeyM,
    )
}

/// True when `pid` names a live process. Split on the unix/windows axis:
/// * unix (macOS, Linux): `kill(pid, 0)` — probes existence/permission, sends
///   no signal;
/// * windows / other: TODO (Windows needs `OpenProcess`); assumes alive so the
///   RADAR liveness fallback degrades gracefully rather than dropping sessions.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 performs only error checking and never
        // delivers a signal; it cannot corrupt memory.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}
