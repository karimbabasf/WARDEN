use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn stable_id(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    hex::encode(&h.finalize()[..16])
}
pub fn hash64(bytes: &[u8]) -> u64 {
    let digest = Sha256::digest(bytes);
    u64::from_be_bytes(digest[0..8].try_into().expect("32-byte SHA-256 digest yields an 8-byte array"))
}
/// Modification time of a file as nanoseconds since the epoch, or `0` when the
/// platform will not say.
///
/// Paired with the file's LENGTH this is the cheap "has anything touched this
/// transcript" oracle the live ingest opens with. It exists because the honest
/// answer used to cost a full read plus a SHA-256 of the whole file, on every
/// filesystem event, for every live transcript: `refresh_live_context` measured
/// 3.1s on a real corpus and almost all of it was re-hashing bytes already parsed.
/// `0` is reserved for "unknown" and never compares equal, so a platform that
/// cannot report an mtime falls back to exactly the old behaviour.
pub fn mtime_nanos(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}
pub fn parse_ts(v: Option<&serde_json::Value>) -> DateTime<Utc> {
    v.and_then(|x| x.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest)
    } else {
        PathBuf::from(p)
    }
}
/// The inverse of [`expand_tilde`]: fold a leading `$HOME` back to `~` for display.
///
/// Every absolute path the harnesses record starts with the user's home directory, so
/// showing one verbatim puts the account name on screen (and, if the radar state is ever
/// transmitted, on the wire). Folding to `~` keeps the path readable and recognisable
/// while dropping the only personally identifying segment. Paths outside home are
/// returned unchanged: they carry no account name to hide.
pub fn display_path(p: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return p.to_string();
    };
    let home = home.to_string_lossy();
    // Guard the empty/`/` home case, where a naive prefix strip would mangle every path.
    if home.is_empty() || home == "/" {
        return p.to_string();
    }
    let home_slash = format!("{home}/");
    match p.strip_prefix(&home_slash) {
        Some(rest) => format!("~/{rest}"),
        None if p == home => "~".to_string(),
        None => p.to_string(),
    }
}

pub fn default_db_path() -> PathBuf {
    std::env::var("WARDEN_DB_PATH")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".warden/warden.db"))
}
/// Path to the user's `~/.warden/config.toml`. Same env-helper shape as
/// `default_db_path`: `WARDEN_CONFIG_PATH` overrides (tests point it at a temp
/// file), otherwise the well-known location next to the database.
pub fn warden_config_path() -> PathBuf {
    std::env::var("WARDEN_CONFIG_PATH")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".warden/config.toml"))
}
/// Path to the host's persistent iroh identity for remote observation.
///
/// It MUST survive restarts: the key is what every issued grant is bound to, so
/// regenerating it on launch would silently kill every grant the user has handed out.
/// Same env-helper shape as `default_db_path`; `WARDEN_OBSERVE_KEY` overrides.
pub fn observe_key_path() -> PathBuf {
    std::env::var("WARDEN_OBSERVE_KEY")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".warden/observer_key"))
}
/// Path to the user's `~/.claude/CLAUDE.md` — the durable Claude Code guidance
/// file that several fix-preview patterns target. `WARDEN_CLAUDE_MD` overrides
/// (tests point it at a temp file).
pub fn claude_md_path() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_MD")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".claude/CLAUDE.md"))
}
pub fn default_claude_projects() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_PROJECTS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".claude/projects"))
}
pub fn default_codex_sessions() -> PathBuf {
    std::env::var("WARDEN_CODEX_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".codex/sessions"))
}
/// Grok CLI session root `~/.grok/sessions`. Layout is
/// `<percent-encoded cwd>/<session uuid>/events.jsonl`, so the WORKING DIRECTORY
/// is carried by the path itself rather than by any record inside the file.
/// `WARDEN_GROK_SESSIONS` overrides (tests point it at a temp dir).
pub fn default_grok_sessions() -> PathBuf {
    std::env::var("WARDEN_GROK_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .expect("home directory should resolve")
                .join(".grok/sessions")
        })
}

