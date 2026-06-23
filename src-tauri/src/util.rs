use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn stable_id(parts: &[&str]) -> String { let mut h=Sha256::new(); for p in parts { h.update(p.as_bytes()); h.update([0]); } hex::encode(&h.finalize()[..16]) }
pub fn hash64(bytes: &[u8]) -> u64 { let digest=Sha256::digest(bytes); u64::from_be_bytes(digest[0..8].try_into().unwrap()) }
pub fn parse_ts(v: Option<&serde_json::Value>) -> DateTime<Utc> { v.and_then(|x| x.as_str()).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc)).unwrap_or_else(Utc::now) }
pub fn expand_tilde(p: &str) -> PathBuf { if let Some(rest)=p.strip_prefix("~/") { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(rest) } else { PathBuf::from(p) } }
pub fn default_db_path() -> PathBuf { std::env::var("WARDEN_DB_PATH").map(|s| expand_tilde(&s)).unwrap_or_else(|_| dirs::home_dir().unwrap().join(".warden/warden.db")) }
pub fn default_claude_projects() -> PathBuf { std::env::var("WARDEN_CLAUDE_PROJECTS").map(|s| expand_tilde(&s)).unwrap_or_else(|_| dirs::home_dir().unwrap().join(".claude/projects")) }
pub fn ensure_parent(path: &Path) -> Result<()> { if let Some(p)=path.parent() { std::fs::create_dir_all(p).with_context(|| format!("create {}",p.display()))?; } Ok(()) }
pub fn repo_root(cwd: &Path) -> Option<PathBuf> { let mut p=cwd.to_path_buf(); loop { if p.join(".git").exists() { return Some(p); } if !p.pop() { return None; } } }
pub fn truncate_chars(s: &str, max: usize) -> String { if s.chars().count() <= max { s.to_string() } else { let mut out=s.chars().take(max.saturating_sub(1)).collect::<String>(); out.push('…'); out } }
