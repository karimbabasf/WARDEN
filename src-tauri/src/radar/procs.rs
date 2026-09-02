//! Local agent PROCESSES: which harnesses are running on this machine right now.
//!
//! RADAR's liveness has always been per-harness and file-shaped: Claude has a
//! `<pid>.json` registry, Codex has a directory a rollout moves out of, and every
//! other harness has nothing. That asymmetry is the reason a killed Codex agent
//! kept its globe: with no pid to probe, "gone" could only be inferred from a file
//! that stops changing, and inference takes time that a dead process does not.
//!
//! This module is the harness-independent answer. One `ps` sweep names every agent
//! process on the machine, so liveness becomes a fact for harnesses that publish
//! nothing, and death is observed rather than waited out.
//!
//! Two rules shape it, both learned from the registry path it generalises:
//!
//! * **The pure core is separate from the syscall.** [`classify`] and
//!   [`parse_ps_line`] are total functions over strings and carry every test; the
//!   `ps` invocation lives behind [`crate::platform::list_agent_processes`] and is
//!   never in the tested path. Same split as `pid_alive` in [`super::liveness`].
//! * **A pid is not an identity.** Pids are recycled, and a recycled one reads as
//!   "still alive" to any check that holds only a number. Every process here
//!   carries its start time, so a match means the same process rather than the
//!   same slot. Claude's own registry writes `procStart` for this reason; this
//!   generalises the guard to harnesses that write nothing at all.

use crate::ir::Harness;

/// One agent process observed on this machine.
///
/// `started_at` is the raw `ps lstart` string, kept opaque on purpose: it is
/// compared, never parsed. Its only job is to make `(pid, started_at)` a stable
/// identity across a pid recycle, and a string compare does that without opening
/// a locale-dependent date parse that could only add failure modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProcess {
    pub harness: Harness,
    pub pid: u32,
    pub ppid: u32,
    pub started_at: String,
    /// The executable that identified this process, as a basename. Kept for the
    /// log line that explains why a process was claimed by a harness.
    pub argv0: String,
}

impl AgentProcess {
    /// The identity token that survives pid reuse: same pid AND same start time.
    pub fn identity(&self) -> (u32, &str) {
        (self.pid, self.started_at.as_str())
    }
}

/// Interpreters that run an agent as a SCRIPT rather than as their own binary.
///
/// Hermes is the live case on this machine: it runs as
/// `Python .../venv/bin/hermes -m fugu-ultra`, so its argv[0] basename is the
/// interpreter and the harness name is one argument along. Matching argv[1] only
/// for this closed list keeps that case working without turning the whole scan
/// into a substring search over every command line, which is what would start
/// claiming unrelated processes that merely mention a harness in a flag.
const INTERPRETERS: &[&str] = &[
    "python", "python3", "python3.11", "python3.12", "python3.13", "node", "bun", "deno", "ruby",
    "uv", "uvx",
];

/// Executable basename to harness. EXACT basename match, never a substring.
///
/// The substring form is tempting and wrong: macOS ships
/// `CursorUIViewService`, a text-input helper present on every Mac, which a
/// `contains("cursor")` test claims as a Cursor agent on a machine where Cursor
/// was never installed. An exact basename cannot make that mistake.
fn harness_for_basename(name: &str) -> Option<Harness> {
    match name {
        "claude" | "claude-code" => Some(Harness::ClaudeCode),
        "codex" => Some(Harness::Codex),
        "grok" => Some(Harness::Grok),
        "cursor-agent" => Some(Harness::Cursor),
        "hermes" => Some(Harness::Hermes),
        "openclaw" | "claw" => Some(Harness::OpenClaw),
        _ => None,
    }
}

/// The basename of a path, without allocating a `Path`.
///
/// Takes the last `/`-separated segment. `ps` reports argv verbatim, so this sees
/// whatever the caller was invoked as: an absolute path, a bare name off `PATH`,
/// or a relative `./bin/grok`. All three reduce correctly.
fn basename(arg: &str) -> &str {
    arg.rsplit('/').next().unwrap_or(arg)
}

