//! The iroh transport for remote read-only observation.
//!
//! This module deliberately cannot name the radar. It is handed a [`FrameSource`], a
//! closure that turns a per-grant salt into an already-projected [`ObservedState`], so the
//! only type it can serialize outward is the redacted one. The un-redacted radar types are
//! not imported here and there is a test asserting this file never mentions them; see
//! `transport_cannot_name_the_radar`. That is the difference between "we remembered to
//! call `project()`" and "there is nothing else to send".
//!
//! Read-only is enforced by protocol shape rather than by a check:
//!
//! * [`ObserverMsg`] has exactly one variant. There is no request, command, subscribe or
//!   ping, so an attacker cannot send a message the host has a type to parse.
//! * After the Hello the host never calls `accept_bi`, `accept_uni` or `read_datagram` on
//!   an observer connection. `accept_bi` is called at most once per connection, in
//!   [`read_hello`], and the [`RecvStream`] is dropped the moment it returns.
//! * Liveness comes from QUIC's own ACKs via `Connection::closed()`, so there is no
//!   heartbeat that would need reading observer bytes.
//!
//! The endpoint is bound LAZILY, on the first [`HostShare::start`]. Warden at rest binds
//! no socket and contacts no relay, which is what keeps "fully local by default" literally
//! true rather than rhetorically true.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, VarInt};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::observe::grants::{GrantStore, ObserverToken, RedeemOutcome, SECRET_LEN};
use crate::observe::projection::{ObservedState, Profile};

/// The one application protocol this endpoint speaks. A connection offering anything else
/// is refused before a single byte of ours is written.
pub const ALPN: &[u8] = b"warden/observe/1";

/// The host is a laptop whose primary job is the local radar. These are the caps that keep
/// a remote observer from ever being the reason it stutters.
const MAX_OBSERVERS: usize = 5;
const HELLO_DEADLINE: Duration = Duration::from_secs(5);
const MAX_HELLO_BYTES: usize = 256;
/// Depth 4, written with `try_send`. A slow observer drops frames rather than
/// back-pressuring the broadcast loop, because the radar must never wait on a network.
const FRAME_CHANNEL_DEPTH: usize = 4;
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
/// How long a connection waits on the host's approve/deny before giving up.
const APPROVAL_DEADLINE: Duration = Duration::from_secs(120);

/// Flood guard on failed redemptions: see [`FailureWindow`].
const MAX_FAILURES: usize = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

// QUIC application close codes, so an observer can render a real reason.
const CODE_REVOKED: u32 = 1;
const CODE_DENIED: u32 = 2;
const CODE_BAD_TOKEN: u32 = 3;
const CODE_BUSY: u32 = 4;
const CODE_TIMEOUT: u32 = 5;
const CODE_SHUTDOWN: u32 = 6;

/// Produces the frame for one grant, already projected through the redaction boundary.
///
/// A closure rather than a handle to the radar cache: the caller owns the un-redacted
/// state and hands this module only the ability to ask for a redacted view.
pub type FrameSource = Arc<dyn Fn(&[u8; 16]) -> Option<ObservedState> + Send + Sync>;

/// Rust to web notifications. A closure for the same reason as [`FrameSource`]: this
/// module never holds a `tauri::AppHandle`, so it cannot invoke a command even if a bug
/// tried to.
pub type EventSink = Arc<dyn Fn(ObserveEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveEvent {
    Grants,
    Peers,
    /// A new frame landed from a host we are watching. Carries only the peer id: the UI
    /// re-fetches, so a frame never rides on the event bus.
    Frame { peer_id: String },
    Approval(PendingApproval),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub conn_id: String,
    pub fingerprint: String,
    pub grant_label: String,
}

/// The ONLY message an observer can send.
///
/// One variant is the point. Adding a mutation later means adding a variant here, which is
/// a visible, reviewable diff rather than a config flag or a forgotten check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum ObserverMsg {
    /// Carries the token secret, hex encoded so the whole message stays near 110 bytes and
    /// comfortably inside [`MAX_HELLO_BYTES`].
    Hello { secret: String, client_version: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloAck {
    pub host_label: String,
    pub profile: Profile,
    pub fingerprint: String,
}

/// Display-safe rendering of a public key: 8 groups of 4 hex characters.
///
/// Half the key, which is ample to compare over a phone call and short enough that a
/// person actually will. The full id is not a secret; this is about readability.
pub fn fingerprint(id: &[u8; 32]) -> String {
    let hex = hex::encode(&id[..16]);
    hex.as_bytes()
        .chunks(4)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .collect::<Vec<_>>()
        .join("-")
}

/// How the endpoint reaches the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindMode {
    /// Number 0's relays plus DNS address lookup: the shipping configuration, and the only
    /// one that crosses arbitrary NATs.
    Public,
    /// No relays, no address lookup, no packet that leaves the machine. Used by the
    /// loopback tests so they exercise the real QUIC handshake without depending on (or
    /// touching) third-party infrastructure.
    LocalOnly,
}

/// Load the host's persistent identity, generating it on first use.
///
/// Persistence is not a convenience: every grant binds to this key, so regenerating it
/// would silently invalidate every grant the user has handed out.
pub fn load_or_create_secret_key(path: &std::path::Path) -> Result<SecretKey> {
    if let Ok(raw) = std::fs::read(path) {
        if raw.len() == 32 {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&raw);
            return Ok(SecretKey::from_bytes(&bytes));
        }
        // A wrong-sized file is corruption, not a format we should guess at. Failing here
        // rather than regenerating means the user is told, instead of quietly losing every
        // grant bound to the old key.
        anyhow::bail!(
            "observer key at {} is {} bytes, expected 32",
            path.display(),
            raw.len()
        );
    }
    let key = SecretKey::generate();
    crate::util::ensure_parent(path)?;
    crate::platform::write_private_file(path, &key.to_bytes())
        .with_context(|| format!("write observer key {}", path.display()))?;
    Ok(key)
}

