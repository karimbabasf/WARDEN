//! Observer grants: the whole credential lifecycle for remote read-only observation.
//!
//! Three things live here on purpose, because a reviewer auditing "is this token scheme
//! sound?" should not have to read three files: the wire format of the token, the hash
//! that turns a secret into a stored verifier, and the SQL that claims a grant exactly
//! once.
//!
//! Two properties are load-bearing and everything else is bookkeeping:
//!
//! 1. **The host never stores the secret.** A stolen `warden.db` yields verifiers, and a
//!    verifier is not a bearer credential: it cannot be replayed at [`GrantStore::redeem`]
//!    because redemption hashes what it is given and compares the result.
//! 2. **Redemption is one atomic statement whose row count is the verdict.** Reading the
//!    state into Rust and then writing it back is the race that turns "single use" into
//!    "single use most of the time". See [`GrantStore::redeem`].

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
// rand 0.10 renamed the core trait: what was `RngCore` is now `Rng`, and the old `Rng`
// extension trait is `RngExt`. `fill_bytes` lives on this one.
use rand::Rng;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::observe::projection::Profile;
use crate::store::Store;

/// Version marker for the token string. A future scheme bumps this rather than trying to
/// stay compatible, so an old client fails to parse instead of misinterpreting bytes.
pub const TOKEN_PREFIX: &str = "warden-obs-v1.";

/// `endpoint_id[32] || secret[32] || exp_unix_u32[4]`.
const TOKEN_PAYLOAD_LEN: usize = 68;
const ENDPOINT_ID_LEN: usize = 32;
pub const SECRET_LEN: usize = 32;

/// Bounds on a grant's lifetime, in seconds. The design offers 5 min / 15 min / 1 hour;
/// these are the hard rails around whatever the UI sends, so a bad `ttl_secs` from a
/// frontend bug cannot mint a credential that outlives the machine.
const MIN_TTL_SECS: u64 = 60;
const MAX_TTL_SECS: u64 = 3600;

/// RFC 4648 base32, lowercase, no padding.
///
/// The standard `BASE32_NOPAD` encoding is uppercase; a 109-character uppercase blob is
/// harder to read and invites transcription errors. The alphabet is spelled out here so
/// the wire format is auditable in one place. `check_trailing_bits` defaults to on, which
/// matters: 68 bytes is 544 bits and 109 symbols carry 545, so the spare bit must be zero
/// and a non-canonical final character is rejected rather than silently accepted.
static BASE32_LOWER: Lazy<data_encoding::Encoding> = Lazy::new(|| {
    let mut spec = data_encoding::Specification::new();
    spec.symbols.push_str("abcdefghijklmnopqrstuvwxyz234567");
    spec.encoding()
        .expect("static lowercase base32 specification is valid")
});

/// Why a token string was refused. An enum rather than a string so tests assert on the
/// actual reason instead of on prose that can drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Missing or wrong `warden-obs-v1.` prefix.
    BadPrefix,
    /// Not decodable as lowercase base32, or non-zero trailing bits.
    BadBase32,
    /// Decoded, but not exactly 68 bytes.
    BadLength,
    /// The secret decoded to all zeroes, which is what an unfilled buffer looks like.
    ZeroSecret,
    /// The endpoint id decoded to all zeroes, which is not a public key.
    ZeroEndpointId,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TokenError::BadPrefix => "not a warden-obs-v1 token",
            TokenError::BadBase32 => "token is corrupted",
            TokenError::BadLength => "token is truncated or padded",
            TokenError::ZeroSecret => "token carries an empty secret",
            TokenError::ZeroEndpointId => "token carries an empty host id",
        };
        f.write_str(s)
    }
}

impl std::error::Error for TokenError {}

/// A parsed or freshly minted observer token.
///
/// The secret is wrapped in [`Zeroizing`] so it is wiped when this value drops, rather
/// than being left in a freed heap page for whatever allocates next.
pub struct ObserverToken {
    /// The host's iroh `EndpointId` (an ed25519 public key). Pins the host's identity:
    /// the QUIC handshake completes only against the matching private key.
    pub endpoint_id: [u8; ENDPOINT_ID_LEN],
    pub secret: Zeroizing<[u8; SECRET_LEN]>,
    /// A display hint so an observer's UI can say "expired" without a round trip. NEVER
    /// authoritative: the host's stored `expires_at` is the only expiry that decides
    /// anything, because this field is under the attacker's control.
    pub exp_hint: u32,
}

/// Hand-written rather than derived, so the secret cannot reach a log line, a panic
/// message, or a test failure dump. A derived `Debug` here would be a credential leak with
/// no code change required to trigger it.
impl std::fmt::Debug for ObserverToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObserverToken")
            .field("endpoint_id", &hex::encode(self.endpoint_id))
            .field("secret", &"<redacted>")
            .field("exp_hint", &self.exp_hint)
            .finish()
    }
}

impl ObserverToken {
    /// Mint a token for `endpoint_id` expiring at `expires_at` (unix seconds), drawing a
    /// fresh 256-bit secret from the OS CSPRNG.
    pub fn mint(endpoint_id: [u8; ENDPOINT_ID_LEN], expires_at: i64) -> Result<Self> {
        let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
        rand::rng().fill_bytes(secret.as_mut_slice());
        // A CSPRNG cannot plausibly return 32 zero bytes, so this fires only when the
        // buffer was never filled. Failing closed here is the difference between a broken
        // RNG being an outage and it being an authentication bypass.
        if secret.iter().all(|b| *b == 0) {
            anyhow::bail!("CSPRNG returned an all-zero secret");
        }
        Ok(Self {
            endpoint_id,
            secret,
            exp_hint: expires_at.clamp(0, u32::MAX as i64) as u32,
        })
    }

