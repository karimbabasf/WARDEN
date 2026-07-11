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
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let mut p = cwd.to_path_buf();
    loop {
        if p.join(".git").exists() {
            return Some(p);
        }
        if !p.pop() {
            return None;
        }
    }
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
}
