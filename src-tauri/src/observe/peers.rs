//! The watching side: hosts this machine observes.
//!
//! Live only. Frames are held in memory and replaced by the next one, never written to
//! disk, so there is no stored observer-visible data to leak and no query surface to
//! abuse. What IS persisted is only which hosts we know about, so the list survives a
//! restart.
//!
//! Like [`super::transport`], this module never reads from a host beyond the frames the
//! host chooses to push, and it never opens a stream back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::observe::projection::ObservedState;
use crate::observe::transport::{
    bind_observer_endpoint, connect_with_token_at, fingerprint, BindMode, EventSink, ObserveEvent,
    PeerConnection,
};
use crate::observe::grants::ObserverToken;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerRow {
    pub peer_id: String,
    pub host_label: String,
    pub fingerprint: String,
    pub connected: bool,
    pub last_frame_at: Option<String>,
    pub error: Option<String>,
}

struct PeerRuntime {
    label: String,
    conn: Option<Arc<PeerConnection>>,
    last_state: Option<ObservedState>,
    last_frame_at: Option<i64>,
    error: Option<String>,
    reader: Option<tokio::task::JoinHandle<()>>,
}

struct HubInner {
    store: Store,
    events: EventSink,
    mode: BindMode,
    endpoint: tokio::sync::Mutex<Option<Endpoint>>,
    peers: Mutex<HashMap<[u8; 32], PeerRuntime>>,
}

/// The observer-side registry. Cloneable; all clones share one endpoint and peer map.
#[derive(Clone)]
pub struct ObserverHub {
    inner: Arc<HubInner>,
}

