//! `~/.warden/config.toml` loader/saver. Forward-only and minimal: only the
//! brain engine overrides the Settings UI can write today live here. Reading a
//! field still goes through the `util::brain_*` env helpers at call time; this
//! file is the *persistence* layer the (future) Settings screen writes into, and
//! a thin merge so a partial patch never clobbers unrelated keys.
use crate::util::warden_config_path;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WardenConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brain_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brain_diagnose_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brain_verify_model: Option<String>,
}

/// Load `~/.warden/config.toml`. A missing file (or `WARDEN_CONFIG_PATH`
/// override pointing nowhere) yields defaults — config is always optional.
/// A malformed file also degrades to defaults rather than crashing startup.
pub fn load() -> WardenConfig {
    let path = warden_config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return WardenConfig::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

/// Merge `patch` (a JSON object of the same keys) over the on-disk config and
/// write it back. Only keys present in `patch` are touched; an explicit JSON
/// `null` clears a key. Unknown keys are ignored. Writes are atomic-enough for a
/// single-writer Settings UI: the parent dir is created and the file replaced.
pub fn save(patch: serde_json::Value) -> Result<()> {
    let mut current = load();
    if let Some(obj) = patch.as_object() {
        apply_string_field(obj.get("brain_base_url"), &mut current.brain_base_url);
        apply_string_field(
            obj.get("brain_diagnose_model"),
            &mut current.brain_diagnose_model,
        );
        apply_string_field(
            obj.get("brain_verify_model"),
            &mut current.brain_verify_model,
        );
    }
    let path = warden_config_path();
    crate::util::ensure_parent(&path)?;
    let text = toml::to_string_pretty(&current).context("serialize warden config")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Apply one optional string field from a JSON patch:
/// - absent key → leave the existing value untouched,
/// - JSON `null` → clear the field,
/// - JSON string → set it.
fn apply_string_field(value: Option<&serde_json::Value>, slot: &mut Option<String>) {
    match value {
        None => {}
        Some(serde_json::Value::Null) => *slot = None,
        Some(v) => {
            if let Some(s) = v.as_str() {
                *slot = Some(s.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `warden_config_path()` reads WARDEN_CONFIG_PATH; serialise env-mutating
    // tests so they cannot race each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_config<T>(f: impl FnOnce() -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::env::set_var("WARDEN_CONFIG_PATH", &path);
        let out = f();
        std::env::remove_var("WARDEN_CONFIG_PATH");
        out
    }

    #[test]
    fn missing_file_yields_defaults() {
        with_temp_config(|| {
            assert_eq!(load(), WardenConfig::default());
        });
    }

    #[test]
    fn save_then_load_round_trips_base_url() {
        with_temp_config(|| {
            save(serde_json::json!({ "brain_base_url": "https://example.test/v9" })).unwrap();
            let cfg = load();
            assert_eq!(
                cfg.brain_base_url.as_deref(),
                Some("https://example.test/v9")
            );
            // Untouched keys stay None.
            assert_eq!(cfg.brain_diagnose_model, None);
        });
    }

    #[test]
    fn partial_patch_preserves_other_keys() {
        with_temp_config(|| {
            save(serde_json::json!({ "brain_base_url": "https://a.test" })).unwrap();
            save(serde_json::json!({ "brain_verify_model": "fugu-mini" })).unwrap();
            let cfg = load();
            assert_eq!(cfg.brain_base_url.as_deref(), Some("https://a.test"));
            assert_eq!(cfg.brain_verify_model.as_deref(), Some("fugu-mini"));
        });
    }

    #[test]
    fn explicit_null_clears_a_key() {
        with_temp_config(|| {
            save(serde_json::json!({ "brain_base_url": "https://a.test" })).unwrap();
            save(serde_json::json!({ "brain_base_url": null })).unwrap();
            assert_eq!(load().brain_base_url, None);
        });
    }
}
