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
    u64::from_be_bytes(digest[0..8].try_into().unwrap())
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
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".warden/warden.db"))
}
/// Path to the user's `~/.warden/config.toml`. Same env-helper shape as
/// `default_db_path`: `WARDEN_CONFIG_PATH` overrides (tests point it at a temp
/// file), otherwise the well-known location next to the database.
pub fn warden_config_path() -> PathBuf {
    std::env::var("WARDEN_CONFIG_PATH")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".warden/config.toml"))
}
/// Path to the user's `~/.claude/CLAUDE.md` — the durable Claude Code guidance
/// file that several fix-preview patterns target. `WARDEN_CLAUDE_MD` overrides
/// (tests point it at a temp file).
pub fn claude_md_path() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_MD")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".claude/CLAUDE.md"))
}
pub fn default_claude_projects() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_PROJECTS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".claude/projects"))
}
pub fn default_codex_sessions() -> PathBuf {
    std::env::var("WARDEN_CODEX_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".codex/sessions"))
}
/// RADAR: the Claude Code liveness registry directory `~/.claude/sessions`. Each
/// `<pid>.json` records a currently-open session `{pid, sessionId, cwd, …}`.
/// `WARDEN_CLAUDE_SESSIONS` overrides (tests point it at a temp dir). The dir is
/// version-dependent (confirmed on Claude Code v2.1.181); liveness falls back to
/// transcript mtime when it is absent.
pub fn default_claude_sessions_dir() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_SESSIONS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".claude/sessions"))
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
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".codex/archived_sessions"))
}
pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
    }
    Ok(())
}
/// M4 Forge: the sibling directory where per-artifact pre-image backups live,
/// `<target_dir>/.warden-bak`. Beside the target so an atomic rename and the
/// backup share the target's filesystem (no cross-device rename), and namespaced
/// per artifact id by the caller so concurrent applies can't clobber each other.
pub fn backup_dir(target: &Path) -> PathBuf {
    target
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".warden-bak")
}
/// Lowercase hex SHA-256 of `bytes`. The integrity primitive for Forge backups:
/// the pre-image hash recorded at apply time is recomputed and compared at revert
/// time, so a tampered/lost backup refuses to restore. `sha256_hex("")` is the
/// empty-input digest (e2…e855…b934ca495991b7852b855).
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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

// ── Brain engine env helpers ──────────────────────────────────────────────────
// All helpers follow the default_db_path() pattern: prefer canonical
// WARDEN_BRAIN_* vars, then documented OpenAI-compatible fallbacks where the
// provider ecosystem already has a convention.

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Base URL for the OpenAI-compatible brain API. Trailing slash is stripped so
/// callers can append endpoint paths without producing double slashes.
pub fn brain_base_url() -> Option<String> {
    env_nonempty("WARDEN_BRAIN_BASE_URL")
        .or_else(|| env_nonempty("OPENAI_BASE_URL"))
        .or_else(|| env_nonempty("OPENAI_API_BASE"))
        .map(|s| s.trim_end_matches('/').to_string())
}

/// Full URL for the Chat Completions endpoint.
pub fn brain_chat_completions_url() -> Option<String> {
    brain_base_url().map(|base| format!("{base}/chat/completions"))
}

/// API key: prefers `WARDEN_BRAIN_API_KEY`, falls back to `OPENAI_API_KEY`.
pub fn brain_api_key() -> Option<String> {
    env_nonempty("WARDEN_BRAIN_API_KEY").or_else(|| env_nonempty("OPENAI_API_KEY"))
}

/// Model for the diagnose / coach (high-quality) tier.
pub fn brain_diagnose_model() -> String {
    std::env::var("WARDEN_BRAIN_DIAGNOSE_MODEL").unwrap_or_else(|_| "z-ai/glm-5.2".to_string())
}

/// Model for the verify (fast/cheap) tier.
pub fn brain_verify_model() -> String {
    std::env::var("WARDEN_BRAIN_VERIFY_MODEL").unwrap_or_else(|_| "z-ai/glm-5.2".to_string())
}