impl ObserverHub {
    pub fn new(store: Store, events: EventSink, mode: BindMode) -> Self {
        Self {
            inner: Arc::new(HubInner {
                store,
                events,
                mode,
                endpoint: tokio::sync::Mutex::new(None),
                peers: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Bound lazily, on the first `add_peer`. Watching, like sharing, binds no socket until
    /// the user asks for it.
    async fn endpoint(&self) -> Result<Endpoint> {
        let mut slot = self.inner.endpoint.lock().await;
        if let Some(ep) = slot.as_ref() {
            return Ok(ep.clone());
        }
        let ep = bind_observer_endpoint(self.inner.mode)
            .await
            .context("bind observer endpoint")?;
        *slot = Some(ep.clone());
        Ok(ep)
    }

    /// Redeem a token, connect, and remember the host.
    ///
    /// The token is consumed here and never stored: from this point the durable credential
    /// is this machine's own key, which the host bound the grant to.
    pub async fn add_peer(&self, token_str: &str) -> Result<PeerRow> {
        let token = ObserverToken::parse(token_str).map_err(|e| anyhow::anyhow!("{e}"))?;
        let host_id = token.endpoint_id;
        let ep = self.endpoint().await?;
        let addr = EndpointAddr::new(
            EndpointId::from_bytes(&host_id).context("token host id is invalid")?,
        );
        let conn = connect_with_token_at(&ep, &token, addr)
            .await
            .context("connect to host")?;
        let label = conn.ack.host_label.clone();
        self.persist(&host_id, &label)?;
        self.install(host_id, label.clone(), Arc::new(conn));
        (self.inner.events)(ObserveEvent::Peers);
        Ok(self.row(&host_id, &label))
    }

    /// Attach an already-open connection. Used by the loopback tests, which dial an
    /// explicit address rather than resolving one through the relay network.
    pub fn attach(&self, conn: PeerConnection) -> Result<PeerRow> {
        let host_id = conn.host_id;
        let label = conn.ack.host_label.clone();
        self.persist(&host_id, &label)?;
        self.install(host_id, label.clone(), Arc::new(conn));
        (self.inner.events)(ObserveEvent::Peers);
        Ok(self.row(&host_id, &label))
    }

    fn persist(&self, host_id: &[u8; 32], label: &str) -> Result<()> {
        self.inner.store.with_conn(|c| {
            c.execute(
                "INSERT INTO observer_peers(peer_id,host_label,added_at) VALUES(?,?,?) \
                 ON CONFLICT(peer_id) DO UPDATE SET host_label=excluded.host_label",
                params![host_id.as_slice(), label, chrono::Utc::now().timestamp()],
            )
            .context("persist observer peer")?;
            Ok(())
        })
    }

    fn install(&self, host_id: [u8; 32], label: String, conn: Arc<PeerConnection>) {
        let hub = self.clone();
        let reader_conn = conn.clone();
        let reader = tokio::spawn(async move {
            while let Some(state) = reader_conn.next_frame().await {
                {
                    let mut peers = hub.inner.peers.lock().expect("peer map poisoned");
                    if let Some(p) = peers.get_mut(&host_id) {
                        p.last_state = Some(state);
                        p.last_frame_at = Some(chrono::Utc::now().timestamp());
                    } else {
                        return;
                    }
                }
                (hub.inner.events)(ObserveEvent::Frame {
                    peer_id: hex::encode(host_id),
                });
            }
            // The host closed the connection: revoked, stopped sharing, or went offline.
            // We cannot tell which from here, so the message stays honest about that.
            let mut peers = hub.inner.peers.lock().expect("peer map poisoned");
            if let Some(p) = peers.get_mut(&host_id) {
                p.conn = None;
                p.error = Some("disconnected by host".to_string());
            }
            (hub.inner.events)(ObserveEvent::Peers);
        });

        let mut peers = self.inner.peers.lock().expect("peer map poisoned");
        if let Some(old) = peers.insert(
            host_id,
            PeerRuntime {
                label,
                conn: Some(conn),
                last_state: None,
                last_frame_at: None,
                error: None,
                reader: Some(reader),
            },
        ) {
            if let Some(t) = old.reader {
                t.abort();
            }
            if let Some(c) = old.conn {
                c.close();
            }
        }
    }

    fn row(&self, host_id: &[u8; 32], fallback_label: &str) -> PeerRow {
        let peers = self.inner.peers.lock().expect("peer map poisoned");
        let rt = peers.get(host_id);
        PeerRow {
            peer_id: hex::encode(host_id),
            host_label: rt.map(|r| r.label.clone()).unwrap_or_else(|| fallback_label.to_string()),
            fingerprint: fingerprint(host_id),
            connected: rt.map(|r| r.conn.is_some()).unwrap_or(false),
            last_frame_at: rt
                .and_then(|r| r.last_frame_at)
                .and_then(unix_to_rfc3339),
            error: rt.and_then(|r| r.error.clone()),
        }
    }

    /// Every known host, persisted ones included, whether or not they are connected now.
    pub fn list(&self) -> Result<Vec<PeerRow>> {
        let stored: Vec<([u8; 32], String)> = self.inner.store.with_conn(|c| {
            let mut st = c
                .prepare("SELECT peer_id,host_label FROM observer_peers ORDER BY added_at DESC")
                .context("prepare peer list")?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?)))
                .context("query peer list")?;
            let mut out = Vec::new();
            for row in rows {
                let (raw, label) = row.context("read peer row")?;
                if raw.len() == 32 {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(&raw);
                    out.push((id, label));
                }
            }
            Ok(out)
        })?;
        Ok(stored
            .iter()
            .map(|(id, label)| self.row(id, label))
            .collect())
    }

    /// The latest frame from one host, or `None` until the first one lands.
    pub fn peer_state(&self, peer_id: &str) -> Result<Option<ObservedState>> {
        let id = decode_peer_id(peer_id)?;
        Ok(self
            .inner
            .peers
            .lock()
            .expect("peer map poisoned")
            .get(&id)
            .and_then(|p| p.last_state.clone()))
    }

    pub fn remove_peer(&self, peer_id: &str) -> Result<()> {
        let id = decode_peer_id(peer_id)?;
        if let Some(rt) = self.inner.peers.lock().expect("peer map poisoned").remove(&id) {
            if let Some(t) = rt.reader {
                t.abort();
            }
            if let Some(c) = rt.conn {
                c.close();
            }
        }
        self.inner.store.with_conn(|c| {
            c.execute(
                "DELETE FROM observer_peers WHERE peer_id=?",
                params![id.as_slice()],
            )
            .context("delete observer peer")?;
            Ok(())
        })?;
        (self.inner.events)(ObserveEvent::Peers);
        Ok(())
    }
}

fn decode_peer_id(peer_id: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(peer_id).context("peer id is not hex")?;
    if raw.len() != 32 {
        anyhow::bail!("peer id must be 32 bytes");
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&raw);
    Ok(id)
}

pub(crate) fn unix_to_rfc3339(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::grants::GrantStore;
    use crate::observe::projection::Profile;
    use crate::observe::transport::{FrameSource, HostShare};
    use std::time::Duration;

    #[test]
    fn peer_ids_must_be_32_bytes_of_hex() {
        assert!(decode_peer_id("nothex").is_err());
        assert!(decode_peer_id(&hex::encode([1u8; 8])).is_err());
        assert!(decode_peer_id(&hex::encode([1u8; 32])).is_ok());
    }

    #[test]
    fn an_unknown_peer_has_no_state() {
        let hub = ObserverHub::new(
            Store::memory().unwrap(),
            Arc::new(|_| {}),
            BindMode::LocalOnly,
        );
        assert!(hub.peer_state(&hex::encode([9u8; 32])).unwrap().is_none());
        assert!(hub.list().unwrap().is_empty());
    }

    /// The observer half of the loopback path: a real grant, a real connection, a frame
    /// surfaced through the same `peer_state` the IPC command calls, and a peer list that
    /// survives because it is persisted.
    #[tokio::test]
    async fn peer_receives_a_frame_and_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let host_store = Store::memory().unwrap();
        let frames: FrameSource = Arc::new(|_salt: &[u8; 16]| {
            Some(ObservedState {
                generated_at: "2026-07-26T12:00:00Z".to_string(),
                agents: Vec::new(),
                truncated: false,
            })
        });
        let host = HostShare::new(
            GrantStore::new(host_store),
            frames,
            Arc::new(|_| {}),
            "Karim's mac".to_string(),
            dir.path().join("observer_key"),
            BindMode::LocalOnly,
        );
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
            .create(
                host_id,
                "Mark",
                900,
                Profile::Shapes,
                chrono::Utc::now().timestamp(),
            )
            .unwrap();
        let token = ObserverToken::parse(&token_str).unwrap();

        let hub = ObserverHub::new(
            Store::memory().unwrap(),
            Arc::new(|_| {}),
            BindMode::LocalOnly,
        );
        let ep = hub.endpoint().await.unwrap();
        let conn = connect_with_token_at(&ep, &token, host_addr).await.unwrap();
        approver.await.unwrap();
        let row = hub.attach(conn).unwrap();
        assert_eq!(row.host_label, "Karim's mac");
        assert!(row.connected);
        assert_eq!(row.peer_id, hex::encode(host_id));

        // The first frame lands within a couple of broadcast ticks.
        let mut state = None;
        for _ in 0..80 {
            if let Some(s) = hub.peer_state(&row.peer_id).unwrap() {
                state = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(125)).await;
        }
        let state = state.expect("a frame should reach the observer");
        assert_eq!(state.generated_at, "2026-07-26T12:00:00Z");

        let listed = hub.list().unwrap();
        assert_eq!(listed.len(), 1, "the peer must be persisted, not just live");
        assert_eq!(listed[0].peer_id, hex::encode(host_id));
        assert!(listed[0].last_frame_at.is_some());

        hub.remove_peer(&row.peer_id).unwrap();
        assert!(hub.list().unwrap().is_empty());
        host.stop().await.unwrap();
    }
}
