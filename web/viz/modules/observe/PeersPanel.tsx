// PeersPanel.tsx: the observer side, "watch someone else." A small trigger toggles a
// left-hand dock (the radar detail dock already owns the right side): a peer list with an
// add-token form, and, once a peer is selected, that peer's live `ObservedRadarView` fed
// by `observe_peer_state` and refreshed on the `observe:frame` push.

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { normalizeObservedState, type ObservedState } from '@/viz/shared/types/observedTypes';
import { normalizePeerRow, readableError, type PeerRow } from './observeTypes';
import { isoRelative } from './observeFormat';
import { ObservedRadarView } from './ObservedRadarView';

function PeerRowView({ peer, onSelect, onRemove }: { peer: PeerRow; onSelect: () => void; onRemove: () => void }) {
  return (
    <li className="wd-observe-peer-row" data-peer-row={peer.peerId}>
      <button type="button" className="wd-observe-peer-btn" onClick={onSelect}>
        <span
          className={`wd-observe-dot${peer.connected ? ' is-connected' : ''}`}
          role="img"
          aria-label={peer.connected ? 'connected' : 'not connected'}
        />
        <span className="wd-observe-peer-main">
          <span className="wd-observe-peer-label">{peer.hostLabel}</span>
          <span className="wd-observe-mono wd-observe-peer-fp">{peer.fingerprint}</span>
        </span>
        <span className="wd-observe-peer-meta">
          {peer.error ? (
            <span className="wd-observe-peer-error">{peer.error}</span>
          ) : peer.lastFrameAt ? (
            <span>{isoRelative(peer.lastFrameAt) || 'last frame unknown'}</span>
          ) : (
            <span>no frame yet</span>
          )}
        </span>
      </button>
      <button type="button" className="wd-observe-peer-remove" aria-label={`Remove ${peer.hostLabel}`} onClick={onRemove}>
        &#10005;
      </button>
    </li>
  );
}

export type PeersPanelProps = {
  /**
   * Lifts the currently-watched peer out of this dock so the war room can park
   * their constellation beside the local one and truck the camera across to it.
   *
   * Called with `(null, null)` when no peer is selected. The dock keeps owning
   * the LIST and the token form; only the "which board am I looking at" question
   * is lifted, because that answer belongs to the scene, not to a sidebar.
   */
  onWatchedPeer?: (peer: { id: string; label: string } | null, state: ObservedState | null) => void;
};

