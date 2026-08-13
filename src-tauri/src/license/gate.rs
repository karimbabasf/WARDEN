//! The gate decision, and the proof that it has exactly one input.
//!
//! Two rules, both load-bearing:
//!
//! 1. A RELEASE build gates. A DEBUG build does not, so `pnpm tauri dev` never
//!    asks Karim for a key.
//! 2. There is NO other way to turn the gate off. Not an environment variable,
//!    not a config file, not a CLI flag, not a file whose mere presence unlocks
//!    anything. In a release build the only thing that opens the radar is a
//!    signature that verifies against the compiled-in public key.
//!
//! Rule 2 is the one that rots quietly, so it is enforced by a test that reads
//! this module's own source (`the_gate_module_reads_no_environment`), in the
//! same spirit as the projection canary in `observe/projection.rs`. A future
//! "just for testing" escape hatch fails the suite instead of shipping.

use super::store;
use super::verify::{verify_key, LicenseClaims};

/// The gate's ONLY input beyond the stored key, fixed at compile time.
///
/// `cfg!(debug_assertions)` is a constant the compiler folds away, so in a
/// release binary the `DevBypass` arm below is not merely unreachable, it is not
/// present in the emitted code. There is nothing to flip at runtime.
pub const GATE_ENABLED: bool = !cfg!(debug_assertions);

/// What the app should do on launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateStatus {
    /// A stored key verified. Open the radar.
    Activated(LicenseClaims),
    /// Gated, and no usable key on disk. Show the activation screen.
    Locked,
    /// Debug build. Not gated at all.
    DevBypass,
}

impl GateStatus {
    /// May the app run? True for a verified key and for a debug build, and for
    /// nothing else.
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Activated(_) | Self::DevBypass)
    }

    /// The verified claims, when there are any. `DevBypass` has none: a dev build
    /// deliberately knows nothing about who owns it.
    pub fn claims(&self) -> Option<&LicenseClaims> {
        match self {
            Self::Activated(c) => Some(c),
            Self::Locked | Self::DevBypass => None,
        }
    }
}

/// The whole decision, as a pure function of its two inputs.
///
/// `gate_enabled` is a parameter rather than a read of [`GATE_ENABLED`] so that
/// BOTH branches are exercised by the test suite, which only ever runs in one
/// profile at a time. The release behaviour is therefore tested by a debug run.
pub fn decide(gate_enabled: bool, stored: Option<&str>) -> GateStatus {
    if !gate_enabled {
        return GateStatus::DevBypass;
    }
    match stored {
        // A corrupt, truncated, or forged stored key is the same as no key: the
        // activation screen. It is never repaired, never partially trusted.
        Some(key) => match verify_key(key) {
            Ok(claims) => GateStatus::Activated(claims),
            Err(_) => GateStatus::Locked,
        },
        None => GateStatus::Locked,
    }
}

/// The live status: the compiled-in gate flag against whatever is on disk.
pub fn status() -> GateStatus {
    decide(GATE_ENABLED, store::read().as_deref())
}

/// Latches open once, and never closed.
///
/// The gate is a LAUNCH gate. Once a verified key has opened this process, a
/// home directory that goes briefly unreadable (a sleeping external volume, a
/// permissions repair) must not lock a paying user out mid-session. The latch
/// only ever moves in one direction, and the only thing that moves it is
/// [`status`] returning an open verdict, which needs a signature that verifies.
static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Is the app open right now? Re-reads and re-verifies the stored key until the
/// answer is yes, then remembers it.
pub fn is_open_now() -> bool {
    use std::sync::atomic::Ordering;
    if OPENED.load(Ordering::Relaxed) {
        return true;
    }
    let open = status().is_open();
    if open {
        OPENED.store(true, Ordering::Relaxed);
    }
    open
}

/// The rule the IPC gate in `lib.rs` applies to a live invoke.
pub fn allows(command: &str) -> bool {
    is_open_now() || UNGATED_COMMANDS.contains(&command)
}

