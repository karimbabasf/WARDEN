use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
pub fn default_claude_projects() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_PROJECTS")
        .map(|s| expand_tilde(&s))
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".claude/projects"))
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

// ── Brain engine env helpers ──────────────────────────────────────────────────
// All helpers follow the default_db_path() pattern: prefer the env var, fall
// back to a hardcoded default.  Default behaviour (no env vars set) is
// byte-identical to the previous hardcoded literals in brain.rs.

/// Base URL for the Fugu/brain API.  Trailing slash is stripped so that
/// callers can safely append `/path` without producing double slashes.
pub fn brain_base_url() -> String {
    std::env::var("WARDEN_BRAIN_BASE_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://api.sakana.ai/v1".to_string())
}

/// Full URL for the `/responses` endpoint.
pub fn brain_responses_url() -> String {
    format!("{}/responses", brain_base_url())
}

/// API key: prefers `WARDEN_BRAIN_API_KEY`, falls back to `SAKANA_API_KEY`.
pub fn brain_api_key() -> Option<String> {
    std::env::var("WARDEN_BRAIN_API_KEY")
        .ok()
        .or_else(|| std::env::var("SAKANA_API_KEY").ok())
}

/// Model for the diagnose / coach (high-quality) tier.
pub fn brain_diagnose_model() -> String {
    std::env::var("WARDEN_BRAIN_DIAGNOSE_MODEL")
        .unwrap_or_else(|_| "fugu-ultra".to_string())
}

/// Model for the verify (fast/cheap) tier.
pub fn brain_verify_model() -> String {
    std::env::var("WARDEN_BRAIN_VERIFY_MODEL")
        .unwrap_or_else(|_| "fugu".to_string())
}

/// Effort string for the reasoning field.
/// When `WARDEN_BRAIN_EFFORT` is set it overrides both tiers.
/// Otherwise: high-tier → "xhigh", low-tier → "high".
pub fn brain_effort(high_tier: bool) -> String {
    std::env::var("WARDEN_BRAIN_EFFORT").unwrap_or_else(|_| {
        if high_tier {
            "xhigh".to_string()
        } else {
            "high".to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env-mutating tests so set/unset can't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── brain_base_url ────────────────────────────────────────────────────────

    #[test]
    fn brain_base_url_defaults_to_sakana() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        let result = brain_base_url();
        assert_eq!(result, "https://api.sakana.ai/v1");
    }

    #[test]
    fn brain_base_url_reads_env_and_strips_trailing_slash() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_BASE_URL", "https://example.test/v2/");
        let result = brain_base_url();
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        assert_eq!(result, "https://example.test/v2");
    }

    // ── brain_responses_url ───────────────────────────────────────────────────

    #[test]
    fn brain_responses_url_default_is_sakana_responses() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        let result = brain_responses_url();
        assert_eq!(result, "https://api.sakana.ai/v1/responses");
    }

    #[test]
    fn brain_responses_url_uses_custom_base() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_BASE_URL", "https://example.test/v2");
        let result = brain_responses_url();
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        assert_eq!(result, "https://example.test/v2/responses");
    }

    // ── brain_diagnose_model ──────────────────────────────────────────────────

    #[test]
    fn brain_diagnose_model_defaults_to_fugu_ultra() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_DIAGNOSE_MODEL");
        let result = brain_diagnose_model();
        assert_eq!(result, "fugu-ultra");
    }

    #[test]
    fn brain_diagnose_model_reads_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_DIAGNOSE_MODEL", "my-model");
        let result = brain_diagnose_model();
        std::env::remove_var("WARDEN_BRAIN_DIAGNOSE_MODEL");
        assert_eq!(result, "my-model");
    }

    // ── brain_verify_model ────────────────────────────────────────────────────

    #[test]
    fn brain_verify_model_defaults_to_fugu() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_VERIFY_MODEL");
        let result = brain_verify_model();
        assert_eq!(result, "fugu");
    }

    #[test]
    fn brain_verify_model_reads_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_VERIFY_MODEL", "fugu-lite");
        let result = brain_verify_model();
        std::env::remove_var("WARDEN_BRAIN_VERIFY_MODEL");
        assert_eq!(result, "fugu-lite");
    }

    // ── brain_api_key ─────────────────────────────────────────────────────────

    #[test]
    fn brain_api_key_prefers_warden_over_sakana() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_API_KEY", "warden-key");
        std::env::set_var("SAKANA_API_KEY", "sakana-key");
        let result = brain_api_key();
        std::env::remove_var("WARDEN_BRAIN_API_KEY");
        std::env::remove_var("SAKANA_API_KEY");
        assert_eq!(result, Some("warden-key".to_string()));
    }

    #[test]
    fn brain_api_key_falls_back_to_sakana() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_API_KEY");
        std::env::set_var("SAKANA_API_KEY", "sakana-key");
        let result = brain_api_key();
        std::env::remove_var("SAKANA_API_KEY");
        assert_eq!(result, Some("sakana-key".to_string()));
    }

    #[test]
    fn brain_api_key_returns_none_when_neither_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_API_KEY");
        std::env::remove_var("SAKANA_API_KEY");
        let result = brain_api_key();
        assert_eq!(result, None);
    }

    // ── brain_effort ──────────────────────────────────────────────────────────

    #[test]
    fn brain_effort_high_tier_default_is_xhigh() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_EFFORT");
        let result = brain_effort(true);
        assert_eq!(result, "xhigh");
    }

    #[test]
    fn brain_effort_low_tier_default_is_high() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_BRAIN_EFFORT");
        let result = brain_effort(false);
        assert_eq!(result, "high");
    }

    #[test]
    fn brain_effort_reads_env_override_for_both_tiers() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_BRAIN_EFFORT", "low");
        let high = brain_effort(true);
        let low = brain_effort(false);
        std::env::remove_var("WARDEN_BRAIN_EFFORT");
        assert_eq!(high, "low");
        assert_eq!(low, "low");
    }
}