/// True when this executable lives inside a macOS application bundle.
///
/// A DESKTOP APP IS NOT A CLI AGENT, and on this machine the two share a name:
/// Claude Desktop runs as `/Applications/Claude.app/Contents/MacOS/Claude`, and
/// its GPU/renderer/utility helpers run as `Claude Helper` out of a nested
/// `.app`. Without this test the scan reported nine Claude Desktop processes as
/// nine live agents, which would have put a globe on the board for every helper
/// Electron happened to fork.
///
/// Tested on the bundle marker rather than on the app's name so it holds for
/// Cursor.app and anything else that ships a GUI beside a CLI of the same name.
fn is_app_bundle(arg: &str) -> bool {
    arg.contains(".app/Contents/")
}

/// Which harness, if any, this command line belongs to.
///
/// Reads argv[0]'s basename first. When that names an interpreter from
/// [`INTERPRETERS`], reads argv[1] too, skipping interpreter FLAGS (`-u`, `-m`)
/// so `python -u .../hermes` resolves the same as `python .../hermes`. Returns
/// `None` for everything else, which is the overwhelming majority of a 985-process
/// machine and must stay cheap.
pub fn classify(argv: &[&str]) -> Option<Harness> {
    let first = argv.first()?;
    if is_app_bundle(first) {
        return None;
    }
    // Case-folded because the case in argv[0] is not the program's to choose:
    // macOS's framework Python reports itself as `Python`, and the filesystem is
    // case-insensitive besides, so `Claude` and `claude` are one binary. Folding
    // cannot widen the match into a false positive, since every arm below is
    // still an exact whole-basename compare.
    let head = basename(first).to_ascii_lowercase();
    if let Some(h) = harness_for_basename(&head) {
        return Some(h);
    }
    if !INTERPRETERS.contains(&head.as_str()) {
        return None;
    }
    // An interpreter: the script it is running decides. Skip leading flags and
    // their values so `-m hermes` and `-u script.py` both land on the script.
    argv.iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .and_then(|a| harness_for_basename(&basename(a).to_ascii_lowercase()))
}

/// One row of `ps -axo pid=,ppid=,lstart=,args=`, split into its fixed prefix and
/// its command line.
///
/// The format is unambiguous DESPITE argv containing spaces, because every field
/// before it has a known token count: pid (1), ppid (1), then `lstart`'s fixed
/// five (`Sun Aug 30 02:10:32 2026`). Everything from token 8 on is argv. Parsing
/// by count rather than by whitespace-splitting the whole line is what keeps a
/// path like `/Applications/My App/bin/claude` from shifting the columns.
///
/// Returns `None` on any row that does not have that shape, so a `ps` that ever
/// changes its output degrades to "no agent processes found" rather than to
/// garbage identities.
pub fn parse_ps_line(line: &str) -> Option<(u32, u32, String, Vec<String>)> {
    let mut tokens = line.split_whitespace();
    let pid = tokens.next()?.parse::<u32>().ok()?;
    let ppid = tokens.next()?.parse::<u32>().ok()?;
    let mut started = String::new();
    for i in 0..5 {
        if i > 0 {
            started.push(' ');
        }
        started.push_str(tokens.next()?);
    }
    let argv: Vec<String> = tokens.map(str::to_string).collect();
    if argv.is_empty() {
        return None;
    }
    Some((pid, ppid, started, argv))
}

/// Turn raw `ps` output into the agent processes it contains.
///
/// `self_pid` is dropped so WARDEN never reports itself, which matters because
/// WARDEN is frequently launched by the very harnesses it watches and would
/// otherwise appear as one of their agents.
pub fn scan_ps_output(output: &str, self_pid: u32) -> Vec<AgentProcess> {
    let mut out = Vec::new();
    for line in output.lines() {
        let Some((pid, ppid, started_at, argv)) = parse_ps_line(line) else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let Some(harness) = classify(&refs) else {
            continue;
        };
        out.push(AgentProcess {
            harness,
            pid,
            ppid,
            started_at,
            argv0: basename(&argv[0]).to_string(),
        });
    }
    out
}