/// The commands that still answer while the app is locked.
///
/// An allowlist, not a denylist: a command added to `commands.rs` next month is
/// gated by default rather than exposed by default. `diag` is here because the
/// packaged window has no devtools and a silent activation screen would be
/// undebuggable; it only writes to the local log. The window verbs are here so a
/// user who does not want to activate can still close the window.
pub const UNGATED_COMMANDS: &[&str] = &[
    "license_status",
    "license_activate",
    "diag",
    "hide_overlay",
    "hide_window",
    "minimize_window",
];

/// The rule the IPC gate in `lib.rs` applies to every incoming command.
pub fn command_is_allowed(status: &GateStatus, command: &str) -> bool {
    status.is_open() || UNGATED_COMMANDS.contains(&command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn vectors_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("docs")
            .join("license-vectors.json")
    }

    fn a_valid_key() -> String {
        let raw = std::fs::read_to_string(vectors_path()).expect("read license-vectors.json");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        v["valid"][0]["key"]
            .as_str()
            .expect("first valid vector")
            .to_string()
    }

    // ── the gated branch, exercised regardless of which profile the suite runs in ──

    #[test]
    fn gated_with_no_stored_key_is_locked() {
        assert_eq!(decide(true, None), GateStatus::Locked);
    }

    #[test]
    fn gated_with_a_valid_stored_key_is_activated() {
        let status = decide(true, Some(&a_valid_key()));
        assert!(status.is_open());
        assert_eq!(
            status.claims().map(|c| c.email.as_str()),
            Some("a@example.com")
        );
    }

    #[test]
    fn gated_with_a_corrupt_stored_key_is_locked() {
        for junk in [
            "",
            "   ",
            "hello",
            "WRDN-",
            "WRDN-not-a-key",
            // A real key with one payload byte edited: the tampered-seats vector.
            "WRDN-eyJ2IjoxLCJpZCI6ImNzX3Rlc3RfYTEiLCJlbWFpbCI6ImFAZXhhbXBsZS5jb20iLCJzZWF0cyI6MywiaWF0IjoxNzg1NzAwMDAwfQ.xUaQMiW3AG1BBTQpBv9zgyb4pevwZL93titEFUQMPVxMGLVfUh4gAplgimsoLMS6qSVdui5hen6wkhtzA8LABQ",
        ] {
            assert_eq!(decide(true, Some(junk)), GateStatus::Locked, "junk {junk:?} unlocked the app");
        }
    }

    #[test]
    fn a_gated_build_answers_only_the_activation_commands() {
        let locked = decide(true, None);
        for c in UNGATED_COMMANDS {
            assert!(
                command_is_allowed(&locked, c),
                "{c} should answer while locked"
            );
        }
        for c in [
            "get_radar_state",
            "rename_session",
            "reveal_path",
            "preview_file",
            "preview_observed_state",
            "observe_start_sharing",
            "observe_create_grant",
            "observe_add_peer",
            // The default for anything invented later.
            "some_command_added_next_quarter",
        ] {
            assert!(!command_is_allowed(&locked, c), "{c} answered while locked");
        }
    }

    #[test]
    fn an_activated_build_answers_everything() {
        let open = decide(true, Some(&a_valid_key()));
        for c in ["get_radar_state", "rename_session", "observe_start_sharing"] {
            assert!(command_is_allowed(&open, c));
        }
    }

    // ── the ungated branch ────────────────────────────────────────────────────

    #[test]
    fn an_ungated_build_never_asks_for_a_key() {
        assert_eq!(decide(false, None), GateStatus::DevBypass);
        assert_eq!(decide(false, Some("garbage")), GateStatus::DevBypass);
        assert!(decide(false, None).is_open());
        assert!(command_is_allowed(&decide(false, None), "get_radar_state"));
    }

    // ── the profile constant ──────────────────────────────────────────────────

    #[test]
    fn the_gate_flag_is_the_inverse_of_the_debug_profile() {
        assert_eq!(GATE_ENABLED, !cfg!(debug_assertions));
    }

    /// Runs only under `cargo test --release`, which is the build the customer
    /// gets. Asserts the shipped profile really does gate and really does refuse
    /// to open without a key.
    #[cfg(not(debug_assertions))]
    #[test]
    // The constant IS the subject: this test exists to pin the value the
    // compiler folded in, so clippy's "assertion has a constant value" is the
    // expected shape rather than a smell.
    #[allow(clippy::assertions_on_constants)]
    fn a_release_build_gates() {
        assert!(GATE_ENABLED, "a release build shipped with the gate OFF");
        assert_eq!(decide(GATE_ENABLED, None), GateStatus::Locked);
        assert!(!decide(GATE_ENABLED, None).is_open());
        assert!(!command_is_allowed(
            &decide(GATE_ENABLED, None),
            "get_radar_state"
        ));
        assert_eq!(
            decide(GATE_ENABLED, Some("WRDN-forged.key")),
            GateStatus::Locked
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    // Same reason as the release twin above: pinning a folded constant.
    #[allow(clippy::assertions_on_constants)]
    fn a_debug_build_bypasses_so_tauri_dev_never_prompts() {
        assert!(!GATE_ENABLED, "a debug build would prompt for a key");
        assert_eq!(decide(GATE_ENABLED, None), GateStatus::DevBypass);
    }

    // ── the canary: no second input, ever ─────────────────────────────────────

    /// Every `.rs` file in the license module: the code that actually ships, with
    /// the `#[cfg(test)]` tail and every comment line removed.
    ///
    /// Both exclusions are sound rather than convenient. A `#[cfg(test)]` block is
    /// compiled out of the release binary and a comment is never executed, so
    /// neither can be a shipped backdoor. Dropping them is also what lets the
    /// forbidden needles below be written as plain literals, and what keeps a
    /// doc comment that merely NAMES the switch from failing the check.
    fn production_sources() -> Vec<(String, String)> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("license");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("license module directory") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read source");
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            let production: String = src
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            out.push((name, production));
        }
        out.sort();
        out
    }

    /// Reads the shipped source of the license module and fails if any of it can
    /// take an instruction from outside. This is the test that catches the
    /// "temporary" skip-the-license switch someone adds while debugging a release
    /// build and forgets to remove.
    #[test]
    fn the_gate_module_reads_no_environment() {
        const FORBIDDEN: &[(&str, &str)] = &[
            ("env::var", "a runtime environment read"),
            ("var_os", "a runtime environment read"),
            ("option_env!", "a build-time environment read"),
            ("env!(", "a build-time environment read"),
            ("WARDEN_", "an environment variable name"),
            ("args()", "a command-line flag"),
        ];

        let sources = production_sources();
        assert!(
            sources.len() >= 5,
            "expected the whole module, scanned {}",
            sources.len()
        );

        for (name, src) in &sources {
            for (needle, what) in FORBIDDEN {
                assert!(
                    !src.contains(needle),
                    "BACKDOOR: {name} contains `{needle}` ({what}). The license gate takes no \
                     input but the stored key and the compile-time profile.",
                );
            }
        }
    }

    /// The profile switch exists in exactly one place. A second
    /// `cfg!(debug_assertions)` elsewhere in the module would mean a second, less
    /// visible way for a build to decide it is not gated.
    #[test]
    fn the_profile_is_consulted_in_exactly_one_place() {
        let sites: Vec<(String, usize)> = production_sources()
            .into_iter()
            .map(|(name, src)| (name, src.matches("cfg!(debug_assertions)").count()))
            .filter(|(_, n)| *n > 0)
            .collect();
        assert_eq!(
            sites,
            vec![("gate.rs".to_string(), 1)],
            "the debug/release switch must exist once, in gate.rs, and nowhere else",
        );
    }

    /// The allowlist must never grow a command that reads the user's machine.
    /// Spelled out rather than derived, so widening it is a deliberate edit that
    /// shows up in review.
    #[test]
    fn the_ungated_allowlist_is_exactly_these_six() {
        assert_eq!(
            UNGATED_COMMANDS,
            &[
                "license_status",
                "license_activate",
                "diag",
                "hide_overlay",
                "hide_window",
                "minimize_window",
            ],
        );
    }
}