export function PeersPanel({ onWatchedPeer }: PeersPanelProps = {}) {
  const [open, setOpen] = useState(false);
  const [peers, setPeers] = useState<PeerRow[]>([]);
  const [listError, setListError] = useState<string | null>(null);

  const [token, setToken] = useState('');
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [peerState, setPeerState] = useState<ObservedState | null>(null);
  const [stateError, setStateError] = useState<string | null>(null);

  const refreshPeers = useCallback(() => {
    invoke('observe_list_peers')
      .then((rows) => setPeers(Array.isArray(rows) ? rows.map(normalizePeerRow) : []))
      .catch((err) => setListError(readableError(err)));
  }, []);

  useEffect(() => {
    if (!open) return;
    refreshPeers();
  }, [open, refreshPeers]);

  useEffect(() => {
    const unlisten = listen('observe:peers', refreshPeers);
    return () => {
      unlisten.then((f) => f()).catch(() => {});
    };
  }, [refreshPeers]);

  const selected = peers.find((p) => p.peerId === selectedId) ?? null;

  // Publish the watched peer upward whenever the selection or its frame changes.
  // Keyed on the frame itself, so every push (`observe:frame`) slides fresh data
  // into the scene without the dock having to know a constellation exists.
  useEffect(() => {
    if (!onWatchedPeer) return;
    if (!selected) {
      onWatchedPeer(null, null);
      return;
    }
    onWatchedPeer({ id: selected.peerId, label: selected.hostLabel }, peerState);
  }, [onWatchedPeer, selected, peerState]);

  const fetchPeerState = useCallback((peerId: string) => {
    invoke('observe_peer_state', { peerId })
      .then((raw) => setPeerState(raw == null ? null : normalizeObservedState(raw)))
      .catch((err) => setStateError(readableError(err)));
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setPeerState(null);
      return;
    }
    setStateError(null);
    fetchPeerState(selectedId);
  }, [selectedId, fetchPeerState]);

  useEffect(() => {
    if (!selectedId) return;
    const unlisten = listen<{ peerId: string }>('observe:frame', (e) => {
      if (e.payload?.peerId === selectedId) fetchPeerState(selectedId);
    });
    return () => {
      unlisten.then((f) => f()).catch(() => {});
    };
  }, [selectedId, fetchPeerState]);

  const addPeer = useCallback(async () => {
    const trimmed = token.trim();
    if (!trimmed) return;
    setAdding(true);
    setAddError(null);
    try {
      await invoke('observe_add_peer', { token: trimmed });
      setToken('');
      refreshPeers();
    } catch (err) {
      setAddError(readableError(err));
    } finally {
      setAdding(false);
    }
  }, [token, refreshPeers]);

  const removePeer = useCallback(
    async (peerId: string) => {
      try {
        await invoke('observe_remove_peer', { peerId });
        if (selectedId === peerId) setSelectedId(null);
        refreshPeers();
      } catch (err) {
        setListError(readableError(err));
      }
    },
    [selectedId, refreshPeers],
  );

  return (
    <div className="wd-observe-peers">
      <button
        type="button"
        className="wd-observe-trigger wd-observe-watch-trigger"
        aria-expanded={open}
        aria-label="Watch a shared machine"
        onClick={() => setOpen((o) => !o)}
      >
        <span aria-hidden>&#9678;</span> Watch
      </button>

      {open ? (
        <aside className="wd-observe-dock" aria-label="Remote peers">
          {selected ? (
            <>
              <div className="wd-observe-dock-head">
                <button type="button" className="wd-observe-back" onClick={() => setSelectedId(null)}>
                  &#8249; Peers
                </button>
              </div>
              <div className="wd-observe-peer-detail-head">
                <span
                  className={`wd-observe-dot${selected.connected ? ' is-connected' : ''}`}
                  role="img"
                  aria-label={selected.connected ? 'connected' : 'not connected'}
                />
                <span>{selected.hostLabel}</span>
                <span className="wd-observe-mono">{selected.fingerprint}</span>
              </div>
              {selected.error ? (
                <p className="wd-observe-error" role="alert">
                  {selected.error}
                </p>
              ) : null}
              {stateError ? (
                <p className="wd-observe-error" role="alert">
                  {stateError}
                </p>
              ) : (
                <ObservedRadarView state={peerState} title={selected.hostLabel} subtitle={selected.fingerprint} />
              )}
            </>
          ) : (
            <>
              <div className="wd-observe-dock-head">
                <span className="wd-card-kicker">Watching</span>
              </div>
              <div className="wd-observe-add">
                <input
                  type="text"
                  className="wd-observe-token-input"
                  placeholder="Paste a token"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  aria-label="Peer token"
                />
                <button type="button" className="wd-observe-btn wd-observe-btn-primary" disabled={adding || !token.trim()} onClick={addPeer}>
                  {adding ? 'Adding...' : 'Add'}
                </button>
              </div>
              {addError ? (
                <p className="wd-observe-error" role="alert">
                  {addError}
                </p>
              ) : null}
              {listError ? (
                <p className="wd-observe-error" role="alert">
                  {listError}
                </p>
              ) : null}
              {peers.length === 0 ? (
                <p className="wd-observe-empty-inline">Not watching anyone yet.</p>
              ) : (
                <ul className="wd-observe-peer-list">
                  {peers.map((p) => (
                    <PeerRowView key={p.peerId} peer={p} onSelect={() => setSelectedId(p.peerId)} onRemove={() => removePeer(p.peerId)} />
                  ))}
                </ul>
              )}
            </>
          )}
        </aside>
      ) : null}
    </div>
  );
}

export default PeersPanel;