async fn bind_endpoint(secret: SecretKey, mode: BindMode) -> Result<Endpoint> {
    let ep = match mode {
        BindMode::Public => {
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(secret)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await?
        }
        BindMode::LocalOnly => {
            Endpoint::builder(iroh::endpoint::presets::Minimal)
                .secret_key(secret)
                .alpns(vec![ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .clear_address_lookup()
                .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
                .bind()
                .await?
        }
    };
    Ok(ep)
}

/// One observer's slice of a broadcast tick: who they are, their per-grant salt, the hash
/// of what they last received, and where to hand the next frame.
type BroadcastTarget = ([u8; 32], [u8; 16], u64, mpsc::Sender<String>);

struct LiveObserver {
    conn: Connection,
    tx: mpsc::Sender<String>,
    salt: [u8; 16],
    /// Hash of the last frame handed to this observer, so an unchanged state produces no
    /// traffic at all.
    last_frame: u64,
}

struct HostInner {
    grants: GrantStore,
    frames: FrameSource,
    events: EventSink,
    host_label: String,
    key_path: std::path::PathBuf,
    mode: BindMode,
    endpoint: tokio::sync::Mutex<Option<Endpoint>>,
    /// The in-memory allow set. Revocation removes an id here, and both the broadcast loop
    /// and each writer re-check it, so a revoke that races an in-flight frame still stops
    /// the next one.
    allow: RwLock<HashSet<[u8; 32]>>,
    live: Mutex<HashMap<[u8; 32], LiveObserver>>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    pending_meta: Mutex<HashMap<String, PendingApproval>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Failed token redemptions, for the flood guard below.
    redeem_failures: Mutex<FailureWindow>,
}

/// Sliding-window guard on FAILED token redemptions.
///
/// Not a brute-force defence: the secret is 256 bits, so guessing it is not a threat model
/// anyone needs a counter for. This exists because a failed redemption never becomes an
/// observer, so `MAX_OBSERVERS` does not bound it: someone who learns the endpoint id could
/// otherwise open connections forever, each costing a QUIC handshake and a task held for up
/// to `HELLO_DEADLINE`. After `MAX_FAILURES` inside `FAILURE_WINDOW`, new inbound
/// connections are refused for `FAILURE_COOLDOWN`.
///
/// Deliberately global rather than per-endpoint: an attacker picks a fresh keypair per
/// attempt for free, so a per-endpoint counter would never trip. The cost of the global
/// choice is that a flood also locks out a legitimate friend, which is the right trade when
/// the alternative is the host's radar stalling, and it never affects an ALREADY authorised
/// observer: reconnects skip this path entirely.
#[derive(Debug, Default)]
struct FailureWindow {
    failures: std::collections::VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

impl FailureWindow {
    /// True when new inbound connections should be refused right now.
    fn blocked(&mut self, now: Instant) -> bool {
        match self.blocked_until {
            Some(t) if now < t => true,
            Some(_) => {
                self.blocked_until = None;
                self.failures.clear();
                false
            }
            None => false,
        }
    }

    /// Record one failed redemption, opening a cooldown once the window fills.
    fn record(&mut self, now: Instant) {
        while let Some(front) = self.failures.front() {
            if now.duration_since(*front) > FAILURE_WINDOW {
                self.failures.pop_front();
            } else {
                break;
            }
        }
        self.failures.push_back(now);
        if self.failures.len() >= MAX_FAILURES {
            self.blocked_until = Some(now + FAILURE_COOLDOWN);
            self.failures.clear();
        }
    }
}

#[derive(Clone)]
pub struct HostShare {
    inner: Arc<HostInner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    pub endpoint_id: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharingStatus {
    pub sharing: bool,
    pub endpoint_id: Option<String>,
    pub fingerprint: Option<String>,
    pub live_observers: u32,
}

impl HostShare {
    pub fn new(
        grants: GrantStore,
        frames: FrameSource,
        events: EventSink,
        host_label: String,
        key_path: std::path::PathBuf,
        mode: BindMode,
    ) -> Self {
        Self {
            inner: Arc::new(HostInner {
                grants,
                frames,
                events,
                host_label,
                key_path,
                mode,
                endpoint: tokio::sync::Mutex::new(None),
                allow: RwLock::new(HashSet::new()),
                live: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                pending_meta: Mutex::new(HashMap::new()),
                tasks: Mutex::new(Vec::new()),
                redeem_failures: Mutex::new(FailureWindow::default()),
            }),
        }
    }

    /// Bind the endpoint and start accepting observers. Idempotent.
    ///
    /// This is the ONLY call site of `Endpoint::bind` in the crate, which is what makes
    /// "no socket until the user clicks Share" checkable rather than aspirational.
    pub async fn start(&self) -> Result<HostIdentity> {
        let mut slot = self.inner.endpoint.lock().await;
        if let Some(ep) = slot.as_ref() {
            return Ok(identity_of(ep.id()));
        }

        let key = load_or_create_secret_key(&self.inner.key_path)?;
        let ep = bind_endpoint(key, self.inner.mode)
            .await
            .context("bind observation endpoint")?;
        let identity = identity_of(ep.id());

        // Kill anything the previous run claimed but never approved. A redeemed grant with
        // no `approved_at` means the app died between the peer's claim and the host's
        // answer, so nobody ever allowed that peer in. Sweeping here, before the allow set
        // is seeded, is what stops a grant orphaned by a crash from being resurrected.
        let swept = self.inner.grants.revoke_unapproved()?;
        if swept > 0 {
            tracing::info!(count = swept, "revoked grants claimed but never approved");
            (self.inner.events)(ObserveEvent::Grants);
        }

        // Seed the allow set from grants that were already approved in a previous run, so
        // a friend reconnects after a restart without needing a new token.
        {
            let active = self.inner.grants.active_endpoints()?;
            let mut allow = self.inner.allow.write().expect("allow set poisoned");
            allow.clear();
            allow.extend(active);
        }

        let accept = tokio::spawn(accept_loop(self.inner.clone(), ep.clone()));
        let broadcast = tokio::spawn(broadcast_loop(self.inner.clone()));
        {
            let mut tasks = self.inner.tasks.lock().expect("task list poisoned");
            tasks.push(accept);
            tasks.push(broadcast);
        }

        *slot = Some(ep);
        Ok(identity)
    }

    /// Unbind and close every live observer. Idempotent.
    pub async fn stop(&self) -> Result<()> {
        let ep = self.inner.endpoint.lock().await.take();
        {
            let mut tasks = self.inner.tasks.lock().expect("task list poisoned");
            for t in tasks.drain(..) {
                t.abort();
            }
        }
        {
            let mut live = self.inner.live.lock().expect("live map poisoned");
            for (_, obs) in live.drain() {
                obs.conn.close(VarInt::from_u32(CODE_SHUTDOWN), b"sharing stopped");
            }
        }
        self.inner.allow.write().expect("allow set poisoned").clear();
        self.inner.pending.lock().expect("pending poisoned").clear();
        self.inner
            .pending_meta
            .lock()
            .expect("pending meta poisoned")
            .clear();
        if let Some(ep) = ep {
            ep.close().await;
        }
        Ok(())
    }

    pub async fn status(&self) -> SharingStatus {
        let slot = self.inner.endpoint.lock().await;
        let live = self.inner.live.lock().expect("live map poisoned").len() as u32;
        match slot.as_ref() {
            Some(ep) => {
                let id = identity_of(ep.id());
                SharingStatus {
                    sharing: true,
                    endpoint_id: Some(id.endpoint_id),
                    fingerprint: Some(id.fingerprint),
                    live_observers: live,
                }
            }
            None => SharingStatus {
                sharing: false,
                endpoint_id: None,
                fingerprint: None,
                live_observers: 0,
            },
        }
    }

    /// The host's endpoint id, or `None` when nothing is bound. Minting a token needs it,
    /// which is why creating a grant requires sharing to be on.
    pub async fn endpoint_id(&self) -> Option<[u8; 32]> {
        self.inner
            .endpoint
            .lock()
            .await
            .as_ref()
            .map(|ep| *ep.id().as_bytes())
    }

    /// The addresses this endpoint is reachable on. `None` before [`Self::start`], which
    /// is exactly the "nothing is bound at rest" assertion the tests make.
    pub async fn bound_addr(&self) -> Option<EndpointAddr> {
        let slot = self.inner.endpoint.lock().await;
        let ep = slot.as_ref()?;
        let mut addr = EndpointAddr::new(ep.id());
        for sock in ep.bound_sockets() {
            let ip = if sock.ip().is_unspecified() {
                std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), sock.port())
            } else {
                sock
            };
            addr = addr.with_ip_addr(ip);
        }
        Some(addr)
    }

    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        self.inner
            .pending_meta
            .lock()
            .expect("pending meta poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Resolve an approve-on-first-connect prompt. Unknown ids are a no-op: the connection
    /// may have already timed out.
    pub fn resolve_approval(&self, conn_id: &str, approve: bool) {
        let tx = self
            .inner
            .pending
            .lock()
            .expect("pending poisoned")
            .remove(conn_id);
        self.inner
            .pending_meta
            .lock()
            .expect("pending meta poisoned")
            .remove(conn_id);
        if let Some(tx) = tx {
            let _ = tx.send(approve);
        }
    }

    /// Revoke a grant: flip the row, drop the endpoint from the allow set, and close the
    /// live connection. All three, because the first alone races an in-flight frame.
    pub fn revoke(&self, grant_id: &str) -> Result<()> {
        let endpoint = self.inner.grants.revoke(grant_id)?;
        if let Some(id) = endpoint {
            self.inner
                .allow
                .write()
                .expect("allow set poisoned")
                .remove(&id);
            if let Some(obs) = self.inner.live.lock().expect("live map poisoned").remove(&id) {
                obs.conn.close(VarInt::from_u32(CODE_REVOKED), b"revoked");
            }
        }
        (self.inner.events)(ObserveEvent::Grants);
        Ok(())
    }

    pub fn grants(&self) -> &GrantStore {
        &self.inner.grants
    }

    pub fn is_connected(&self, endpoint: &[u8; 32]) -> bool {
        self.inner
            .live
            .lock()
            .expect("live map poisoned")
            .contains_key(endpoint)
    }
}

fn identity_of(id: EndpointId) -> HostIdentity {
    HostIdentity {
        endpoint_id: hex::encode(id.as_bytes()),
        fingerprint: fingerprint(id.as_bytes()),
    }
}

async fn accept_loop(inner: Arc<HostInner>, ep: Endpoint) {
    while let Some(incoming) = ep.accept().await {
        let inner = inner.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_incoming(inner, incoming).await {
                tracing::debug!(error = %format!("{e:#}"), "observer connection ended");
            }
        });
    }
}

async fn handle_incoming(inner: Arc<HostInner>, incoming: iroh::endpoint::Incoming) -> Result<()> {
    let conn = incoming.await?;
    let remote = *conn.remote_id().as_bytes();

    if inner.live.lock().expect("live map poisoned").len() >= MAX_OBSERVERS {
        conn.close(VarInt::from_u32(CODE_BUSY), b"too many observers");
        return Ok(());
    }

    // Flood guard. Checked BEFORE the allow-set lookup would matter, but it only refuses
    // connections that would have to redeem a token: an already-authorised observer
    // reconnecting is let through below without consuming this budget.
    let already_allowed = inner
        .allow
        .read()
        .expect("allow set poisoned")
        .contains(&remote);
    if !already_allowed
        && inner
            .redeem_failures
            .lock()
            .expect("failure window poisoned")
            .blocked(Instant::now())
    {
        conn.close(VarInt::from_u32(CODE_BUSY), b"too many failed attempts");
        return Ok(());
    }

    // A reconnect from an already-authorised endpoint needs no token: the QUIC handshake
    // already proved possession of that private key, which is the durable credential. This
    // is what survives a laptop sleep or a wifi flap, and it is why no Hello is read here.
    let known = inner
        .allow
        .read()
        .expect("allow set poisoned")
        .contains(&remote);

    let (salt, profile) = if known {
        let grant = inner
            .grants
            .grant_for_endpoint(&remote)?
            .context("allowed endpoint has no grant")?;
        (grant_salt(&inner, &grant.grant_id)?, grant.profile)
    } else {
        let secret = read_hello(&conn).await?;
        let outcome = inner
            .grants
            .redeem(&secret, remote, chrono::Utc::now().timestamp())?;
        let grant = match outcome {
            RedeemOutcome::Claimed(g) => *g,
            RedeemOutcome::Unknown | RedeemOutcome::NotPending => {
                inner
                    .redeem_failures
                    .lock()
                    .expect("failure window poisoned")
                    .record(Instant::now());
                conn.close(VarInt::from_u32(CODE_BAD_TOKEN), b"token not valid");
                return Ok(());
            }
            RedeemOutcome::Expired => {
                inner
                    .redeem_failures
                    .lock()
                    .expect("failure window poisoned")
                    .record(Instant::now());
                conn.close(VarInt::from_u32(CODE_BAD_TOKEN), b"token expired");
                return Ok(());
            }
        };
        (inner.events)(ObserveEvent::Grants);

        // Approve on first connect. Nothing has been sent yet and nothing will be until
        // the host resolves this, which is what defeats an interceptor who redeemed a
        // token they were never meant to have.
        if !await_approval(&inner, &remote, &grant.label).await {
            conn.close(VarInt::from_u32(CODE_DENIED), b"not approved");
            // A denied observer must not be able to reconnect on the strength of the
            // grant it just claimed, so the grant dies with the refusal.
            let _ = inner.grants.revoke(&grant.grant_id);
            (inner.events)(ObserveEvent::Grants);
            return Ok(());
        }

        // Commit the approval to disk BEFORE anyone is let in. The `await_approval` above
        // is memory only (a oneshot and the in-memory `allow` set), so without this write
        // the only durable trace of this whole exchange would be the redemption, which the
        // peer drove. That is the wrong default: a host that quits or crashes inside the
        // approval deadline would leave a claimed grant behind, and the next `start()`
        // would seed it into the allow set as a permanently authorised observer.
        //
        // A false answer means the grant stopped being approvable while the prompt was up
        // (revoked from the Access tab, most likely), so the connection is refused.
        if !inner
            .grants
            .approve(&grant.grant_id, chrono::Utc::now().timestamp())?
        {
            conn.close(VarInt::from_u32(CODE_REVOKED), b"grant is no longer valid");
            (inner.events)(ObserveEvent::Grants);
            return Ok(());
        }
        (inner.events)(ObserveEvent::Grants);

        inner
            .allow
            .write()
            .expect("allow set poisoned")
            .insert(remote);
        (grant_salt(&inner, &grant.grant_id)?, grant.profile)
    };

    let ack = HelloAck {
        host_label: inner.host_label.clone(),
        profile,
        fingerprint: fingerprint(&remote),
    };
    let mut ack_stream = conn.open_uni().await?;
    ack_stream
        .write_all(serde_json::to_string(&ack)?.as_bytes())
        .await?;
    ack_stream.finish()?;

    let (tx, rx) = mpsc::channel::<String>(FRAME_CHANNEL_DEPTH);
    inner.live.lock().expect("live map poisoned").insert(
        remote,
        LiveObserver {
            conn: conn.clone(),
            tx,
            salt,
            last_frame: 0,
        },
    );
    (inner.events)(ObserveEvent::Peers);

    let writer_inner = inner.clone();
    let writer = tokio::spawn(writer_loop(writer_inner, remote, conn.clone(), rx));
    inner
        .tasks
        .lock()
        .expect("task list poisoned")
        .push(writer);

    // Wait for the peer to go away. This is the ONLY thing the host awaits on an observer
    // connection after the Hello: QUIC's own ACKs detect a dead peer, so there is no
    // application heartbeat and therefore no reason to ever read observer bytes.
    conn.closed().await;
    inner.live.lock().expect("live map poisoned").remove(&remote);
    let _ = inner
        .grants
        .touch_last_seen(&remote, chrono::Utc::now().timestamp());
    (inner.events)(ObserveEvent::Peers);
    Ok(())
}

/// Read the one and only message an observer may send.
///
/// The single `accept_bi` in this module. The stream is capped at
/// [`MAX_HELLO_BYTES`], must arrive within [`HELLO_DEADLINE`], and the [`RecvStream`] is
/// dropped when this returns, so there is no path that reads observer bytes afterwards.
async fn read_hello(conn: &Connection) -> Result<zeroize::Zeroizing<[u8; SECRET_LEN]>> {
    let (send, mut recv) = tokio::time::timeout(HELLO_DEADLINE, conn.accept_bi())
        .await
        .map_err(|_| {
            conn.close(VarInt::from_u32(CODE_TIMEOUT), b"hello deadline");
            anyhow::anyhow!("observer did not send a hello within {HELLO_DEADLINE:?}")
        })??;
    // Nothing is ever written back on this stream; the ack goes out on a fresh uni stream.
    drop(send);

    let raw = tokio::time::timeout(HELLO_DEADLINE, recv.read_to_end(MAX_HELLO_BYTES))
        .await
        .map_err(|_| anyhow::anyhow!("observer hello stalled"))?
        .context("hello exceeded the 256 byte cap or the stream failed")?;
    drop(recv);

    let msg: ObserverMsg = serde_json::from_slice(&raw).context("hello is not a known message")?;
    let ObserverMsg::Hello { secret, .. } = msg;
    // Wiped on drop at every hop. The secret is a bearer credential right up until the
    // grant is claimed, so it should not outlive this function in a freed heap page.
    let secret = zeroize::Zeroizing::new(secret);
    let bytes = zeroize::Zeroizing::new(hex::decode(&*secret).context("hello secret is not hex")?);
    if bytes.len() != SECRET_LEN {
        anyhow::bail!("hello secret is {} bytes, expected 32", bytes.len());
    }
    let mut out = zeroize::Zeroizing::new([0u8; SECRET_LEN]);
    out.copy_from_slice(&bytes);
    Ok(out)
}

async fn await_approval(inner: &Arc<HostInner>, remote: &[u8; 32], label: &str) -> bool {
    let conn_id = hex::encode(&remote[..8]);
    let approval = PendingApproval {
        conn_id: conn_id.clone(),
        fingerprint: fingerprint(remote),
        grant_label: label.to_string(),
    };
    let (tx, rx) = oneshot::channel();
    inner
        .pending
        .lock()
        .expect("pending poisoned")
        .insert(conn_id.clone(), tx);
    inner
        .pending_meta
        .lock()
        .expect("pending meta poisoned")
        .insert(conn_id.clone(), approval.clone());
    (inner.events)(ObserveEvent::Approval(approval));

    let approved = matches!(
        tokio::time::timeout(APPROVAL_DEADLINE, rx).await,
        Ok(Ok(true))
    );
    inner
        .pending
        .lock()
        .expect("pending poisoned")
        .remove(&conn_id);
    inner
        .pending_meta
        .lock()
        .expect("pending meta poisoned")
        .remove(&conn_id);
    approved
}

fn grant_salt(inner: &Arc<HostInner>, grant_id: &str) -> Result<[u8; 16]> {
    // The salt is derived from the verifier, which is derived from the secret; the host no
    // longer has the secret, so it is rebuilt from the stored verifier via the grant id.
    let raw = hex::decode(grant_id).context("grant id is not hex")?;
    let mut token_id = [0u8; 8];
    if raw.len() != 8 {
        anyhow::bail!("grant id must be 8 bytes");
    }
    token_id.copy_from_slice(&raw);
    inner.grants.salt_for(&token_id)
}

/// Push frames to every live observer, at most once per [`FRAME_INTERVAL`] and only when
/// that observer's projected view actually changed.
async fn broadcast_loop(inner: Arc<HostInner>) {
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;

        // Snapshot the ids first so the projection (which can be slow) never runs while
        // the live map is locked.
        let targets: Vec<BroadcastTarget> = {
            let live = inner.live.lock().expect("live map poisoned");
            live.iter()
                .map(|(id, o)| (*id, o.salt, o.last_frame, o.tx.clone()))
                .collect()
        };
        if targets.is_empty() {
            continue;
        }

        for (id, salt, last, tx) in targets {
            // Re-checked before EVERY frame. A revoke that raced an in-flight frame still
            // stops the next one, which is what makes revocation a guarantee rather than a
            // best effort.
            if !inner
                .allow
                .read()
                .expect("allow set poisoned")
                .contains(&id)
            {
                continue;
            }
            let Some(state) = (inner.frames)(&salt) else {
                continue;
            };
            let Ok(json) = serde_json::to_string(&state) else {
                continue;
            };
            let hash = crate::util::hash64(json.as_bytes());
            if hash == last {
                continue; // nothing changed: send nothing at all
            }
            // try_send, never send: a stalled observer drops frames instead of stalling
            // this loop, which is shared with every other observer.
            if tx.try_send(json).is_ok() {
                if let Some(o) = inner.live.lock().expect("live map poisoned").get_mut(&id) {
                    o.last_frame = hash;
                }
            }
        }
    }
}