    pub fn encode(&self) -> String {
        let mut payload = Vec::with_capacity(TOKEN_PAYLOAD_LEN);
        payload.extend_from_slice(&self.endpoint_id);
        payload.extend_from_slice(self.secret.as_slice());
        payload.extend_from_slice(&self.exp_hint.to_be_bytes());
        format!("{TOKEN_PREFIX}{}", BASE32_LOWER.encode(&payload))
    }

    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        let body = raw
            .trim()
            .strip_prefix(TOKEN_PREFIX)
            .ok_or(TokenError::BadPrefix)?;
        // Deliberate normalization, not leniency in the decoder: some chat clients
        // capitalize, and the alphabet itself stays strictly lowercase.
        let body = body.to_ascii_lowercase();
        let payload = BASE32_LOWER
            .decode(body.as_bytes())
            .map_err(|_| TokenError::BadBase32)?;
        if payload.len() != TOKEN_PAYLOAD_LEN {
            return Err(TokenError::BadLength);
        }

        let mut endpoint_id = [0u8; ENDPOINT_ID_LEN];
        endpoint_id.copy_from_slice(&payload[..ENDPOINT_ID_LEN]);
        let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
        secret.copy_from_slice(&payload[ENDPOINT_ID_LEN..ENDPOINT_ID_LEN + SECRET_LEN]);
        let exp_hint = u32::from_be_bytes([payload[64], payload[65], payload[66], payload[67]]);

        if secret.iter().all(|b| *b == 0) {
            return Err(TokenError::ZeroSecret);
        }
        if endpoint_id.iter().all(|b| *b == 0) {
            return Err(TokenError::ZeroEndpointId);
        }
        Ok(Self {
            endpoint_id,
            secret,
            exp_hint,
        })
    }
}

/// What the host stores in place of the secret.
///
/// The design specifies blake3; this uses sha2, already a dependency of this crate for
/// content hashing, rather than pulling a crate in for one hash. The property needed is
/// preimage resistance over 256 bits of input entropy, which sha256 provides, and the
/// speed difference is irrelevant at a handful of redemptions per day. The prefix is
/// domain separation, so this digest can never collide with some other sha256 in the
/// codebase computed over the same bytes.
pub fn verifier_of(secret: &[u8; SECRET_LEN]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"warden-obs-v1/verifier");
    h.update(secret);
    h.finalize().into()
}

/// The public handle for a grant: the first 8 bytes of its verifier.
pub fn token_id_of(verifier: &[u8; 32]) -> [u8; 8] {
    let mut id = [0u8; 8];
    id.copy_from_slice(&verifier[..8]);
    id
}

/// Per-grant salt for [`crate::observe::project_state`], derived rather than stored.
///
/// Derived from the verifier so it needs no column and cannot drift out of sync with the
/// grant. Its job is to stop two DIFFERENT observers correlating the same session across
/// grants; that an observer could derive their own salt is irrelevant, since knowing your
/// own salt tells you nothing about anyone else's.
pub fn salt_of(verifier: &[u8; 32]) -> [u8; 16] {
    let mut h = Sha256::new();
    h.update(b"warden-obs-v1/salt");
    h.update(verifier);
    let out = h.finalize();
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&out[..16]);
    salt
}

/// The lifecycle of a grant ROW. Note what this enum deliberately does not encode:
/// `Redeemed` means a peer proved possession of the token, NOT that the host allowed them
/// in. Approval is a second, separate durable fact (`GrantRecord::approved_at`), because
/// the peer drives redemption and only the host drives approval. Anything that decides
/// access must check both, which is why [`GrantStore::active_endpoints`] and
/// [`GrantStore::grant_for_endpoint`] are the only two readers of that pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    Pending,
    Redeemed,
    Revoked,
    Expired,
}

impl GrantState {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantState::Pending => "pending",
            GrantState::Redeemed => "redeemed",
            GrantState::Revoked => "revoked",
            GrantState::Expired => "expired",
        }
    }
    /// An unrecognised state reads as `Revoked`, the closed position. A corrupted row must
    /// never fail open into something that grants access.
    fn from_db(s: &str) -> Self {
        match s {
            "pending" => GrantState::Pending,
            "redeemed" => GrantState::Redeemed,
            "expired" => GrantState::Expired,
            _ => GrantState::Revoked,
        }
    }
}

/// One grant as the host's management tab sees it. Carries no secret and no verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRecord {
    /// Hex of `token_id`. Safe to show and to pass back over IPC.
    pub grant_id: String,
    pub label: String,
    pub state: GrantState,
    pub created_at: i64,
    pub expires_at: i64,
    pub redeemed_by: Option<[u8; 32]>,
    pub redeemed_at: Option<i64>,
    /// When the HOST approved this observer, or `None` if it never did. Redemption alone
    /// sets this to `None`, so a grant claimed by a peer the host never answered for (the
    /// app quit or crashed inside the approval prompt) is durably unapproved and cannot be
    /// resurrected on the next launch.
    pub approved_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub profile: Profile,
}

/// The outcome of one redemption attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedeemOutcome {
    /// Claimed. Exactly one caller can ever see this for a given token.
    Claimed(Box<GrantRecord>),
    /// No such grant, or the verifier did not match. One variant for both on purpose: a
    /// distinct "wrong secret" answer would confirm that a brute-forced `token_id` names
    /// a real grant.
    Unknown,
    /// The grant exists but is no longer `pending` (already used, or revoked).
    NotPending,
    /// The grant exists and is pending, but the host's clock says it has expired.
    Expired,
}

/// Grant persistence. Wraps [`Store`] but exposes only grant operations: the transport is
/// handed one of these and therefore has no route to sessions, turns, or events.
#[derive(Clone)]
pub struct GrantStore {
    store: Store,
}

