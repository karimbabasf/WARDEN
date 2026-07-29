//! The verifier. Pure, offline, and total: no clock, no filesystem, no socket.
//!
//! The format is fixed by `docs/LAUNCH-SPEC.md` section 2 and is produced by
//! `site/api/_lib/license.ts`. Both sides are tested against the SAME
//! `docs/license-vectors.json`, so a drift between producer and consumer fails a
//! test here rather than a customer at first launch.
//!
//! Order of operations matters and is deliberate: decode, verify the signature,
//! and only THEN parse the payload as JSON. The claims are attacker-controlled
//! bytes until the signature says otherwise, so nothing structural is read out
//! of them before that point.

use data_encoding::BASE64URL_NOPAD;
use ed25519_dalek::{Signature, VerifyingKey, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};

use super::pubkey::LICENSE_PUBLIC_KEY;

/// Every key starts with this. Checked exactly, not case-insensitively.
pub const KEY_PREFIX: &str = "WRDN-";

/// The only payload version this build understands.
pub const SUPPORTED_VERSION: u32 = 1;

/// The signed body of a license. Field order here is documentation only: the
/// bytes that were signed are the producer's hand-serialized JSON, which is why
/// the payload is verified as raw bytes and never re-serialized for comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub v: u32,
    pub id: String,
    pub email: String,
    pub seats: u32,
    pub iat: i64,
}

/// Why a key was refused. Three cases, because the activation screen needs to
/// tell "you pasted the wrong thing" apart from "this key is not real".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseError {
    /// Not shaped like a key at all: wrong prefix, missing separator, or a part
    /// that is not base64url of the right length.
    Malformed,
    /// Correctly shaped, but the signature does not verify against the
    /// compiled-in public key. Covers both a forged key and an edited one.
    BadSignature,
    /// The signature is genuine but the payload asks for a format this build
    /// does not implement.
    UnsupportedVersion(u32),
}

impl std::fmt::Display for LicenseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "that does not look like a WARDEN license key"),
            Self::BadSignature => write!(f, "this license key is not valid"),
            Self::UnsupportedVersion(v) => {
                write!(f, "this key is version {v}; this build of WARDEN reads version {SUPPORTED_VERSION}")
            }
        }
    }
}

impl std::error::Error for LicenseError {}

/// The compiled-in signing identity, parsed into a curve point.
///
/// Fallible rather than `.expect()`: a malformed constant here would mean every
/// license on earth fails, and reporting that as a refused key beats crashing on
/// launch. The constant is checked for real by
/// `compiled_in_public_key_matches_the_vectors_file`, which is where a bad
/// regeneration actually gets caught.
fn verifying_key() -> Result<VerifyingKey, LicenseError> {
    VerifyingKey::from_bytes(&LICENSE_PUBLIC_KEY).map_err(|_| LicenseError::BadSignature)
}

/// Split `WRDN-<payload>.<signature>` into the raw signed bytes and the
/// signature. Surrounding whitespace is tolerated because the key arrives by
/// paste, from an email or a success page; nothing inside the key is.
fn split_key(raw: &str) -> Result<(Vec<u8>, Signature), LicenseError> {
    let body = raw
        .trim()
        .strip_prefix(KEY_PREFIX)
        .ok_or(LicenseError::Malformed)?;

    let (payload_b64, sig_b64) = body.split_once('.').ok_or(LicenseError::Malformed)?;
    // Base64url's alphabet has no '.', so a second one means the key was mangled
    // (concatenated, truncated at a line wrap) rather than merely wrong.
    if sig_b64.contains('.') {
        return Err(LicenseError::Malformed);
    }

    let payload = BASE64URL_NOPAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| LicenseError::Malformed)?;
    let sig_bytes: [u8; SIGNATURE_LENGTH] = BASE64URL_NOPAD
        .decode(sig_b64.as_bytes())
        .map_err(|_| LicenseError::Malformed)?
        .try_into()
        .map_err(|_| LicenseError::Malformed)?;

    Ok((payload, Signature::from_bytes(&sig_bytes)))
}

/// Read the claims out of already-verified payload bytes and enforce the version.
///
/// Separate from the signature check so the version gate is reachable by a test
/// without the private key: minting a genuinely-signed `v: 2` key would need the
/// signing seed, which by design exists only in the Vercel env.
fn decode_claims(payload: &[u8]) -> Result<LicenseClaims, LicenseError> {
    let claims: LicenseClaims =
        serde_json::from_slice(payload).map_err(|_| LicenseError::Malformed)?;
    if claims.v != SUPPORTED_VERSION {
        return Err(LicenseError::UnsupportedVersion(claims.v));
    }
    Ok(claims)
}