/// Structured-output mode for OpenAI-compatible hosts.
pub fn brain_structured_output() -> String {
    match env_nonempty("WARDEN_BRAIN_STRUCTURED_OUTPUT")
        .unwrap_or_else(|| "json_object".to_string())
        .as_str()
    {
        "json_schema" => "json_schema".to_string(),
        "prompt" => "prompt".to_string(),
        _ => "json_object".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env-mutating tests so set/unset can't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_brain_env() {
        for key in [
            "WARDEN_BRAIN_BASE_URL",
            "OPENAI_BASE_URL",
            "OPENAI_API_BASE",
            "WARDEN_BRAIN_API_KEY",
            "OPENAI_API_KEY",
            "WARDEN_BRAIN_DIAGNOSE_MODEL",
            "WARDEN_BRAIN_VERIFY_MODEL",
            "WARDEN_BRAIN_STRUCTURED_OUTPUT",
            "WARDEN_BRAIN_EFFORT",
        ] {
            std::env::remove_var(key);
        }
        std::env::remove_var(["SA", "KANA_API_KEY"].concat());
    }

    // ── brain_base_url ────────────────────────────────────────────────────────

    #[test]
    fn brain_base_url_returns_none_without_base_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_base_url();
        assert_eq!(result, None);
    }

    #[test]
    fn brain_base_url_prefers_warden_and_strips_trailing_slash() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_BASE_URL", "https://near.example/v1/");
        std::env::set_var("OPENAI_BASE_URL", "https://openai.example/v1");
        let result = brain_base_url();
        clear_brain_env();
        assert_eq!(result, Some("https://near.example/v1".to_string()));
    }

    #[test]
    fn brain_base_url_falls_back_to_openai_base_then_api_base() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("OPENAI_API_BASE", "https://api-base.example/v1/");
        assert_eq!(
            brain_base_url(),
            Some("https://api-base.example/v1".to_string())
        );

        std::env::set_var("OPENAI_BASE_URL", "https://base.example/v1/");
        let result = brain_base_url();
        clear_brain_env();
        assert_eq!(result, Some("https://base.example/v1".to_string()));
    }

    // ── brain_chat_completions_url ────────────────────────────────────────────

    #[test]
    fn brain_chat_completions_url_returns_none_without_base() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_chat_completions_url();
        assert_eq!(result, None);
    }

    #[test]
    fn brain_chat_completions_url_uses_custom_base() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_BASE_URL", "https://example.test/v2");
        let result = brain_chat_completions_url();
        clear_brain_env();
        assert_eq!(
            result,
            Some("https://example.test/v2/chat/completions".to_string())
        );
    }

    // ── brain_diagnose_model ──────────────────────────────────────────────────

    #[test]
    fn brain_diagnose_model_defaults_to_near_glm_52_slug() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_diagnose_model();
        assert_eq!(result, "z-ai/glm-5.2");
    }

    #[test]
    fn brain_diagnose_model_reads_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_DIAGNOSE_MODEL", "my-model");
        let result = brain_diagnose_model();
        clear_brain_env();
        assert_eq!(result, "my-model");
    }

    // ── brain_verify_model ────────────────────────────────────────────────────

    #[test]
    fn brain_verify_model_defaults_to_near_glm_52_slug() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_verify_model();
        assert_eq!(result, "z-ai/glm-5.2");
    }

    #[test]
    fn brain_verify_model_reads_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_VERIFY_MODEL", "glm-fast");
        let result = brain_verify_model();
        clear_brain_env();
        assert_eq!(result, "glm-fast");
    }

    // ── brain_api_key ─────────────────────────────────────────────────────────

    #[test]
    fn brain_api_key_prefers_warden_over_openai() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_API_KEY", "warden-key");
        std::env::set_var("OPENAI_API_KEY", "openai-key");
        let result = brain_api_key();
        clear_brain_env();
        assert_eq!(result, Some("warden-key".to_string()));
    }

    #[test]
    fn brain_api_key_falls_back_to_openai() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("OPENAI_API_KEY", "openai-key");
        let result = brain_api_key();
        clear_brain_env();
        assert_eq!(result, Some("openai-key".to_string()));
    }

    #[test]
    fn brain_api_key_ignores_legacy_fallback() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var(["SA", "KANA_API_KEY"].concat(), "legacy-key");
        let result = brain_api_key();
        clear_brain_env();
        assert_eq!(result, None);
    }

    #[test]
    fn brain_api_key_returns_none_when_neither_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_api_key();
        assert_eq!(result, None);
    }

    // ── brain_structured_output ───────────────────────────────────────────────

    #[test]
    fn brain_structured_output_defaults_to_json_object() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        let result = brain_structured_output();
        assert_eq!(result, "json_object");
    }

    #[test]
    fn brain_structured_output_accepts_known_modes_and_falls_back_on_invalid() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_brain_env();
        std::env::set_var("WARDEN_BRAIN_STRUCTURED_OUTPUT", "json_schema");
        assert_eq!(brain_structured_output(), "json_schema");
        std::env::set_var("WARDEN_BRAIN_STRUCTURED_OUTPUT", "prompt");
        assert_eq!(brain_structured_output(), "prompt");
        std::env::set_var("WARDEN_BRAIN_STRUCTURED_OUTPUT", "surprise");
        let result = brain_structured_output();
        clear_brain_env();
        assert_eq!(result, "json_object");
    }

    // ── backup_dir / sha256_hex (M4 Forge) ───────────────────────────────────

    #[test]
    fn backup_dir_is_sibling_warden_bak_of_target() {
        let target = PathBuf::from("/tmp/warden-forge/CLAUDE.md");
        assert_eq!(
            backup_dir(&target),
            PathBuf::from("/tmp/warden-forge/.warden-bak")
        );
    }

    #[test]
    fn sha256_hex_of_empty_input_is_known_digest() {
        // The canonical SHA-256 of the empty byte string. Apply against a missing
        // target records exactly this as the pre-image hash.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_changes_with_content() {
        assert_ne!(sha256_hex(b"a"), sha256_hex(b"b"));
        // Deterministic for the same bytes.
        assert_eq!(sha256_hex(b"warden"), sha256_hex(b"warden"));
    }

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