async fn writer_loop(
    inner: Arc<HostInner>,
    remote: [u8; 32],
    conn: Connection,
    mut rx: mpsc::Receiver<String>,
) {
    while let Some(json) = rx.recv().await {
        if !inner
            .allow
            .read()
            .expect("allow set poisoned")
            .contains(&remote)
        {
            break;
        }
        let Ok(mut stream) = conn.open_uni().await else {
            break;
        };
        if stream.write_all(json.as_bytes()).await.is_err() {
            break;
        }
        if stream.finish().is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Observer side
// ---------------------------------------------------------------------------

/// A live connection to one host, from the watching side.
pub struct PeerConnection {
    pub host_id: [u8; 32],
    pub ack: HelloAck,
    conn: Connection,
}

impl PeerConnection {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.host_id)
    }

    pub fn close(&self) {
        self.conn.close(VarInt::from_u32(0), b"bye");
    }

    /// Read the next frame. `None` once the host closes the connection.
    ///
    /// The observer reads uni streams the host opened; it never opens one back.
    pub async fn next_frame(&self) -> Option<ObservedState> {
        loop {
            let mut recv = self.conn.accept_uni().await.ok()?;
            // Frames are bounded by the projection's own caps (64 agents, 12 activity
            // entries, 64-char strings); 1 MiB is a generous ceiling that still refuses a
            // host trying to exhaust an observer's memory.
            let raw = recv.read_to_end(1024 * 1024).await.ok()?;
            if let Ok(state) = serde_json::from_slice::<ObservedState>(&raw) {
                return Some(state);
            }
            // Not a frame (the HelloAck arrives on the first uni stream): keep reading.
        }
    }
}

