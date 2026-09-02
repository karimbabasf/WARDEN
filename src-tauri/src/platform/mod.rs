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

/// Bring WARDEN itself in front of every other app.
///
/// Distinct from focusing a window: on macOS a window can be key inside an app that
/// is still behind the one the user is looking at, and Tauri's `set_focus` cannot
/// fix that on its own (see the macOS adapter). Anything that means "open WARDEN"
/// has to call this too. MUST run on the main thread.
pub fn activate_self() {
    imp::activate_self();
}

// ---------------------------------------------------------------------------
// Terminal LOCATION: finding the window an agent is running in, and raising it.
//
// This surface only READS the process tree and RAISES a window. It never types
// into another program, which is what keeps the read-only invariant in CLAUDE.md
// unconditional: bringing a window forward is the same class of act as
// `reveal_path` opening Finder.
// ---------------------------------------------------------------------------

/// A terminal emulator WARDEN can address one tab of, by the tty attached to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalApp {
    /// `Terminal.app`. Addressed per TAB (`tty of t`).
    Apple,
    /// `iTerm2`. Addressed per SESSION, so a split pane resolves exactly rather
    /// than landing in whichever pane happens to be focused.
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

/// What is hosting a session's process, as far up the tree as WARDEN can see.
///
/// Three arms rather than `Option`, because "the app is Ghostty and WARDEN cannot
/// address its tabs" and "there is no GUI app in the ancestry at all" are
/// different answers and the panel says different things about them. Guessing an
/// emulator would be worse than either: a blind AppleScript attempt against an
/// app the session does not belong to raises a permission dialog for a program
/// the user is not even running, and a refusal there is permanent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalHost {
    /// An emulator WARDEN knows how to point at one tab of.
    Scriptable(TerminalApp),
    /// A GUI app in the ancestry that exposes no tty-to-tab lookup (Ghostty,
    /// Warp, a VS Code integrated terminal). Carries its display name so the
    /// panel can name it instead of shrugging.
    Unscriptable(String),
    /// Nothing GUI above this process: a daemon, an ssh login, a detached run.
    None,
}

/// Why raising a window did not work.
///
/// `NotPermitted` is called out separately because it is the one failure that is
/// PERMANENT: macOS records the refusal and never re-prompts, so the caller must
/// point the user at System Settings instead of suggesting they try again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationError {
    /// `errAEEventNotPermitted` (-1743). Sticky. Recoverable only in System Settings.
    NotPermitted,
    /// `procNotFound` (-600): the emulator is not running.
    TargetNotRunning,
    /// The emulator is scriptable and permitted, but no tab owns that tty. The
    /// window was closed between the probe and the click.
    NoMatchingTab,
    /// Anything else, with the raw message kept for the UI's detail line.
    Failed(String),
    /// No Apple Events surface on this platform at all.
    Unsupported,
}

impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Short on purpose: the panel pairs this with an Open Settings
            // button, so the sentence does not have to carry the directions too.
            AutomationError::NotPermitted => {
                write!(f, "macOS has not allowed WARDEN to control your terminal")
            }
            AutomationError::TargetNotRunning => write!(f, "the terminal app is not running"),
            AutomationError::NoMatchingTab => {
                write!(f, "that window has closed since WARDEN last looked")
            }
            AutomationError::Failed(m) => write!(f, "{m}"),
            AutomationError::Unsupported => {
                write!(f, "this platform has no way to raise a terminal window")
            }
        }
    }
}

/// The controlling terminal of `pid` as a device path (`/dev/ttys001`), or `None`
/// when the process has no tty (an IDE-hosted or daemonised session).
pub fn controlling_tty(pid: u32) -> Option<String> {
    imp::controlling_tty(pid)
}

/// Which app is hosting `pid`, found by walking the parent chain until a GUI
/// bundle appears. See [`TerminalHost`] for why the answer is three-valued.
pub fn terminal_host_for_pid(pid: u32) -> TerminalHost {
    imp::terminal_host_for_pid(pid)
}

/// Select the tab or session attached to `tty`, raise its window, and bring the
/// emulator to the front.
///
/// Read-and-raise only. Nothing is typed, nothing is sent to the agent, and the
/// terminal's buffer is not touched.
pub fn focus_tty(app: TerminalApp, tty: &str) -> std::result::Result<(), AutomationError> {
    imp::focus_tty(app, tty)
}

/// Deep-link into Privacy and Security, Automation. A denial is sticky and never
/// re-prompts, so this pane is the only place the user can undo it.
pub fn open_automation_settings() {
    imp::open_automation_settings();
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

/// Every local agent process, from one `ps` sweep.
///
/// This is the harness-INDEPENDENT liveness source. [`process_alive`] answers a
/// question you can only ask once you already hold a pid, which is exactly what a
/// harness without a registry never gives you; this enumerates instead, so a
/// harness that publishes nothing is still tracked, and its death is observed on
/// the next sweep rather than inferred from a file that stopped changing.
///
/// Thin syscall wrapper by design: the decisions live in
/// [`crate::radar::procs::scan_ps_output`], which is pure and carries the tests.
///
/// `/bin/ps` rather than a process-listing crate: `controlling_tty` and
/// `terminal_host_for_pid` already read the process table this way, so this adds
/// no dependency and no new failure mode. `-axo pid=,ppid=,lstart=,args=` is BSD
/// syntax; Linux's `ps` accepts the same fields, so the one porting hazard is a
/// platform whose `ps` lacks `lstart`, where the parse rejects every row and the
/// scan degrades to empty rather than to wrong identities.
/// `None` when the sweep could not run. That is NOT the same as an empty vec, and
/// the caller must not flatten it: an empty sweep means "no agents are running",
/// which closes every globe, while a failed sweep means "we did not look" and has
/// to leave the board alone.
pub fn list_agent_processes() -> Option<Vec<crate::radar::procs::AgentProcess>> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,lstart=,args="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Some(crate::radar::procs::scan_ps_output(&text, std::process::id()))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Working directories for `pids`, in ONE `lsof` call.
///
/// A missing entry is normal and must stay survivable: `lsof` can be denied by
/// policy, and a process can exit between the sweep and this lookup. The caller
/// treats an absent directory as "cannot decide", never as "not running".
///
/// One call for the whole set rather than one per pid: this runs on the liveness
/// tick, and a spawn per agent would scale the cost with the size of the fleet
/// exactly when the fleet is busiest.
pub fn process_cwds(pids: &[u32]) -> std::collections::HashMap<u32, String> {
    #[cfg(unix)]
    {
        if pids.is_empty() {
            return std::collections::HashMap::new();
        }
        let list = pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
            .args(["-a", "-p", &list, "-d", "cwd", "-Fn"])
            .output()
        else {
            return std::collections::HashMap::new();
        };
        crate::radar::procs::parse_lsof_cwds(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(unix))]
    {
        let _ = pids;
        std::collections::HashMap::new()
    }
}

/// The full process picture RADAR asks liveness questions of: one `ps` sweep plus
/// one `lsof` for the agents it found.
pub fn process_index() -> crate::radar::procs::ProcessIndex {
    let Some(procs) = list_agent_processes() else {
        tracing::warn!("process sweep failed; RADAR liveness falls back to file rules");
        return crate::radar::procs::ProcessIndex::unscanned();
    };
    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let cwds = process_cwds(&pids);
    crate::radar::procs::ProcessIndex::new(procs, cwds)
}