impl GrantStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Mint a grant and return both the row and the token string.
    ///
    /// The token string is the ONLY time the secret exists outside the caller's stack; it
    /// is never stored and therefore never re-retrievable. The caller shows it once.
    pub fn create(
        &self,
        endpoint_id: [u8; ENDPOINT_ID_LEN],
        label: &str,
        ttl_secs: u64,
        profile: Profile,
        now: i64,
    ) -> Result<(GrantRecord, String)> {
        let ttl = ttl_secs.clamp(MIN_TTL_SECS, MAX_TTL_SECS) as i64;
        let expires_at = now.saturating_add(ttl);
        let token = ObserverToken::mint(endpoint_id, expires_at)?;
        let verifier = verifier_of(&token.secret);
        let token_id = token_id_of(&verifier);
        let label = crate::util::truncate_chars(label.trim(), 64);
        let profile_str =
            serde_json::to_string(&profile).context("serialize grant profile")?;

        self.store.with_conn(|c| {
            c.execute(
                "INSERT INTO observer_grants(token_id,verifier,friend_label,created_at,expires_at,state,redeemed_by,redeemed_at,profile,last_seen_at,approved_at) \
                 VALUES(?,?,?,?,?,'pending',NULL,NULL,?,NULL,NULL)",
                params![
                    token_id.as_slice(),
                    verifier.as_slice(),
                    label,
                    now,
                    expires_at,
                    profile_str
                ],
            )
            .context("insert observer grant")?;
            Ok(())
        })?;

        Ok((
            GrantRecord {
                grant_id: hex::encode(token_id),
                label,
                state: GrantState::Pending,
                created_at: now,
                expires_at,
                redeemed_by: None,
                redeemed_at: None,
                approved_at: None,
                last_seen_at: None,
                profile,
            },
            token.encode(),
        ))
    }

    /// Claim a grant for `observer`, single use.
    ///
    /// Claiming grants NOTHING on its own. The row lands in `redeemed` with `approved_at`
    /// still NULL, which every access check reads as denied, and only [`Self::approve`]
    /// clears it. The peer drives this call, so if it were also the thing that authorised
    /// access then a host that never answered the prompt (quit, crash, power loss inside
    /// the approval deadline) would leave behind a row that grants access forever.
    ///
    /// The verifier is compared in constant time via [`subtle`] before the claim, because
    /// `token_id` is only 64 bits: without this check an attacker who brute-forced a
    /// matching `token_id` would redeem without ever knowing the secret. The comparison
    /// cannot race, since a verifier never changes after insert.
    ///
    /// The claim itself is one `UPDATE` and its row count is the verdict. Two concurrent
    /// redemptions of the same token contend on SQLite's write lock and exactly one comes
    /// back with `rows == 1`.
    pub fn redeem(
        &self,
        secret: &[u8; SECRET_LEN],
        observer: [u8; 32],
        now: i64,
    ) -> Result<RedeemOutcome> {
        let verifier = verifier_of(secret);
        let token_id = token_id_of(&verifier);

        self.store.with_conn(|c| {
            let row: Option<(Vec<u8>, String, i64)> = c
                .query_row(
                    "SELECT verifier,state,expires_at FROM observer_grants WHERE token_id=?",
                    params![token_id.as_slice()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .context("look up observer grant")?;

            let Some((stored_verifier, state, expires_at)) = row else {
                return Ok(RedeemOutcome::Unknown);
            };
            if stored_verifier.ct_eq(&verifier).unwrap_u8() != 1 {
                return Ok(RedeemOutcome::Unknown);
            }
            if GrantState::from_db(&state) != GrantState::Pending {
                return Ok(RedeemOutcome::NotPending);
            }
            if expires_at <= now {
                return Ok(RedeemOutcome::Expired);
            }

            // THE claim. Everything above is a courtesy that produces a better error
            // message; this statement is what actually enforces single use, and it
            // re-checks state and expiry so nothing decided above can go stale.
            //
            // `approved_at` is cleared explicitly rather than left alone: a pending row can
            // only ever hold NULL there, so this is belt and braces, but it means the claim
            // statement itself states the invariant that a fresh claim is never approved.
            let rows = c
                .execute(
                    "UPDATE observer_grants SET state='redeemed', redeemed_by=?, redeemed_at=?, approved_at=NULL \
                     WHERE token_id=? AND state='pending' AND expires_at>?",
                    params![observer.as_slice(), now, token_id.as_slice(), now],
                )
                .context("claim observer grant")?;
            if rows != 1 {
                // Lost the race, or the row moved out of `pending` between the read and
                // the write. Either way this caller did not claim it.
                return Ok(RedeemOutcome::NotPending);
            }

            let record = read_grant(c, &token_id)?.context("claimed grant vanished")?;
            Ok(RedeemOutcome::Claimed(Box::new(record)))
        })
    }

    /// Record the host's approval of a redeemed grant, durably.
    ///
    /// This is the ONLY statement in the crate that turns a claimed grant into an allowed
    /// one, so it is the single place the "default deny" rule has to hold. Returns whether
    /// the grant is approved once this call returns, which is not the same as whether this
    /// call did the writing:
    ///
    /// * revoked while the host was deciding: no row matches, the grant is not approved,
    ///   and the caller must refuse the connection rather than trust its stale read.
    /// * a second connection from the same peer racing the first: the row is already
    ///   approved, nothing to write, and the answer is still yes.
    pub fn approve(&self, grant_id: &str, now: i64) -> Result<bool> {
        let token_id = decode_grant_id(grant_id)?;
        self.store.with_conn(|c| {
            c.execute(
                "UPDATE observer_grants SET approved_at=? \
                 WHERE token_id=? AND state='redeemed' AND approved_at IS NULL",
                params![now, token_id.as_slice()],
            )
            .context("approve observer grant")?;
            // Re-read rather than trust the row count: the answer we owe the caller is the
            // persisted state, not whether this particular statement was the one to set it.
            let Some(record) = read_grant(c, &token_id)? else {
                return Ok(false);
            };
            Ok(record.state == GrantState::Redeemed && record.approved_at.is_some())
        })
    }

    /// Revoke every grant that was claimed but never approved. Returns the number killed.
    ///
    /// Run at host startup. A redeemed row with no `approved_at` is the fingerprint of a
    /// process that died between the claim and the host's answer, so its peer was never
    /// allowed in by anyone. Revoking rather than reverting to `pending` is deliberate: the
    /// token was spent, and a second attempt on it must fail, not succeed quietly. The
    /// friend gets a fresh token, which costs one message and is the safe direction.
    pub fn revoke_unapproved(&self) -> Result<usize> {
        self.store.with_conn(|c| {
            c.execute(
                "UPDATE observer_grants SET state='revoked' \
                 WHERE state='redeemed' AND approved_at IS NULL",
                [],
            )
            .context("revoke unapproved grants")
        })
    }

    pub fn list(&self) -> Result<Vec<GrantRecord>> {
        self.store.with_conn(|c| {
            let mut st = c
                .prepare(
                    "SELECT token_id,friend_label,state,created_at,expires_at,redeemed_by,redeemed_at,last_seen_at,profile,approved_at \
                     FROM observer_grants ORDER BY created_at DESC",
                )
                .context("prepare grant list")?;
            let rows = st
                .query_map([], row_to_record)
                .context("query grant list")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("read grant list")
        })
    }

    /// Revoke a grant. Returns the observer endpoint bound to it, if any, so the caller
    /// can close the live connection. Revoking an already-revoked grant is a no-op.
    pub fn revoke(&self, grant_id: &str) -> Result<Option<[u8; 32]>> {
        let token_id = decode_grant_id(grant_id)?;
        self.store.with_conn(|c| {
            let existing = read_grant(c, &token_id)?;
            let Some(existing) = existing else {
                return Ok(None);
            };
            c.execute(
                "UPDATE observer_grants SET state='revoked' WHERE token_id=?",
                params![token_id.as_slice()],
            )
            .context("revoke observer grant")?;
            Ok(existing.redeemed_by)
        })
    }

    /// Flip stale `pending` rows to `expired` so the management tab reads correctly.
    /// Cosmetic only: [`Self::redeem`] never trusts `state` alone, it re-checks
    /// `expires_at` in the same statement that claims the row.
    pub fn sweep_expired(&self, now: i64) -> Result<usize> {
        self.store.with_conn(|c| {
            c.execute(
                "UPDATE observer_grants SET state='expired' WHERE state='pending' AND expires_at<=?",
                params![now],
            )
            .context("sweep expired grants")
        })
    }

    /// Every observer endpoint currently allowed to receive frames.
    ///
    /// An APPROVED grant stays valid past `expires_at`: the token expires, the grant does
    /// not. That is what lets a friend keep watching for an hour, and what lets a
    /// reconnect after a wifi drop re-authenticate by public key with no token involved.
    ///
    /// `approved_at IS NOT NULL` is what keeps that generosity from applying to a grant the
    /// host never said yes to. Since this seeds the in-memory allow set at startup, an
    /// unapproved row reaching this query would promote a peer to permanent observer with
    /// no prompt, no token, and no way for the host to notice.
    pub fn active_endpoints(&self) -> Result<Vec<[u8; 32]>> {
        self.store.with_conn(|c| {
            let mut st = c
                .prepare("SELECT redeemed_by FROM observer_grants WHERE state='redeemed' AND approved_at IS NOT NULL AND redeemed_by IS NOT NULL")
                .context("prepare active endpoints")?;
            let rows = st
                .query_map([], |r| r.get::<_, Vec<u8>>(0))
                .context("query active endpoints")?;
            let mut out = Vec::new();
            for row in rows {
                if let Some(id) = to_endpoint_id(&row.context("read active endpoint")?) {
                    out.push(id);
                }
            }
            Ok(out)
        })
    }

    /// The approved grant bound to `observer`, if it is still active.
    ///
    /// The transport calls this to resolve a reconnect that skips the token, so the same
    /// `approved_at IS NOT NULL` gate as [`Self::active_endpoints`] applies: a claimed but
    /// unapproved grant must not be able to answer "who is this peer" either.
    pub fn grant_for_endpoint(&self, observer: &[u8; 32]) -> Result<Option<GrantRecord>> {
        self.store.with_conn(|c| {
            let token_id: Option<Vec<u8>> = c
                .query_row(
                    "SELECT token_id FROM observer_grants \
                     WHERE state='redeemed' AND approved_at IS NOT NULL AND redeemed_by=? \
                     ORDER BY redeemed_at DESC LIMIT 1",
                    params![observer.as_slice()],
                    |r| r.get(0),
                )
                .optional()
                .context("look up grant by endpoint")?;
            let Some(token_id) = token_id else {
                return Ok(None);
            };
            let mut id = [0u8; 8];
            if token_id.len() != 8 {
                return Ok(None);
            }
            id.copy_from_slice(&token_id);
            read_grant(c, &id)
        })
    }

    /// The projection salt for one grant.
    ///
    /// Read from the stored verifier rather than kept in memory: the host does not have
    /// the secret, and deriving it here means the salt cannot drift from the grant it
    /// belongs to.
    pub fn salt_for(&self, token_id: &[u8; 8]) -> Result<[u8; 16]> {
        self.store.with_conn(|c| {
            let verifier: Vec<u8> = c
                .query_row(
                    "SELECT verifier FROM observer_grants WHERE token_id=?",
                    params![token_id.as_slice()],
                    |r| r.get(0),
                )
                .optional()
                .context("read grant verifier")?
                .context("no such grant")?;
            if verifier.len() != 32 {
                anyhow::bail!("stored verifier is malformed");
            }
            let mut v = [0u8; 32];
            v.copy_from_slice(&verifier);
            Ok(salt_of(&v))
        })
    }

    pub fn touch_last_seen(&self, observer: &[u8; 32], now: i64) -> Result<()> {
        self.store.with_conn(|c| {
            c.execute(
                "UPDATE observer_grants SET last_seen_at=? \
                 WHERE state='redeemed' AND approved_at IS NOT NULL AND redeemed_by=?",
                params![now, observer.as_slice()],
            )
            .context("touch grant last_seen")?;
            Ok(())
        })
    }
}

fn decode_grant_id(grant_id: &str) -> Result<[u8; 8]> {
    let raw = hex::decode(grant_id).context("grant id is not hex")?;
    if raw.len() != 8 {
        anyhow::bail!("grant id must be 8 bytes");
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw);
    Ok(id)
}

fn to_endpoint_id(raw: &[u8]) -> Option<[u8; 32]> {
    if raw.len() != 32 {
        return None;
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(raw);
    Some(id)
}

fn read_grant(c: &rusqlite::Connection, token_id: &[u8; 8]) -> Result<Option<GrantRecord>> {
    c.query_row(
        "SELECT token_id,friend_label,state,created_at,expires_at,redeemed_by,redeemed_at,last_seen_at,profile,approved_at \
         FROM observer_grants WHERE token_id=?",
        params![token_id.as_slice()],
        row_to_record,
    )
    .optional()
    .context("read observer grant")
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<GrantRecord> {
    let token_id: Vec<u8> = r.get(0)?;
    let redeemed_by: Option<Vec<u8>> = r.get(5)?;
    let profile_raw: String = r.get(8)?;
    Ok(GrantRecord {
        grant_id: hex::encode(&token_id),
        label: r.get(1)?,
        state: GrantState::from_db(&r.get::<_, String>(2)?),
        created_at: r.get(3)?,
        expires_at: r.get(4)?,
        redeemed_by: redeemed_by.as_deref().and_then(to_endpoint_id),
        redeemed_at: r.get(6)?,
        approved_at: r.get(9)?,
        last_seen_at: r.get(7)?,
        // An unparseable profile falls back to the narrowest one. Failing closed here
        // means a corrupted row reveals less, never more.
        profile: serde_json::from_str(&profile_raw).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn store() -> GrantStore {
        GrantStore::new(Store::memory().unwrap())
    }

    const HOST: [u8; 32] = [7u8; 32];
    const OBSERVER: [u8; 32] = [9u8; 32];

    // ---- token codec ----

    #[test]
    fn token_round_trips() {
        let t = ObserverToken::mint(HOST, 1_800_000_000).unwrap();
        let encoded = t.encode();
        assert!(encoded.starts_with(TOKEN_PREFIX));
        // 68 bytes of payload is 109 base32 symbols, plus the 14-character prefix.
        assert_eq!(encoded.len(), TOKEN_PREFIX.len() + 109);
        assert_eq!(encoded, encoded.to_ascii_lowercase(), "token must be lowercase");

        let parsed = ObserverToken::parse(&encoded).unwrap();
        assert_eq!(parsed.endpoint_id, t.endpoint_id);
        assert_eq!(parsed.secret.as_slice(), t.secret.as_slice());
        assert_eq!(parsed.exp_hint, 1_800_000_000);
    }

    #[test]
    fn two_mints_never_share_a_secret() {
        let a = ObserverToken::mint(HOST, 1).unwrap();
        let b = ObserverToken::mint(HOST, 1).unwrap();
        assert_ne!(a.secret.as_slice(), b.secret.as_slice());
    }

    #[test]
    fn rejects_wrong_prefix() {
        let good = ObserverToken::mint(HOST, 1).unwrap().encode();
        let body = good.strip_prefix(TOKEN_PREFIX).unwrap();
        assert_eq!(
            ObserverToken::parse(body).unwrap_err(),
            TokenError::BadPrefix,
            "the bare payload without the version prefix must not parse"
        );
        assert_eq!(
            ObserverToken::parse(&format!("warden-obs-v2.{body}")).unwrap_err(),
            TokenError::BadPrefix
        );
        assert_eq!(
            ObserverToken::parse("").unwrap_err(),
            TokenError::BadPrefix
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let good = ObserverToken::mint(HOST, 1).unwrap().encode();
        // Dropping 5 symbols leaves 104, a whole number of base32 blocks, so it decodes
        // cleanly to 65 bytes and is caught by the length check.
        assert_eq!(
            ObserverToken::parse(&good[..good.len() - 5]).unwrap_err(),
            TokenError::BadLength
        );
        // Which of the two rejections fires for a given cut depends on the secret's bits
        // (a short prefix may or may not leave non-zero trailing bits), so the invariant
        // worth asserting is that NO prefix parses, not which error it produces.
        for cut in 1..40 {
            assert!(
                ObserverToken::parse(&good[..good.len() - cut]).is_err(),
                "a token truncated by {cut} symbols must not parse"
            );
        }
    }

    #[test]
    fn rejects_corrupted_base32() {
        let good = ObserverToken::mint(HOST, 1).unwrap().encode();
        // '0', '1' and '8' are outside the base32 alphabet.
        let corrupt = format!("{}0", &good[..good.len() - 1]);
        assert_eq!(
            ObserverToken::parse(&corrupt).unwrap_err(),
            TokenError::BadBase32
        );
    }

    #[test]
    fn rejects_trailing_garbage() {
        let good = ObserverToken::mint(HOST, 1).unwrap().encode();
        assert_eq!(
            ObserverToken::parse(&format!("{good}aaaaaaaa")).unwrap_err(),
            TokenError::BadLength,
            "extra symbols must not be ignored"
        );
        assert_eq!(
            ObserverToken::parse(&format!("{good}!")).unwrap_err(),
            TokenError::BadBase32
        );
    }

    #[test]
    fn rejects_all_zero_secret() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&HOST);
        payload.extend_from_slice(&[0u8; 32]);
        payload.extend_from_slice(&7u32.to_be_bytes());
        let forged = format!("{TOKEN_PREFIX}{}", BASE32_LOWER.encode(&payload));
        assert_eq!(
            ObserverToken::parse(&forged).unwrap_err(),
            TokenError::ZeroSecret
        );
    }

    #[test]
    fn rejects_all_zero_endpoint_id() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0u8; 32]);
        payload.extend_from_slice(&[3u8; 32]);
        payload.extend_from_slice(&7u32.to_be_bytes());
        let forged = format!("{TOKEN_PREFIX}{}", BASE32_LOWER.encode(&payload));
        assert_eq!(
            ObserverToken::parse(&forged).unwrap_err(),
            TokenError::ZeroEndpointId
        );
    }

    #[test]
    fn rejects_non_canonical_trailing_bits() {
        // 68 bytes is 544 bits carried in 109 symbols, which hold 545, so the final
        // symbol's lowest bit is spare and must be zero. Setting it yields a DIFFERENT
        // string that decodes to the SAME 68 bytes, which would make one grant have two
        // valid spellings. The encoding must refuse it.
        const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz234567";
        let good = ObserverToken::mint(HOST, 1).unwrap().encode();
        let last = good.chars().last().expect("token is non-empty");
        let idx = ALPHABET.find(last).expect("token uses the base32 alphabet");
        assert_eq!(idx % 2, 0, "encode() must leave the spare bit clear");
        // The sibling symbol differs only in that spare bit.
        let sibling = ALPHABET.as_bytes()[idx ^ 1] as char;
        let mutated = format!("{}{sibling}", &good[..good.len() - 1]);
        assert_ne!(mutated, good);
        assert_eq!(
            ObserverToken::parse(&mutated).unwrap_err(),
            TokenError::BadBase32,
            "a token must have exactly one valid spelling"
        );
    }

    // ---- verifier ----

    #[test]
    fn verifier_is_deterministic_and_secret_dependent() {
        let a = verifier_of(&[1u8; 32]);
        let b = verifier_of(&[1u8; 32]);
        let c = verifier_of(&[2u8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(token_id_of(&a), token_id_of(&b));
        assert_ne!(token_id_of(&a), token_id_of(&c));
    }

    #[test]
    fn salt_differs_per_grant() {
        assert_ne!(salt_of(&verifier_of(&[1u8; 32])), salt_of(&verifier_of(&[2u8; 32])));
    }

    // ---- grant store ----

    #[test]
    fn create_stores_verifier_never_secret() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        assert_eq!(rec.state, GrantState::Pending);
        assert_eq!(rec.expires_at, 1900);

        let parsed = ObserverToken::parse(&token).unwrap();
        let secret_hex = hex::encode(parsed.secret.as_slice());
        let dump: String = gs
            .store
            .with_conn(|c| {
                let mut st = c.prepare("SELECT token_id,verifier,friend_label,profile FROM observer_grants")?;
                let rows = st.query_map([], |r| {
                    Ok(format!(
                        "{} {} {} {}",
                        hex::encode(r.get::<_, Vec<u8>>(0)?),
                        hex::encode(r.get::<_, Vec<u8>>(1)?),
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?
                    ))
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.join("\n"))
            })
            .unwrap();
        assert!(
            !dump.contains(&secret_hex),
            "the secret must never reach the database"
        );
        assert!(dump.contains(&hex::encode(verifier_of(&parsed.secret))));
    }

    #[test]
    fn ttl_is_clamped() {
        let gs = store();
        let (short, _) = gs.create(HOST, "a", 1, Profile::Shapes, 0).unwrap();
        assert_eq!(short.expires_at, MIN_TTL_SECS as i64);
        let (long, _) = gs.create(HOST, "b", 10_000_000, Profile::Shapes, 0).unwrap();
        assert_eq!(long.expires_at, MAX_TTL_SECS as i64);
    }

    #[test]
    fn redeem_happy_path_then_second_attempt_fails() {
        let gs = store();
        let (_, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;

        match gs.redeem(&secret, OBSERVER, 1001).unwrap() {
            RedeemOutcome::Claimed(g) => {
                assert_eq!(g.state, GrantState::Redeemed);
                assert_eq!(g.redeemed_by, Some(OBSERVER));
                assert_eq!(g.redeemed_at, Some(1001));
            }
            other => panic!("expected a claim, got {other:?}"),
        }
        assert_eq!(
            gs.redeem(&secret, [1u8; 32], 1002).unwrap(),
            RedeemOutcome::NotPending,
            "a token is single use"
        );
    }

    #[test]
    fn redeem_rejects_unknown_and_wrong_secret() {
        let gs = store();
        let (_, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        assert_eq!(
            gs.redeem(&[42u8; 32], OBSERVER, 1001).unwrap(),
            RedeemOutcome::Unknown
        );

        // Same token_id, different secret: forge a row whose token_id matches a real
        // grant's but whose verifier does not. The constant-time compare must refuse.
        let real = *ObserverToken::parse(&token).unwrap().secret;
        let real_verifier = verifier_of(&real);
        let mut forged_verifier = real_verifier;
        forged_verifier[31] ^= 0xff;
        gs.store
            .with_conn(|c| {
                c.execute(
                    "UPDATE observer_grants SET verifier=? WHERE token_id=?",
                    params![
                        forged_verifier.as_slice(),
                        token_id_of(&real_verifier).as_slice()
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            gs.redeem(&real, OBSERVER, 1001).unwrap(),
            RedeemOutcome::Unknown,
            "a matching token_id with a mismatched verifier must not redeem"
        );
    }

    #[test]
    fn redeem_refuses_expired_by_host_clock() {
        let gs = store();
        let (_, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        assert_eq!(
            gs.redeem(&secret, OBSERVER, 9_999_999).unwrap(),
            RedeemOutcome::Expired
        );
    }

    #[test]
    fn redeem_refuses_revoked() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.revoke(&rec.grant_id).unwrap();
        assert_eq!(
            gs.redeem(&secret, OBSERVER, 1001).unwrap(),
            RedeemOutcome::NotPending
        );
    }

    #[test]
    fn revoke_returns_bound_endpoint_and_drops_it_from_the_allow_set() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.redeem(&secret, OBSERVER, 1001).unwrap();
        assert!(gs.approve(&rec.grant_id, 1002).unwrap());
        assert_eq!(gs.active_endpoints().unwrap(), vec![OBSERVER]);

        assert_eq!(gs.revoke(&rec.grant_id).unwrap(), Some(OBSERVER));
        assert!(gs.active_endpoints().unwrap().is_empty());
        assert!(gs.grant_for_endpoint(&OBSERVER).unwrap().is_none());
    }

    #[test]
    fn sweep_flips_stale_pending_only() {
        let gs = store();
        let (stale, _) = gs.create(HOST, "old", 60, Profile::Shapes, 0).unwrap();
        let (fresh, _) = gs.create(HOST, "new", 3600, Profile::Shapes, 0).unwrap();
        assert_eq!(gs.sweep_expired(1000).unwrap(), 1);
        let by_id = |id: &str| {
            gs.list()
                .unwrap()
                .into_iter()
                .find(|g| g.grant_id == id)
                .unwrap()
                .state
        };
        assert_eq!(by_id(&stale.grant_id), GrantState::Expired);
        assert_eq!(by_id(&fresh.grant_id), GrantState::Pending);
    }

    #[test]
    fn last_seen_tracks_the_bound_observer() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.redeem(&secret, OBSERVER, 1001).unwrap();
        gs.approve(&rec.grant_id, 1002).unwrap();
        gs.touch_last_seen(&OBSERVER, 1234).unwrap();
        assert_eq!(
            gs.grant_for_endpoint(&OBSERVER).unwrap().unwrap().last_seen_at,
            Some(1234)
        );
    }

    #[test]
    fn a_redeemed_grant_outlives_its_token_expiry() {
        // The token expires; the grant does not. This is what lets a friend keep watching
        // for an hour after a 15-minute token, and what lets a reconnect after a wifi drop
        // re-authenticate by public key with no token involved.
        //
        // Approval is part of the setup, not the thing under test: this generosity is
        // deliberate for a grant the host said yes to, and `redeem_alone_never_authorises`
        // below is what pins that it does NOT extend to one the host never answered for.
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 60, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.redeem(&secret, OBSERVER, 1001).unwrap();
        gs.approve(&rec.grant_id, 1002).unwrap();
        gs.sweep_expired(9_999_999).unwrap();
        assert_eq!(gs.active_endpoints().unwrap(), vec![OBSERVER]);
        // And a restart does not undo it: the startup sweep only kills unapproved grants.
        assert_eq!(gs.revoke_unapproved().unwrap(), 0);
        assert_eq!(gs.active_endpoints().unwrap(), vec![OBSERVER]);
    }

    /// The invariant the whole `approved_at` column exists for: redemption is something
    /// the PEER drives, so on its own it must authorise nothing at all.
    #[test]
    fn redeem_alone_never_authorises() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        assert!(matches!(
            gs.redeem(&secret, OBSERVER, 1001).unwrap(),
            RedeemOutcome::Claimed(_)
        ));

        // Claimed, and still not allowed anywhere.
        assert!(gs.active_endpoints().unwrap().is_empty());
        assert!(gs.grant_for_endpoint(&OBSERVER).unwrap().is_none());
        let listed = gs.list().unwrap();
        assert_eq!(listed[0].state, GrantState::Redeemed);
        assert_eq!(listed[0].approved_at, None);

        // The host says yes, and only now does anything open.
        assert!(gs.approve(&rec.grant_id, 1002).unwrap());
        assert_eq!(gs.active_endpoints().unwrap(), vec![OBSERVER]);
        assert_eq!(
            gs.grant_for_endpoint(&OBSERVER).unwrap().unwrap().approved_at,
            Some(1002)
        );
    }

    #[test]
    fn approve_refuses_a_grant_revoked_while_the_prompt_was_up() {
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.redeem(&secret, OBSERVER, 1001).unwrap();
        gs.revoke(&rec.grant_id).unwrap();

        assert!(
            !gs.approve(&rec.grant_id, 1002).unwrap(),
            "a revoked grant must not become approved, whatever the prompt returns"
        );
        assert!(gs.active_endpoints().unwrap().is_empty());
        // An unknown grant id is likewise a no, not an error the caller might swallow.
        assert!(!gs.approve(&hex::encode([0u8; 8]), 1002).unwrap());
    }

    #[test]
    fn approving_twice_is_idempotent_and_keeps_the_first_timestamp() {
        // Two connections from the same peer can race the prompt. The second must get a
        // yes rather than be refused for having nothing to write.
        let gs = store();
        let (rec, token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        gs.redeem(&secret, OBSERVER, 1001).unwrap();
        assert!(gs.approve(&rec.grant_id, 1002).unwrap());
        assert!(gs.approve(&rec.grant_id, 5555).unwrap());
        assert_eq!(
            gs.grant_for_endpoint(&OBSERVER).unwrap().unwrap().approved_at,
            Some(1002)
        );
    }

    #[test]
    fn revoke_unapproved_kills_orphans_and_spares_approved_grants() {
        let gs = store();
        let (orphan, orphan_token) = gs.create(HOST, "crashed", 900, Profile::Shapes, 1000).unwrap();
        let (good, good_token) = gs.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let (pending, _) = gs.create(HOST, "unused", 900, Profile::Shapes, 1000).unwrap();
        let orphan_secret = *ObserverToken::parse(&orphan_token).unwrap().secret;
        let good_secret = *ObserverToken::parse(&good_token).unwrap().secret;

        gs.redeem(&orphan_secret, [1u8; 32], 1001).unwrap();
        gs.redeem(&good_secret, OBSERVER, 1001).unwrap();
        gs.approve(&good.grant_id, 1002).unwrap();

        assert_eq!(gs.revoke_unapproved().unwrap(), 1);
        let by_id = |id: &str| {
            gs.list()
                .unwrap()
                .into_iter()
                .find(|g| g.grant_id == id)
                .unwrap()
        };
        assert_eq!(by_id(&orphan.grant_id).state, GrantState::Revoked);
        assert_eq!(by_id(&good.grant_id).state, GrantState::Redeemed);
        // An untouched pending grant is not collateral: it can still be redeemed.
        assert_eq!(by_id(&pending.grant_id).state, GrantState::Pending);
        assert_eq!(gs.active_endpoints().unwrap(), vec![OBSERVER]);

        // And the orphan's token is spent, so the peer cannot simply redeem it again.
        assert_eq!(
            gs.redeem(&orphan_secret, [1u8; 32], 1003).unwrap(),
            RedeemOutcome::NotPending
        );
    }

    /// A real `~/.warden/warden.db` written before `approved_at` existed. Its `redeemed`
    /// rows carry no record of whether the host ever approved them, so the migration must
    /// read them as unapproved rather than assume the friendly answer.
    #[test]
    fn an_old_schema_redeemed_row_migrates_to_unapproved() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("warden.db");

        // The pre-`approved_at` DDL, verbatim, with one redeemed row bound to an observer.
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE observer_grants(
                    token_id BLOB PRIMARY KEY,
                    verifier BLOB NOT NULL,
                    friend_label TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    redeemed_by BLOB,
                    redeemed_at INTEGER,
                    profile TEXT NOT NULL,
                    last_seen_at INTEGER
                );",
            )
            .unwrap();
            c.execute(
                "INSERT INTO observer_grants(token_id,verifier,friend_label,created_at,expires_at,state,redeemed_by,redeemed_at,profile,last_seen_at) \
                 VALUES(?,?,'legacy',1000,1060,'redeemed',?,1001,'\"shapes\"',NULL)",
                params![[3u8; 8].as_slice(), [4u8; 32].as_slice(), OBSERVER.as_slice()],
            )
            .unwrap();
        }

        // Opening the store runs the migration.
        let gs = GrantStore::new(Store::open(&db).unwrap());
        let listed = gs.list().unwrap();
        assert_eq!(listed.len(), 1, "the row must survive the migration");
        assert_eq!(listed[0].approved_at, None, "unknown approval reads as none");
        assert!(
            gs.active_endpoints().unwrap().is_empty(),
            "a legacy redeemed row must not authorise anyone on the strength of its state alone"
        );
        assert!(gs.grant_for_endpoint(&OBSERVER).unwrap().is_none());

        // And migrating twice is a no-op, not an error.
        let gs = GrantStore::new(Store::open(&db).unwrap());
        assert_eq!(gs.list().unwrap().len(), 1);
    }

    /// The test the whole atomic-claim design exists for: two threads, two SEPARATE
    /// SQLite connections onto the same file, redeeming the same token at the same
    /// instant. Exactly one must win.
    ///
    /// Two connections, not one `Store` clone, is the point: a single shared connection is
    /// serialized by the Rust mutex, which would make this pass without proving anything.
    /// Here SQLite's own write lock is the only arbiter, which is the real condition.
    #[test]
    fn two_concurrent_redemptions_exactly_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("grants.db");

        let seed = GrantStore::new(Store::open(&db).unwrap());
        let (_, token) = seed.create(HOST, "Mark", 900, Profile::Shapes, 1000).unwrap();
        let secret = *ObserverToken::parse(&token).unwrap().secret;
        drop(seed);

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for (i, observer) in [[11u8; 32], [22u8; 32]].into_iter().enumerate() {
            let db = db.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let store = Store::open(&db).unwrap();
                // Without a busy timeout the loser of the write lock gets SQLITE_BUSY
                // instead of a clean "someone else claimed it" answer.
                store
                    .with_conn(|c| {
                        c.busy_timeout(std::time::Duration::from_secs(5))?;
                        Ok(())
                    })
                    .unwrap();
                let gs = GrantStore::new(store);
                barrier.wait();
                (i, gs.redeem(&secret, observer, 1001).unwrap())
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners: Vec<_> = results
            .iter()
            .filter(|(_, r)| matches!(r, RedeemOutcome::Claimed(_)))
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "exactly one redemption must win, got {results:?}"
        );
        assert!(
            results
                .iter()
                .any(|(_, r)| matches!(r, RedeemOutcome::NotPending)),
            "the loser must be told the grant is no longer pending, got {results:?}"
        );

        // And the persisted row agrees with whoever won.
        let gs = GrantStore::new(Store::open(&db).unwrap());
        let all = gs.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, GrantState::Redeemed);
        gs.approve(&all[0].grant_id, 1002).unwrap();
        assert_eq!(gs.active_endpoints().unwrap().len(), 1);
    }
}