/// Redeem a token against its host and open the observation stream.
pub async fn connect_with_token(ep: &Endpoint, token: &ObserverToken) -> Result<PeerConnection> {
    let host_id = EndpointId::from_bytes(&token.endpoint_id).context("token host id is invalid")?;
    let conn = ep
        .connect(EndpointAddr::new(host_id), ALPN)
        .await
        .context("dial host")?;
    send_hello(&conn, &token.secret).await?;
    finish_connect(conn, token.endpoint_id).await
}

/// Same, but dialling an explicit address. Used by the loopback test so it exercises the
/// real handshake without a relay or DNS lookup.
pub async fn connect_with_token_at(
    ep: &Endpoint,
    token: &ObserverToken,
    addr: EndpointAddr,
) -> Result<PeerConnection> {
    let conn = ep.connect(addr, ALPN).await.context("dial host")?;
    send_hello(&conn, &token.secret).await?;
    finish_connect(conn, token.endpoint_id).await
}

// NOTE: the HOST side already supports token-free reconnection (an endpoint already in the
// allow set skips the Hello entirely; see `handle_incoming`). The matching observer-side
// reconnect loop is NOT implemented: after a drop, the peer is marked disconnected and the
// user re-adds it. Shipping an untested dial path in this module would be worse than the
// gap, so it is left out rather than stubbed.