/// THE gate. A valid Ed25519 signature over the payload, plus `v == 1`, is the
/// whole of it: no expiry, no seat enforcement, no activation call, no clock.
///
/// `verify_strict` rather than `verify`: it additionally rejects small-order
/// public keys and small-order `R` values, which closes the signature
/// malleability holes in the permissive reading of RFC 8032. Every key the
/// producer mints passes it.
pub fn verify_key(raw: &str) -> Result<LicenseClaims, LicenseError> {
    let (payload, signature) = split_key(raw)?;
    verifying_key()?
        .verify_strict(&payload, &signature)
        .map_err(|_| LicenseError::BadSignature)?;
    decode_claims(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    /// The conformance vectors, read from disk AT TEST TIME. Deliberately not
    /// copied into Rust source: the point of the file is that the Node signer and
    /// this verifier are pinned to the same bytes, and a hand-copied duplicate
    /// would let them drift silently.
    fn vectors() -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("docs")
            .join("license-vectors.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&raw).expect("license-vectors.json is valid JSON")
    }

    #[test]
    fn every_valid_vector_is_accepted() {
        let v = vectors();
        let valid = v["valid"].as_array().expect("valid[] present");
        assert_eq!(valid.len(), 5, "expected 5 valid vectors");

        for case in valid {
            let name = case["name"].as_str().expect("vector has a name");
            let key = case["key"].as_str().expect("vector has a key");
            let claims =
                verify_key(key).unwrap_or_else(|e| panic!("{name}: rejected a valid key: {e}"));

            // Not merely "it verified": the decoded claims must equal what the
            // producer says it signed, field for field. That is what catches a
            // payload encoding change that still happens to verify.
            let expected = &case["claims"];
            assert_eq!(
                claims.v,
                expected["v"].as_u64().expect("v") as u32,
                "{name}: v"
            );
            assert_eq!(
                claims.id,
                expected["id"].as_str().expect("id"),
                "{name}: id"
            );
            assert_eq!(
                claims.email,
                expected["email"].as_str().expect("email"),
                "{name}: email"
            );
            assert_eq!(
                claims.seats,
                expected["seats"].as_u64().expect("seats") as u32,
                "{name}: seats"
            );
            assert_eq!(
                claims.iat,
                expected["iat"].as_i64().expect("iat"),
                "{name}: iat"
            );
        }
    }

    #[test]
    fn every_invalid_vector_is_rejected() {
        let v = vectors();
        let invalid = v["invalid"].as_array().expect("invalid[] present");
        assert_eq!(invalid.len(), 3, "expected 3 invalid vectors");

        for case in invalid {
            let name = case["name"].as_str().expect("vector has a name");
            let key = case["key"].as_str().expect("vector has a key");
            assert!(
                verify_key(key).is_err(),
                "{name}: a tampered key was ACCEPTED"
            );
        }
    }

    /// The tampered-payload vector must fail on the SIGNATURE, not by accident on
    /// a parse error. Without this, a verifier that never checked the signature at
    /// all could still pass `every_invalid_vector_is_rejected`.
    #[test]
    fn tampering_is_caught_by_the_signature_and_not_by_luck() {
        let v = vectors();
        let escalated = v["invalid"][0]["key"]
            .as_str()
            .expect("first invalid vector");
        assert_eq!(verify_key(escalated), Err(LicenseError::BadSignature));

        let flipped = v["invalid"][1]["key"]
            .as_str()
            .expect("second invalid vector");
        assert_eq!(verify_key(flipped), Err(LicenseError::BadSignature));

        let no_prefix = v["invalid"][2]["key"]
            .as_str()
            .expect("third invalid vector");
        assert_eq!(verify_key(no_prefix), Err(LicenseError::Malformed));
    }

    /// The one that catches a bad `gen-license-keypair.mjs` run: the committed
    /// Rust constant and the committed vectors file must name the same identity.
    #[test]
    fn compiled_in_public_key_matches_the_vectors_file() {
        let v = vectors();
        let from_file = BASE64URL_NOPAD
            .decode(
                v["publicKey"]
                    .as_str()
                    .expect("publicKey present")
                    .as_bytes(),
            )
            .expect("publicKey is base64url");
        assert_eq!(
            from_file,
            LICENSE_PUBLIC_KEY.to_vec(),
            "src/license/pubkey.rs and docs/license-vectors.json disagree about the signing identity",
        );
        assert!(
            verifying_key().is_ok(),
            "compiled-in key is not a valid curve point"
        );
    }

    #[test]
    fn a_genuinely_signed_key_of_an_unknown_version_is_refused() {
        // Reachable without the signing seed because the version gate is checked
        // over already-verified bytes.
        let payload =
            br#"{"v":2,"id":"cs_test_a1","email":"a@example.com","seats":1,"iat":1785700000}"#;
        assert_eq!(
            decode_claims(payload),
            Err(LicenseError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn malformed_shapes_are_refused_before_any_crypto() {
        let v = vectors();
        let good = v["valid"][0]["key"]
            .as_str()
            .expect("first valid vector")
            .to_string();
        let (_, sig) = good.split_once('.').expect("vector has a separator");

        for (label, key) in [
            ("empty", String::new()),
            ("prefix only", KEY_PREFIX.to_string()),
            ("lowercase prefix", good.replacen("WRDN-", "wrdn-", 1)),
            ("no separator", good.replace('.', "")),
            ("two separators", format!("{good}.{sig}")),
            (
                "payload is not base64url",
                format!("{KEY_PREFIX}not base64!.{sig}"),
            ),
            (
                "signature truncated",
                format!("{KEY_PREFIX}eyJ2IjoxfQ.{}", &sig[..40]),
            ),
        ] {
            assert_eq!(verify_key(&key), Err(LicenseError::Malformed), "{label}");
        }
    }

    /// A key pasted out of an email arrives with a trailing newline more often
    /// than not. Tolerating the surrounding whitespace is a UX decision, not a
    /// weakening: the signature still has to verify over the exact inner bytes.
    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let v = vectors();
        let key = v["valid"][0]["key"].as_str().expect("first valid vector");
        assert!(verify_key(&format!("  {key}\n")).is_ok());
        // But whitespace INSIDE the key is not silently repaired.
        assert!(verify_key(&key.replace('.', ". ")).is_err());
    }
}
