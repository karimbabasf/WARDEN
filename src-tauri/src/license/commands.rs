//! The two commands the activation screen talks to.
//!
//! They live here rather than in the crate-level `commands.rs` so the entire
//! gate, including its IPC surface, is one directory wide and the canary in
//! `gate.rs` can read all of it.

use serde::Serialize;

use super::gate::{self, GateStatus};
use super::store;
use super::verify::verify_key;

/// What the frontend needs to decide between the activation screen and the
/// radar. The buyer's own email is echoed back so the activation screen can
/// confirm which purchase unlocked this Mac; nothing else about the key is
/// exposed and nothing leaves the machine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    /// True when the app may run: a verified key, or a debug build.
    pub activated: bool,
    /// False in a debug build. Lets the dev harness say so out loud instead of
    /// looking like a silently activated release.
    pub gated: bool,
    pub email: Option<String>,
    pub seats: Option<u32>,
}

impl From<&GateStatus> for LicenseStatus {
    fn from(status: &GateStatus) -> Self {
        Self {
            activated: status.is_open(),
            gated: gate::GATE_ENABLED,
            email: status.claims().map(|c| c.email.clone()),
            seats: status.claims().map(|c| c.seats),
        }
    }
}

/// Read the gate. Called once on boot, before anything else is mounted.
#[tauri::command]
pub fn license_status() -> LicenseStatus {
    LicenseStatus::from(&gate::status())
}

/// Verify a pasted key and, if it holds up, persist it.
///
/// Verification comes first and storage second, so a bad paste can never leave a
/// junk file behind for the next launch to trip over. The error string is the
/// verifier's own `Display`, which is written for the person reading it.
#[tauri::command]
pub fn license_activate(key: String) -> Result<LicenseStatus, String> {
    let claims = verify_key(&key).map_err(|e| e.to_string())?;
    store::write(key.trim()).map_err(|e| format!("could not save the license: {e}"))?;
    Ok(LicenseStatus {
        activated: true,
        gated: gate::GATE_ENABLED,
        email: Some(claims.email),
        seats: Some(claims.seats),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::verify::LicenseClaims;

    fn claims() -> LicenseClaims {
        LicenseClaims {
            v: 1,
            id: "cs_test_a1".into(),
            email: "a@example.com".into(),
            seats: 2,
            iat: 1_785_700_000,
        }
    }

    #[test]
    fn an_activated_status_carries_the_buyer_back_to_the_screen() {
        let s = LicenseStatus::from(&GateStatus::Activated(claims()));
        assert!(s.activated);
        assert_eq!(s.email.as_deref(), Some("a@example.com"));
        assert_eq!(s.seats, Some(2));
    }

    #[test]
    fn a_locked_status_reveals_nothing() {
        let s = LicenseStatus::from(&GateStatus::Locked);
        assert!(!s.activated);
        assert_eq!(s.email, None);
        assert_eq!(s.seats, None);
    }

    #[test]
    fn a_dev_bypass_is_open_but_anonymous() {
        let s = LicenseStatus::from(&GateStatus::DevBypass);
        assert!(s.activated);
        assert_eq!(s.email, None, "a dev build must not invent an owner");
    }

    #[test]
    fn the_status_serializes_as_camel_case_for_the_frontend() {
        let json = serde_json::to_string(&LicenseStatus::from(&GateStatus::Locked))
            .expect("status serializes");
        assert!(json.contains(r#""activated":false"#), "{json}");
        assert!(json.contains(r#""gated":"#), "{json}");
    }

    /// A refused key returns a message a human can act on, and never the raw
    /// bytes they pasted (which would put a stray key into a log).
    #[test]
    fn a_bad_key_is_refused_with_a_readable_message() {
        let err = license_activate("nonsense".to_string()).expect_err("should refuse");
        assert_eq!(err, "that does not look like a WARDEN license key");
        assert!(!err.contains("nonsense"));
    }

    #[test]
    fn a_forged_key_is_refused_before_anything_is_written() {
        let forged = "WRDN-eyJ2IjoxLCJpZCI6ImNzX3Rlc3RfYTEiLCJlbWFpbCI6ImFAZXhhbXBsZS5jb20iLCJzZWF0cyI6MywiaWF0IjoxNzg1NzAwMDAwfQ.xUaQMiW3AG1BBTQpBv9zgyb4pevwZL93titEFUQMPVxMGLVfUh4gAplgimsoLMS6qSVdui5hen6wkhtzA8LABQ";
        let err = license_activate(forged.to_string()).expect_err("should refuse");
        assert_eq!(err, "this license key is not valid");
    }
}