async fn send_hello(conn: &Connection, secret: &[u8; SECRET_LEN]) -> Result<()> {
    let msg = ObserverMsg::Hello {
        secret: hex::encode(secret),
        client_version: 1,
    };
    let body = serde_json::to_vec(&msg)?;
    anyhow::ensure!(
        body.len() <= MAX_HELLO_BYTES,
        "hello is {} bytes, over the {MAX_HELLO_BYTES} cap",
        body.len()
    );
    let (mut send, recv) = conn.open_bi().await?;
    // The host never writes here; the ack comes back on a uni stream it opens.
    drop(recv);
    send.write_all(&body).await?;
    send.finish()?;
    Ok(())
}

async fn finish_connect(conn: Connection, host_id: [u8; 32]) -> Result<PeerConnection> {
    let mut recv = tokio::time::timeout(Duration::from_secs(30), conn.accept_uni())
        .await
        .context("host did not acknowledge in time")??;
    let raw = recv.read_to_end(4096).await?;
    let ack: HelloAck = serde_json::from_slice(&raw).context("host ack was not readable")?;
    Ok(PeerConnection {
        host_id,
        ack,
        conn,
    })
}

/// Bind an endpoint for the watching side.
pub async fn bind_observer_endpoint(mode: BindMode) -> Result<Endpoint> {
    bind_endpoint(SecretKey::generate(), mode).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// The production half of this file, with comments and the test module stripped. The
    /// tests below grep the real code, so they must not read their own assertion strings.
    fn production_source() -> String {
        let src = include_str!("transport.rs");
        let end = src
            .find("#[cfg(test)]")
            .expect("this file has a test module");
        src[..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The invariant the whole module exists to hold: the transport has no way to name the
    /// un-redacted radar types, so the only thing it CAN serialize outward is the
    /// projection. A grep rather than a type check because the point is the absence of a
    /// path, and absence is what a reviewer has to be able to confirm cheaply.
    #[test]
    fn transport_cannot_name_the_radar() {
        let code = production_source();
        for forbidden in ["RadarState", "RadarAgent", "crate::radar", "RadarStateCache"] {
            assert!(
                !code.contains(forbidden),
                "transport.rs must not reference {forbidden}: only ObservedState may cross the wire"
            );
        }
    }

    /// The host reads observer bytes in exactly one place. If a second `accept_bi` (or any
    /// `accept_uni` / `read_datagram`) appears on the host path, read-only stopped being a
    /// property of the protocol shape.
    #[test]
    fn host_reads_observer_bytes_in_exactly_one_place() {
        let src = production_source();
        let count = |needle: &str| src.lines().filter(|l| l.contains(needle)).count();
        assert_eq!(
            count("conn.accept_bi()"),
            1,
            "the host must accept exactly one bi stream, in read_hello"
        );
        assert_eq!(
            count("read_datagram"),
            0,
            "the host must never read datagrams from an observer"
        );
        // The single accept_uni belongs to PeerConnection, the OBSERVER side.
        assert_eq!(
            count("self.conn.accept_uni()"),
            1,
            "only the observer accepts uni streams"
        );
    }

    #[test]
    fn observer_protocol_has_exactly_one_message_variant() {
        // A compile-time exhaustiveness check: adding a variant breaks this match, which
        // is the visible diff the design asks for.
        let msg = ObserverMsg::Hello {
            secret: hex::encode([1u8; 32]),
            client_version: 1,
        };
        match &msg {
            ObserverMsg::Hello { .. } => {}
        }
        let encoded = serde_json::to_vec(&msg).unwrap();
        assert!(
            encoded.len() <= MAX_HELLO_BYTES,
            "hello is {} bytes, over the cap",
            encoded.len()
        );
    }

    #[test]
    fn failure_window_opens_a_cooldown_only_after_the_budget_is_spent() {
        let mut w = FailureWindow::default();
        let t0 = Instant::now();
        for i in 0..MAX_FAILURES - 1 {
            w.record(t0 + Duration::from_millis(i as u64));
            assert!(!w.blocked(t0), "must not block before the budget is spent");
        }
        w.record(t0 + Duration::from_millis(MAX_FAILURES as u64));
        assert!(w.blocked(t0 + Duration::from_secs(1)), "budget spent, must block");
    }

    #[test]
    fn the_cooldown_expires_and_the_budget_resets() {
        let mut w = FailureWindow::default();
        let t0 = Instant::now();
        for i in 0..MAX_FAILURES {
            w.record(t0 + Duration::from_millis(i as u64));
        }
        assert!(w.blocked(t0 + Duration::from_secs(1)));
        // Past the cooldown the host accepts connections again, and a single later
        // failure must not re-trip it immediately.
        let after = t0 + FAILURE_COOLDOWN + Duration::from_secs(1);
        assert!(!w.blocked(after));
        w.record(after);
        assert!(!w.blocked(after + Duration::from_secs(1)));
    }

    #[test]
    fn failures_spread_beyond_the_window_never_trip_the_guard() {
        // A friend fat-fingering a token once an hour is not an attack.
        let mut w = FailureWindow::default();
        let t0 = Instant::now();
        for i in 0..MAX_FAILURES * 3 {
            let t = t0 + FAILURE_WINDOW * 2 * (i as u32);
            w.record(t);
            assert!(!w.blocked(t), "sparse failures must never block");
        }
    }

    #[test]
    fn fingerprint_is_eight_groups_of_four() {
        let fp = fingerprint(&[0xABu8; 32]);
        let groups: Vec<&str> = fp.split('-').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 4));
    }

    #[test]
    fn a_wrong_sized_key_file_errors_rather_than_regenerating() {
        // Silently regenerating would invalidate every grant bound to the old key, and the
        // user's only symptom would be that sharing mysteriously stopped working.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observer_key");
        std::fs::write(&path, b"too short").unwrap();
        assert!(load_or_create_secret_key(&path).is_err());
    }

    #[test]
    fn key_is_created_private_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observer_key");
        let a = load_or_create_secret_key(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the host private key must not be readable by other users");
        }
        let b = load_or_create_secret_key(&path).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes(), "the identity must survive a restart");
    }

    fn test_host(dir: &std::path::Path) -> (HostShare, Arc<Mutex<Vec<ObserveEvent>>>) {
        test_host_with(dir, GrantStore::new(Store::memory().unwrap()))
    }

    /// A host over a caller-supplied grant store, so a test can restart the host across a
    /// database that outlives it.
    fn test_host_with(
        dir: &std::path::Path,
        grants: GrantStore,
    ) -> (HostShare, Arc<Mutex<Vec<ObserveEvent>>>) {
        let frames: FrameSource = Arc::new(|_salt: &[u8; 16]| {
            Some(ObservedState {
                generated_at: "2026-07-26T00:00:00Z".to_string(),
                agents: Vec::new(),
                truncated: false,
            })
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        let events: EventSink = Arc::new(move |e| {
            sink_seen.lock().expect("event log poisoned").push(e);
        });
        let host = HostShare::new(
            grants,
            frames,
            events,
            "test host".to_string(),
            dir.join("observer_key"),
            BindMode::LocalOnly,
        );
        (host, seen)
    }

    /// "Fully local at rest" is a product claim, so it gets a test: before `start`, the
    /// host has no endpoint and therefore no bound socket.
    #[tokio::test]
    async fn nothing_is_bound_before_sharing_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _) = test_host(dir.path());

        assert!(host.bound_addr().await.is_none());
        assert!(host.endpoint_id().await.is_none());
        let status = host.status().await;
        assert!(!status.sharing);
        assert_eq!(status.endpoint_id, None);
        // The key file is not even created until sharing starts.
        assert!(!dir.path().join("observer_key").exists());

        host.start().await.unwrap();
        let addr = host.bound_addr().await.expect("an endpoint after start");
        assert!(
            addr.ip_addrs().next().is_some(),
            "start must bind a real socket"
        );
        assert!(host.status().await.sharing);

        host.stop().await.unwrap();
        assert!(host.bound_addr().await.is_none());
    }

    /// The crash path: a peer redeems a token, and the host dies before it ever answers
    /// the approval prompt. That peer must not come back as an authorised observer.
    ///
    /// Denial was always durable (the refusal revokes the grant), but a host that quits or
    /// crashes inside `APPROVAL_DEADLINE` writes nothing at all. Redemption is driven by
    /// the PEER, so if it were the only durable trace of the exchange then the default on
    /// disk would be allow, and the next `start()` would seed that peer straight into the
    /// allow set: no token, no prompt, and permanent, since an approved grant deliberately
    /// outlives its token expiry.
    #[tokio::test]
    async fn a_grant_redeemed_but_never_approved_does_not_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("warden.db");
        let observer = [42u8; 32];

        // Run one: sharing is on, and a peer claims a token. This is `handle_incoming`
        // exactly as far as its `redeem` call, the last thing that happens before the host
        // is asked to approve.
        let (host, _) = test_host_with(dir.path(), GrantStore::new(Store::open(&db).unwrap()));
        host.start().await.unwrap();
        let host_id = host.endpoint_id().await.unwrap();
        let (grant, token_str) = host
            .grants()
            .create(host_id, "Mark", 900, Profile::Shapes, 1000)
            .unwrap();
        let secret = *ObserverToken::parse(&token_str).unwrap().secret;
        assert!(matches!(
            host.grants().redeem(&secret, observer, 1001).unwrap(),
            RedeemOutcome::Claimed(_)
        ));

        // ...and the app goes away with the prompt still up. `stop()` runs only because a
        // test cannot leave a bound endpoint behind for the restart below; it writes
        // nothing about approval, so the bytes on disk here are what a tray Quit, a
        // Cmd+Q, or a power loss would leave, which the next two asserts pin down.
        host.stop().await.unwrap();
        let orphan = host.grants().list().unwrap();
        assert_eq!(orphan[0].state, crate::observe::GrantState::Redeemed);
        assert_eq!(
            orphan[0].approved_at, None,
            "nothing may record an approval the host never gave"
        );
        drop(host);

        // Run two: a fresh process over the same database.
        let (restarted, _) = test_host_with(dir.path(), GrantStore::new(Store::open(&db).unwrap()));
        restarted.start().await.unwrap();

        assert!(
            !restarted
                .inner
                .allow
                .read()
                .unwrap()
                .contains(&observer),
            "an unapproved peer must not be seeded into the allow set"
        );
        assert!(
            !restarted
                .grants()
                .active_endpoints()
                .unwrap()
                .contains(&observer),
            "an unapproved peer must never count as an active endpoint"
        );
        // The `known` branch of `handle_incoming` resolves the salt and profile through
        // this call, so a `None` here is what makes the token-free, prompt-free reconnect
        // impossible rather than merely unlikely.
        assert!(
            restarted
                .grants()
                .grant_for_endpoint(&observer)
                .unwrap()
                .is_none(),
            "an unapproved peer must not be able to take the reconnect branch"
        );

        // The orphan is durably dead, not just filtered out of one query, so nothing can
        // promote it later. Its token stays spent: a replay is not a second chance.
        let listed = restarted.grants().list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].grant_id, grant.grant_id);
        assert_eq!(listed[0].state, crate::observe::GrantState::Revoked);
        assert_eq!(
            restarted.grants().redeem(&secret, observer, 1002).unwrap(),
            RedeemOutcome::NotPending
        );
        restarted.stop().await.unwrap();
    }

    /// The end-to-end test: a real grant redeemed over a real QUIC connection between two
    /// endpoints in this process, one frame received, then a revoke that must close the
    /// connection and stop any further frame.
    #[tokio::test]
    async fn loopback_redeem_frame_then_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let (host, events) = test_host(dir.path());
        host.start().await.unwrap();
        let host_addr = host.bound_addr().await.unwrap();
        let host_id = host.endpoint_id().await.unwrap();

        // Auto-approve as soon as the host raises the prompt, standing in for the user
        // clicking Approve. Nothing is sent before this resolves.
        let approver = {
            let host = host.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    let pending = host.pending_approvals();
                    if let Some(p) = pending.first() {
                        host.resolve_approval(&p.conn_id, true);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
        };

        let (grant, token_str) = host
            .grants()
            .create(host_id, "Mark", 900, Profile::Shapes, chrono::Utc::now().timestamp())
            .unwrap();
        let token = ObserverToken::parse(&token_str).unwrap();

        let observer_ep = bind_observer_endpoint(BindMode::LocalOnly).await.unwrap();
        let peer = connect_with_token_at(&observer_ep, &token, host_addr)
            .await
            .expect("redeem over a real connection");
        approver.await.unwrap();

        assert_eq!(peer.ack.host_label, "test host");
        assert_eq!(peer.host_id, host_id);

        let frame = tokio::time::timeout(Duration::from_secs(10), peer.next_frame())
            .await
            .expect("a frame within 10s")
            .expect("a frame, not a closed connection");
        assert_eq!(frame.generated_at, "2026-07-26T00:00:00Z");

        // The grant is now redeemed and bound to the observer's real endpoint id.
        let listed = host.grants().list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, crate::observe::GrantState::Redeemed);
        assert!(
            listed[0].approved_at.is_some(),
            "the host's approval must be on disk before any frame goes out, or a restart \
             would either lock the friend out or, worse, let them back in unapproved"
        );
        assert_eq!(
            listed[0].redeemed_by,
            Some(*observer_ep.id().as_bytes()),
            "the grant must bind to the endpoint that actually connected"
        );
        assert!(host.is_connected(observer_ep.id().as_bytes()));

        // Revoke: the live connection must close and no further frame may arrive.
        host.revoke(&grant.grant_id).unwrap();
        let after = tokio::time::timeout(Duration::from_secs(10), peer.next_frame()).await;
        match after {
            Ok(None) => {}
            Ok(Some(_)) => panic!("a frame arrived after revocation"),
            Err(_) => panic!("the connection did not close after revocation"),
        }
        assert!(
            !host
                .inner
                .allow
                .read()
                .unwrap()
                .contains(observer_ep.id().as_bytes()),
            "revocation must drop the endpoint from the allow set"
        );

        {
            let log = events.lock().unwrap();
            assert!(log.iter().any(|e| matches!(e, ObserveEvent::Approval(_))));
        }
        host.stop().await.unwrap();
    }

    /// A second redemption of the same token must fail at the transport, not just in the
    /// store: this is the interception alarm the UX is built around.
    #[tokio::test]
    async fn a_token_cannot_be_redeemed_twice_over_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _) = test_host(dir.path());
        host.start().await.unwrap();
        let host_addr = host.bound_addr().await.unwrap();
        let host_id = host.endpoint_id().await.unwrap();

        let approver = {
            let host = host.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    if let Some(p) = host.pending_approvals().first() {
                        host.resolve_approval(&p.conn_id, true);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
        };

        let (_, token_str) = host
            .grants()
            .create(host_id, "Mark", 900, Profile::Shapes, chrono::Utc::now().timestamp())
            .unwrap();
        let token = ObserverToken::parse(&token_str).unwrap();

        let first_ep = bind_observer_endpoint(BindMode::LocalOnly).await.unwrap();
        let _first = connect_with_token_at(&first_ep, &token, host_addr.clone())
            .await
            .expect("the first redemption wins");
        approver.await.unwrap();

        // A different machine replaying the same token string.
        let second_ep = bind_observer_endpoint(BindMode::LocalOnly).await.unwrap();
        let token2 = ObserverToken::parse(&token_str).unwrap();
        let second = tokio::time::timeout(
            Duration::from_secs(10),
            connect_with_token_at(&second_ep, &token2, host_addr),
        )
        .await
        .expect("the second attempt must resolve, not hang");
        assert!(second.is_err(), "a token is single use on the wire too");

        host.stop().await.unwrap();
    }

    /// A denied observer gets nothing and cannot come back on the grant it claimed.
    #[tokio::test]
    async fn a_denied_observer_receives_no_frame() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _) = test_host(dir.path());
        host.start().await.unwrap();
        let host_addr = host.bound_addr().await.unwrap();
        let host_id = host.endpoint_id().await.unwrap();

        let denier = {
            let host = host.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    if let Some(p) = host.pending_approvals().first() {
                        host.resolve_approval(&p.conn_id, false);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
        };

        let (grant, token_str) = host
            .grants()
            .create(host_id, "Stranger", 900, Profile::Shapes, chrono::Utc::now().timestamp())
            .unwrap();
        let token = ObserverToken::parse(&token_str).unwrap();
        let observer_ep = bind_observer_endpoint(BindMode::LocalOnly).await.unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            connect_with_token_at(&observer_ep, &token, host_addr),
        )
        .await
        .expect("the denial must resolve, not hang");
        denier.await.unwrap();
        assert!(result.is_err(), "a denied observer must not get an ack");

        let listed = host.grants().list().unwrap();
        assert_eq!(
            listed
                .iter()
                .find(|g| g.grant_id == grant.grant_id)
                .map(|g| g.state),
            Some(crate::observe::GrantState::Revoked),
            "denying must kill the grant, not leave it redeemed"
        );
        host.stop().await.unwrap();
    }
}
