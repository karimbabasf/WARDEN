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

/// Write `bytes` to `path` readable only by the current user.
///
/// Split on the unix/windows axis, same as [`process_alive`]:
/// * unix (macOS, Linux): created with mode 0600, so the permission is set by the `open`
///   itself. Writing first and chmod-ing after would leave the file world-readable for the
///   window in between, which for a private key is the whole risk;
/// * windows / other: a plain write, with the ACL left to the parent directory. Flagged
///   rather than silently accepted, because the caller stores a private key here.
pub fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)
}

/// A terminal emulator WARDEN knows how to drive, resolved from a session's
/// process ancestry rather than guessed. Both are AppleScript targets on macOS;
/// on any other platform nothing resolves and the caller degrades to notify-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalApp {
    /// `Terminal.app`, driven with `do script "..." in <tab>`.
    Apple,
    /// `iTerm2`, driven with `tell <session> to write text "..."`. More precise
    /// (per session rather than per tab) when the user has it.
    ITerm2,
}

impl TerminalApp {
    /// The name AppleScript addresses the app by, and the label the UI shows.
    pub fn app_name(&self) -> &'static str {
        match self {
            TerminalApp::Apple => "Terminal",
            TerminalApp::ITerm2 => "iTerm2",
        }
    }
}

/// Why an Apple Events send did not land.
///
/// `NotPermitted` is called out separately from `Failed` because it is the one
/// failure that is PERMANENT: macOS records the refusal and never re-prompts, so
/// the caller must switch that harness to notify-only and point the user at
/// System Settings instead of retrying forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationError {
    /// `errAEEventNotPermitted` (-1743). Sticky. Recoverable only in System Settings.
    NotPermitted,
    /// `procNotFound` (-600): the emulator is not running.
    TargetNotRunning,
    /// The emulator is scriptable and permitted, but no tab or session owns that tty.
    NoMatchingTab,
    /// Anything else, with the raw message kept for the UI's detail line.
    Failed(String),
    /// No Apple Events surface on this platform at all.
    Unsupported,
}

impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutomationError::NotPermitted => write!(
                f,
                "automation permission denied (errAEEventNotPermitted): enable WARDEN under Privacy and Security, Automation"
            ),
            AutomationError::TargetNotRunning => write!(f, "the terminal app is not running"),
            AutomationError::NoMatchingTab => {
                write!(f, "no terminal tab is attached to that session's tty")
            }
            AutomationError::Failed(m) => write!(f, "{m}"),
            AutomationError::Unsupported => {
                write!(f, "this platform has no terminal automation surface")
            }
        }
    }
}

/// The controlling terminal of `pid` as a device path (`/dev/ttys001`), or `None`
/// when the process has no tty (an IDE-hosted or daemonised session).
pub fn controlling_tty(pid: u32) -> Option<String> {
    imp::controlling_tty(pid)
}

/// Which terminal emulator owns `pid`, found by walking the parent chain until a
/// known emulator executable appears. `None` when the session is not inside one
/// of the emulators WARDEN can drive.
pub fn terminal_app_for_pid(pid: u32) -> Option<TerminalApp> {
    imp::terminal_app_for_pid(pid)
}

/// Type `text` plus a return into the tab or session attached to `tty`, exactly
/// as if the user had typed it.
///
/// This is the one place WARDEN writes INTO another program. It runs only for a
/// session the user explicitly armed, and it is the deliberate exception to the
/// read-only-toward-projects rule (it drives the agent, never the project files).
pub fn send_text_to_tty(
    app: TerminalApp,
    tty: &str,
    text: &str,
) -> std::result::Result<(), AutomationError> {
    imp::send_text_to_tty(app, tty, text)
}

/// Ask the OS whether WARDEN may drive `app`, PROMPTING the user the first time.
///
/// Called on first arm rather than at install, so the consent dialog arrives with
/// the reason visible on screen. `Ok(())` means permitted now.
pub fn request_automation(app: TerminalApp) -> std::result::Result<(), AutomationError> {
    imp::request_automation(app)
}

/// Open System Settings at Privacy and Security, Automation, where a sticky
/// denial is undone. Best-effort: failure to open a settings pane is not worth
/// failing a command over.
pub fn open_automation_settings() {
    imp::open_automation_settings();
}

/// Post a local notification. The notify-only degradation path: when WARDEN
/// cannot press the key it still says when the moment arrived.
pub fn post_notification(title: &str, body: &str) {
    imp::post_notification(title, body);
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