/// Parse `lsof -d cwd -Fn` output into pid to working directory.
///
/// The `-F` machine format is a stream of one-letter-tagged lines: `p<pid>` opens
/// a process block, `f<fd>` opens a file block within it, `n<name>` gives that
/// file's path. Asking for `-d cwd` means the only file in each block is the
/// working directory, so the last `n` seen after a `p` is the answer.
///
/// Parsed rather than shelled per-pid because one `lsof` for the whole agent set
/// is a single process spawn on a path that runs every liveness tick.
pub fn parse_lsof_cwds(output: &str) -> std::collections::HashMap<u32, String> {
    let mut out = std::collections::HashMap::new();
    let mut current: Option<u32> = None;
    for line in output.lines() {
        let Some((tag, rest)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => current = rest.parse::<u32>().ok(),
            "n" => {
                if let Some(pid) = current {
                    out.insert(pid, rest.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

/// What the process table can say about one session's liveness.
///
/// Three-valued on purpose. A two-valued answer would have to guess when the OS
/// declines to report a working directory, and the guess is unsafe in both
/// directions: guessing Open reinstates the lingering globe this module exists to
/// remove, guessing Closed implodes a working agent because `lsof` was denied.
/// `Unknown` hands the decision back to the file-shaped rule that ran before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// A live process of this harness is working in this session's directory.
    Open,
    /// No live process can own this session. Implode it now.
    Closed,
    /// The process table cannot decide. Defer to the caller's file-based rule.
    Unknown,
}

/// The process sweep, indexed for per-session liveness questions.
///
/// `cwds` is separate from the sweep because a working directory costs a second
/// syscall per pid: the sweep is one `ps` over ~1000 processes, and only the
/// handful it claims are ever asked where they are running.
#[derive(Debug, Clone, Default)]
pub struct ProcessIndex {
    procs: Vec<AgentProcess>,
    cwds: std::collections::HashMap<u32, String>,
    /// Did the sweep actually run? An index that never observed the machine must
    /// answer `Unknown` to everything.
    ///
    /// This is the difference between "nothing is running" and "we did not look",
    /// and conflating them is catastrophic in one direction: a failed `ps` would
    /// otherwise report zero processes for every harness, which reads as "all
    /// agents are dead" and implodes the entire board at once. The default is
    /// deliberately the safe one, so a caller that forgets to scan degrades to the
    /// file-based behaviour that predates this module rather than to an empty
    /// radar.
    scanned: bool,
}

impl ProcessIndex {
    pub fn new(
        procs: Vec<AgentProcess>,
        cwds: std::collections::HashMap<u32, String>,
    ) -> Self {
        Self {
            procs,
            cwds,
            scanned: true,
        }
    }

    /// An index that observed nothing and therefore claims nothing. Used when the
    /// sweep fails, and as the default in tests of the file-based rules.
    pub fn unscanned() -> Self {
        Self::default()
    }

    pub fn processes(&self) -> &[AgentProcess] {
        &self.procs
    }

    /// Live processes belonging to `harness`.
    fn of(&self, harness: &Harness) -> impl Iterator<Item = &AgentProcess> {
        let want = harness.as_str().to_string();
        self.procs
            .iter()
            .filter(move |p| p.harness.as_str() == want)
    }

    /// Can a live process own a session of `harness` running in `session_cwd`?
    ///
    /// The rule, in the order it decides:
    ///
    /// 1. **No process of this harness at all → `Closed`.** This is the whole
    ///    point and it needs no directory: a harness with nothing running has no
    ///    open sessions, whatever its files still say. It is what makes killing
    ///    the last Codex agent implode its globe on the next tick instead of
    ///    waiting for the rollout to be archived.
    /// 2. **No process whose directory we could read → `Unknown`.** `lsof` can be
    ///    denied or race a process that just exited; that must never read as "the
    ///    fleet is gone".
    /// 3. **Otherwise, `Open` iff some live process of this harness is running in
    ///    that directory**, else `Closed`.
    ///
    /// A session with no recorded directory can only reach rule 1, so it is
    /// `Unknown` whenever the harness is running at all: without a directory
    /// there is nothing to match on, and refusing to guess is the honest answer.
    pub fn resolve(&self, harness: &Harness, session_cwd: Option<&str>) -> Liveness {
        if !self.scanned {
            return Liveness::Unknown;
        }
        let mut any = false;
        let mut any_cwd = false;
        let mut matched = false;
        for p in self.of(harness) {
            any = true;
            if let Some(cwd) = self.cwds.get(&p.pid) {
                any_cwd = true;
                if Some(cwd.as_str()) == session_cwd {
                    matched = true;
                }
            }
        }
        if !any {
            return Liveness::Closed;
        }
        if !any_cwd || session_cwd.is_none() {
            return Liveness::Unknown;
        }
        if matched {
            Liveness::Open
        } else {
            Liveness::Closed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(harness: Harness, pid: u32) -> AgentProcess {
        AgentProcess {
            harness,
            pid,
            ppid: 1,
            started_at: "Sun Aug 30 02:10:32 2026".to_string(),
            argv0: "x".to_string(),
        }
    }

    fn index(procs: Vec<AgentProcess>, cwds: &[(u32, &str)]) -> ProcessIndex {
        ProcessIndex::new(
            procs,
            cwds.iter().map(|(p, c)| (*p, c.to_string())).collect(),
        )
    }

    #[test]
    fn parse_lsof_cwds_reads_the_machine_format() {
        let out = "p51434\nfcwd\nn/Users/k\np59829\nfcwd\nn/Users/k/Developer/WARDEN\n";
        let map = parse_lsof_cwds(out);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&51434).map(String::as_str), Some("/Users/k"));
        assert_eq!(
            map.get(&59829).map(String::as_str),
            Some("/Users/k/Developer/WARDEN")
        );
    }

    /// A process block with no `n` line (the process exited between the sweep and
    /// the lookup) contributes nothing rather than a bogus directory.
    #[test]
    fn parse_lsof_cwds_skips_a_process_with_no_path() {
        let map = parse_lsof_cwds("p1\nfcwd\np2\nfcwd\nn/tmp\n");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&2).map(String::as_str), Some("/tmp"));
    }

    #[test]
    fn parse_lsof_cwds_is_empty_safe() {
        assert!(parse_lsof_cwds("").is_empty());
    }

    /// The reported bug, as a test. Codex publishes no pid, so before this a
    /// killed agent kept its globe until its rollout was archived. With nothing
    /// of that harness running, the answer is Closed with no directory needed.
    #[test]
    fn resolve_closes_a_harness_with_no_live_process() {
        let idx = index(vec![proc(Harness::ClaudeCode, 10)], &[(10, "/a")]);
        assert_eq!(idx.resolve(&Harness::Codex, Some("/a")), Liveness::Closed);
    }

    #[test]
    fn resolve_opens_a_session_whose_directory_has_a_live_process() {
        let idx = index(vec![proc(Harness::Codex, 20)], &[(20, "/repo")]);
        assert_eq!(idx.resolve(&Harness::Codex, Some("/repo")), Liveness::Open);
    }

    /// Two agents of one harness, one killed: the survivor must not keep the
    /// dead one's globe alive. This is why the match is per-directory and not
    /// merely "is the harness running".
    #[test]
    fn resolve_closes_one_session_while_a_sibling_survives() {
        let idx = index(vec![proc(Harness::Codex, 20)], &[(20, "/alive")]);
        assert_eq!(idx.resolve(&Harness::Codex, Some("/alive")), Liveness::Open);
        assert_eq!(idx.resolve(&Harness::Codex, Some("/killed")), Liveness::Closed);
    }

    /// `lsof` denied or racing an exit: defer, never implode the fleet.
    #[test]
    fn resolve_defers_when_no_directory_could_be_read() {
        let idx = index(vec![proc(Harness::Codex, 20)], &[]);
        assert_eq!(idx.resolve(&Harness::Codex, Some("/repo")), Liveness::Unknown);
    }

    /// A session the store never recorded a directory for has nothing to match
    /// against, so it defers rather than guessing.
    #[test]
    fn resolve_defers_for_a_session_with_no_directory() {
        let idx = index(vec![proc(Harness::Codex, 20)], &[(20, "/repo")]);
        assert_eq!(idx.resolve(&Harness::Codex, None), Liveness::Unknown);
    }

    /// The safety valve, and the most important test in the module. A failed `ps`
    /// yields an index holding zero processes, which is byte-identical to "nothing
    /// is running" unless the two are distinguished. Without the `scanned` flag a
    /// single failed sweep would implode every globe on the board at once.
    #[test]
    fn an_unscanned_index_claims_nothing_rather_than_closing_everything() {
        let idx = ProcessIndex::unscanned();
        assert_eq!(idx.resolve(&Harness::Codex, Some("/repo")), Liveness::Unknown);
        assert_eq!(idx.resolve(&Harness::ClaudeCode, None), Liveness::Unknown);
    }

    /// And the contrast: a sweep that RAN and found nothing does close, because
    /// that is a real observation.
    #[test]
    fn a_scanned_empty_index_closes() {
        let idx = ProcessIndex::new(Vec::new(), Default::default());
        assert_eq!(idx.resolve(&Harness::Codex, Some("/repo")), Liveness::Closed);
    }

    #[test]
    fn classify_reads_a_bare_binary_name() {
        assert_eq!(classify(&["claude"]), Some(Harness::ClaudeCode));
        assert_eq!(classify(&["codex", "resume"]), Some(Harness::Codex));
    }

    #[test]
    fn classify_reads_an_absolute_path() {
        assert_eq!(
            classify(&["/Users/k/.grok/bin/grok"]),
            Some(Harness::Grok)
        );
        assert_eq!(
            classify(&["/Users/k/.local/bin/cursor-agent", "--resume"]),
            Some(Harness::Cursor)
        );
    }

    /// The load-bearing negative. `CursorUIViewService` ships with macOS and is
    /// running on every Mac, so a substring match on "cursor" would put a Cursor
    /// globe on the board of someone who has never installed Cursor.
    #[test]
    fn classify_does_not_claim_the_macos_cursor_helper() {
        let path = "/System/Library/PrivateFrameworks/TextInputUIMacHelper.framework/Versions/A/XPCServices/CursorUIViewService.xpc/Contents/MacOS/CursorUIViewService";
        assert_eq!(classify(&[path]), None);
    }

    /// Claude DESKTOP is not Claude Code. Both are named `Claude`, and the desktop
    /// app forks a helper per renderer, so before the bundle test a live probe on
    /// this machine reported 9 phantom agents beside the 3 real CLI sessions.
    #[test]
    fn classify_does_not_claim_a_desktop_app_bundle() {
        assert_eq!(
            classify(&["/Applications/Claude.app/Contents/MacOS/Claude"]),
            None
        );
        // Electron helpers arrive with a space in the path, so `ps` splits them
        // across argv. argv[0] still carries the bundle marker, which is why the
        // test is on the marker and not on the basename.
        assert_eq!(
            classify(&[
                "/Applications/Claude.app/Contents/Frameworks/Claude",
                "Helper.app/Contents/MacOS/Claude",
                "Helper",
                "--type=gpu-process",
            ]),
            None
        );
    }

    /// The CLI of the same name, invoked off PATH, is still an agent.
    #[test]
    fn classify_still_claims_the_cli_beside_the_desktop_app() {
        assert_eq!(
            classify(&["claude", "--effort", "max"]),
            Some(Harness::ClaudeCode)
        );
    }

    /// Nor anything that merely mentions a harness in an argument: an editor
    /// opening a file named `codex` is not an agent.
    #[test]
    fn classify_ignores_a_harness_name_in_a_plain_argument() {
        assert_eq!(classify(&["/usr/bin/vim", "notes/codex.md"]), None);
        assert_eq!(classify(&["/bin/cat", "grok"]), None);
    }

    /// Hermes runs as a Python script, which is why argv[1] is read at all.
    #[test]
    fn classify_reads_an_interpreter_hosted_agent() {
        assert_eq!(
            classify(&["Python", "/Users/k/.hermes/venv/bin/hermes", "-m", "fugu-ultra"]),
            Some(Harness::Hermes)
        );
        assert_eq!(
            classify(&["python3", "-u", "/opt/venv/bin/hermes"]),
            Some(Harness::Hermes)
        );
    }

    /// An interpreter running anything else stays unclaimed.
    #[test]
    fn classify_ignores_an_interpreter_running_a_normal_script() {
        assert_eq!(classify(&["python3", "/Users/k/scripts/backup.py"]), None);
        assert_eq!(classify(&["node", "/Users/k/app/server.js"]), None);
    }

    #[test]
    fn classify_is_empty_safe() {
        assert_eq!(classify(&[]), None);
    }

    #[test]
    fn parse_ps_line_splits_the_fixed_prefix_from_argv() {
        let line = "51434     1 Tue Sep  1 23:11:51 2026 claude --resume";
        let (pid, ppid, started, argv) = parse_ps_line(line).unwrap();
        assert_eq!(pid, 51434);
        assert_eq!(ppid, 1);
        assert_eq!(started, "Tue Sep 1 23:11:51 2026");
        assert_eq!(argv, vec!["claude", "--resume"]);
    }

    /// The reason the prefix is parsed by TOKEN COUNT and not by splitting the
    /// whole line: an executable path containing a space must not shift columns.
    #[test]
    fn parse_ps_line_survives_a_space_in_the_executable_path() {
        let line = "900 1 Sun Aug 30 02:10:32 2026 /Applications/My App/bin/claude";
        let (pid, _, _, argv) = parse_ps_line(line).unwrap();
        assert_eq!(pid, 900);
        assert_eq!(argv, vec!["/Applications/My", "App/bin/claude"]);
        // argv[0] alone cannot classify it, and that is the honest outcome: a
        // space-bearing path is indistinguishable from two arguments in ps output.
        // It resolves because argv[1]'s basename is still `claude`... which is a
        // coincidence of THIS path shape, so assert only what is guaranteed.
        assert_eq!(&argv[0], "/Applications/My");
    }

    #[test]
    fn parse_ps_line_rejects_a_malformed_row() {
        assert_eq!(parse_ps_line(""), None);
        assert_eq!(parse_ps_line("not-a-pid 1 Sun Aug 30 02:10:32 2026 claude"), None);
        // Truncated date: fewer than the five lstart tokens plus an argv.
        assert_eq!(parse_ps_line("900 1 Sun Aug 30 claude"), None);
    }

    #[test]
    fn scan_ps_output_finds_agents_and_drops_the_rest() {
        let out = "\
    1     0 Sun Aug 30 02:10:32 2026 /sbin/launchd
51434     1 Tue Sep  1 23:11:51 2026 claude
59829     1 Tue Sep  1 20:02:10 2026 claude --continue
  777     1 Tue Sep  1 21:00:00 2026 /Users/k/.grok/bin/grok
  888     1 Tue Sep  1 21:00:00 2026 /usr/libexec/logd
";
        let procs = scan_ps_output(out, 0);
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].pid, 51434);
        assert_eq!(procs[0].harness, Harness::ClaudeCode);
        assert_eq!(procs[2].harness, Harness::Grok);
        assert_eq!(procs[2].argv0, "grok");
    }

    /// WARDEN is often launched FROM a harness session, so it must never scan
    /// itself into the fleet it is drawing.
    #[test]
    fn scan_ps_output_excludes_warden_itself() {
        let out = "4242 1 Tue Sep  1 23:11:51 2026 claude\n";
        assert!(scan_ps_output(out, 4242).is_empty());
    }

    /// The pid-reuse guard: identity is the pair, never the number.
    #[test]
    fn identity_pairs_pid_with_start_time() {
        let out = "900 1 Sun Aug 30 02:10:32 2026 claude\n";
        let a = scan_ps_output(out, 0).remove(0);
        let recycled = "900 1 Tue Sep  1 09:00:00 2026 claude\n";
        let b = scan_ps_output(recycled, 0).remove(0);
        assert_eq!(a.pid, b.pid);
        assert_ne!(a.identity(), b.identity());
    }
}