/// RADAR: the Claude Code liveness registry directory `~/.claude/sessions`. Each
/// `<pid>.json` records a currently-open session `{pid, sessionId, cwd, …}`.
/// `WARDEN_CLAUDE_SESSIONS` overrides (tests point it at a temp dir). The dir is
/// version-dependent (confirmed on Claude Code v2.1.181); liveness falls back to
/// transcript mtime when it is absent.
pub fn default_claude_sessions_dir() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".claude/sessions"))
}
/// RADAR: transcript-mtime recency window (ms) for the LAST-RESORT liveness fallback —
/// used ONLY when a session has no usable action events at all to read semantically
/// (see `status_from_last_event`). With events present, working/idle now comes from the
/// SHAPE of the last action, not this timer. `WARDEN_RADAR_WORKING_MS` overrides; default
/// 15000ms. (The primary path is event-semantic and needs no window.)
pub fn radar_working_ms() -> u64 {
    std::env::var("WARDEN_RADAR_WORKING_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(15000)
}
/// RADAR: seconds after which a non-archived Codex rollout is treated as abandoned
/// and dropped from the live forest (hybrid stale policy). Codex has no
/// process/termination signal (unlike Claude's PID), so a rollout the user never
/// archived would otherwise linger forever. `WARDEN_RADAR_CODEX_STALE_HRS` overrides
/// (hours, default 6); `0` disables the cutoff. Claude is unaffected — its PID is the
/// hard liveness signal.
pub fn radar_codex_stale_secs() -> u64 {
    std::env::var("WARDEN_RADAR_CODEX_STALE_HRS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(6)
        * 3600
}
/// RADAR (Fault B): how long since a session's LAST ingested event a "working" verdict
/// stays trusted before it is downgraded to idle — the BACKSTOP for the conversation-
/// state liveness rule (last event = an unanswered UserPrompt / in-flight ToolCall ⇒
/// working). `WARDEN_RADAR_WORKING_STALE_SECS` overrides. Generous default (180s) so a
/// long tool run or a slow generation is never mistaken for a stuck agent, while a
/// session that fell silent mid-step still settles to idle instead of glowing forever.
pub fn radar_working_stale_secs() -> u64 {
    std::env::var("WARDEN_RADAR_WORKING_STALE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(180)
}
/// RADAR: how long a subagent may be silent (no transcript writes) while its parent
/// is still alive before it is treated as terminated — a BACKSTOP only; the primary
/// signal is the parent's tool-result for the subagent's call. `WARDEN_RADAR_SUBAGENT_TERMINATE_MS`
/// overrides. Generous default (90s) so a long-running tool call is never mistaken
/// for a finished subagent.
pub fn radar_subagent_terminate_ms() -> u64 {
    std::env::var("WARDEN_RADAR_SUBAGENT_TERMINATE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(90_000)
}

/// RADAR: how long a terminated subagent stays in the emitted forest (status
/// "terminated") so the FACE can play its implode, before it is dropped. Derived
/// from the permanent termination timestamp, so dropping is idempotent.
pub fn radar_terminate_grace_ms() -> u64 {
    std::env::var("WARDEN_RADAR_TERMINATE_GRACE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(5_000)
}

/// How long an in-process TEAMMATE must be quiet before it is treated as finished,
/// and then ONLY while its lead is idle and its own last turn has completed (see
/// `assemble`). A teammate has no tool-result and no PID, so it used to ride the lead's
/// whole lifetime and only clear when the lead PROCESS exited; for a monitor left open
/// all day that meant a finished team lingered forever. The window is the grace on top
/// of the lead-idle + member-idle gate, never the sole signal: a member is quiet between
/// turns while the lead works, which is why the gate, not just the timer, decides.
pub fn radar_teammate_done_ms() -> u64 {
    std::env::var("WARDEN_RADAR_TEAMMATE_DONE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60_000)
}
pub fn default_codex_archived_sessions() -> PathBuf {
    std::env::var("WARDEN_CODEX_ARCHIVED_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().expect("home directory should resolve").join(".codex/archived_sessions"))
}
pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
    }
    Ok(())
}
/// The repo a cwd belongs to, walking up until it finds `.git`.
///
/// A LINKED WORKTREE roots at a `.git` file rather than a directory, so the plain
/// "`.git` exists" walk used to stop there and report the worktree as its own repo.
/// The harnesses now put each session in its own worktree, which made N sessions on
/// one repo look like N unrelated projects to every consumer of this. A worktree is
/// resolved back to the repo it belongs to; everything else is unchanged.
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let mut p = cwd.to_path_buf();
    loop {
        let dot_git = p.join(".git");
        if dot_git.is_dir() {
            return Some(canonical(p));
        }
        if dot_git.is_file() {
            let linked = worktree_main_repo(&p, &dot_git);
            return Some(canonical(linked.unwrap_or(p)));
        }
        if !p.pop() {
            return None;
        }
    }
}

/// Resolve symlinks, keeping the original on failure.
///
/// BOTH branches above go through this or they disagree on macOS: git writes a
/// RESOLVED path into a worktree's `.git` file (`/private/var/x`), while walking up a
/// harness-reported cwd keeps whatever form the harness used (`/var/x`). One repo
/// would then have two spellings, and the worktree grouping this exists for would
/// miss exactly the case it is meant to catch.
fn canonical(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// The main repo behind a linked worktree, whose `.git` file holds
/// `gitdir: <main>/.git/worktrees/<name>` (absolute, or relative to the worktree).
///
/// `None` for any other `.git` file, which is what keeps SUBMODULES pointing at
/// themselves: their gitdir lands in `.git/modules/<name>`, and folding every
/// submodule into its superproject is a different decision from this one.
fn worktree_main_repo(worktree: &Path, dot_git_file: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(dot_git_file).ok()?;
    let target = raw
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?
        .trim();
    let target = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        lexical_normalize(&worktree.join(target))
    };

    // `<main>/.git/worktrees/<name>` → `<main>`: the `.git` ancestor below a
    // `worktrees` segment. Matching `worktrees` first is what excludes `modules`.
    let mut node = target.as_path();
    let mut under_worktrees = false;
    while let Some(parent) = node.parent() {
        match node.file_name().and_then(|n| n.to_str()) {
            Some("worktrees") => under_worktrees = true,
            Some(".git") if under_worktrees => return Some(parent.to_path_buf()),
            _ => {}
        }
        node = parent;
    }
    None
}

/// Resolve `.` and `..` textually, without touching the filesystem or following
/// symlinks. A relative `gitdir` leaves `..` segments behind, and two worktrees of one
/// repo have to produce the SAME path for grouping to work.
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }
    out
}
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max.saturating_sub(1)).collect::<String>();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env-mutating tests so set/unset can't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── claude_md_path ────────────────────────────────────────────────────────
    // Covers the fix-preview target resolution. The forge fix-preview tests were
    // made pure (target injected) to kill a cross-thread `WARDEN_CLAUDE_MD` race,
    // so the override→path behaviour is covered here instead — safely, under the
    // shared ENV_LOCK that serialises every env-mutating test in this module.

    #[test]
    fn claude_md_path_reads_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_CLAUDE_MD", "/tmp/warden-test/CLAUDE.md");
        let result = claude_md_path();
        std::env::remove_var("WARDEN_CLAUDE_MD");
        assert_eq!(result, PathBuf::from("/tmp/warden-test/CLAUDE.md"));
    }

    #[test]
    fn claude_md_path_defaults_under_home_claude_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_CLAUDE_MD");
        let result = claude_md_path();
        assert!(
            result.ends_with(".claude/CLAUDE.md"),
            "expected default under ~/.claude/CLAUDE.md, got {result:?}"
        );
    }

    // ── repo_root ─────────────────────────────────────────────────────────────
    // A linked worktree (`git worktree add`, and what the harnesses now create per
    // session) roots at a `.git` FILE, not a directory, holding
    // `gitdir: <main>/.git/worktrees/<name>`. A plain ".git exists" walk stops there
    // and calls the worktree its own repo, so N worktrees of one repo become N
    // unrelated projects to everything downstream.

    /// `repo_root` resolves symlinks, and a macOS tempdir lives under one
    /// (`/var` to `/private/var`), so expectations are built the same way.
    fn canon(p: &Path) -> Option<PathBuf> {
        Some(std::fs::canonicalize(p).expect("fixture path exists"))
    }

    /// `<root>/main` with a real `.git` dir, plus `<root>/<name>` as a linked
    /// worktree pointing back into it. Returns `(main, worktree)`.
    fn worktree_fixture(root: &Path, name: &str) -> (PathBuf, PathBuf) {
        let main = root.join("main");
        let git_dir = main.join(".git");
        std::fs::create_dir_all(git_dir.join("worktrees").join(name)).expect("main .git");
        let wt = root.join(name);
        std::fs::create_dir_all(&wt).expect("worktree dir");
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", git_dir.join("worktrees").join(name).display()),
        )
        .expect("worktree .git file");
        (main, wt)
    }

    #[test]
    fn repo_root_finds_the_repo_from_a_nested_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main = tmp.path().join("main");
        std::fs::create_dir_all(main.join(".git")).expect("git dir");
        let deep = main.join("src").join("radar");
        std::fs::create_dir_all(&deep).expect("subdir");
        assert_eq!(repo_root(&deep), canon(&main));
    }

    #[test]
    fn repo_root_resolves_a_linked_worktree_to_its_main_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (main, wt) = worktree_fixture(tmp.path(), "feature");
        assert_eq!(
            repo_root(&wt),
            canon(&main),
            "a worktree must report the repo it belongs to, not itself"
        );
    }

    #[test]
    fn repo_root_resolves_a_subdirectory_of_a_worktree_to_its_main_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (main, wt) = worktree_fixture(tmp.path(), "feature");
        let deep = wt.join("src").join("radar");
        std::fs::create_dir_all(&deep).expect("subdir");
        assert_eq!(repo_root(&deep), canon(&main));
    }

    #[test]
    fn two_worktrees_of_one_repo_share_a_repo_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (main, a) = worktree_fixture(tmp.path(), "feature-a");
        let (_, b) = worktree_fixture(tmp.path(), "feature-b");
        assert_eq!(repo_root(&a), repo_root(&b));
        assert_eq!(repo_root(&a), canon(&main));
    }

    #[test]
    fn repo_root_resolves_a_worktree_whose_gitdir_is_relative() {
        // `worktree.useRelativePaths` (and `git worktree add --relative-paths`) writes
        // `gitdir: ../main/.git/worktrees/<name>`. Joining that onto the worktree leaves
        // `..` segments in the path, so two worktrees would produce two different
        // strings for one repo unless the result is normalized.
        let tmp = tempfile::tempdir().expect("tempdir");
        let main = tmp.path().join("main");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("rel")).expect("main");
        let wt = tmp.path().join("rel");
        std::fs::create_dir_all(&wt).expect("worktree dir");
        std::fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/rel\n").expect("git file");
        assert_eq!(repo_root(&wt), canon(&main));
    }

    /// The fixtures above hand-write the `.git` file, which only proves the parser
    /// matches what THIS test believes git writes. This one drives real `git worktree
    /// add` so the belief itself is under test and cannot drift silently.
    #[test]
    fn repo_root_resolves_a_worktree_that_real_git_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).expect("main dir");
        let git = |args: &[&str], cwd: &Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
        };
        let Ok(init) = git(&["init", "-q"], &main) else {
            eprintln!("git unavailable; skipping");
            return;
        };
        assert!(init.status.success(), "git init failed");
        for args in [
            &["commit", "-q", "--allow-empty", "-m", "init"][..],
            &["worktree", "add", "-q", "../feature", "-b", "feature"][..],
        ] {
            let out = git(args, &main).expect("git runs");
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        }

        let wt = tmp.path().join("feature");
        assert!(wt.join(".git").is_file(), "a worktree roots at a .git FILE");
        assert_eq!(
            repo_root(&wt),
            canon(&main),
            "must resolve to the repo git itself reports as the common dir's parent"
        );
    }

    #[test]
    fn repo_root_leaves_a_submodule_pointing_at_itself() {
        // A submodule also roots at a `.git` FILE, but its gitdir goes to
        // `.git/modules/<name>`, not `.git/worktrees/<name>`. Redirecting it would
        // silently fold every submodule into its superproject, which is a different
        // decision from this fix. Only `worktrees` is rerouted.
        let tmp = tempfile::tempdir().expect("tempdir");
        let sup = tmp.path().join("super");
        std::fs::create_dir_all(sup.join(".git").join("modules").join("vendor"))
            .expect("super .git");
        let sub = sup.join("vendor");
        std::fs::create_dir_all(&sub).expect("submodule dir");
        std::fs::write(sub.join(".git"), "gitdir: ../.git/modules/vendor\n").expect("sub .git");
        assert_eq!(repo_root(&sub), canon(&sub));
    }
}
