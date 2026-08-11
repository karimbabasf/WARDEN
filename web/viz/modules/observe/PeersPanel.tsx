// PeersPanel.tsx: the observer side, "watch someone else." A small trigger toggles a
// left-hand dock (the radar detail dock already owns the right side): a peer list with an
// add-token form, and, once a peer is selected, that peer's live `ObservedRadarView` fed
// by `observe_peer_state` and refreshed on the `observe:frame` push.
//
// THE DOCK IS A POPOVER, and it behaves like one. It opens out of the Watch button at
// the foot of the fleet rack, over a scrim, on its own z layer, and it closes on the
// scrim, on Escape, and the moment you pick a peer.
//
// It is PORTALLED TO THE BODY, and that is the actual fix rather than a tidy-up. The
// trigger lives in the fleet rack's footer, and `.wd-fleet-body` carries a `transform`
// for the whole life of the panel (its `wd-rail-in` entry animation is `both`-filled,
// so the final keyframe's transform sticks). A transformed ancestor becomes the
// containing block for `position: fixed` descendants, so the dock's `top` and `left`
// were being resolved against the RAIL rather than the viewport: it landed partway
// down the rack, on top of the strips, which is what it looked like. A scrim written
// the same way was trapped in the same box and dimmed nothing at all. Rendering both
// into the body takes them out of that containing block, and the dock is then anchored
// to the trigger's measured rect so it still reads as growing out of the button.
//
// Picking a peer closes it for a second reason, not just tidiness: the peer's board is
// rendered in the SCENE now (WarRoom parks their constellation beside yours and the top
// switcher moves between them), so once you have chosen someone the answer is behind
// the dock, not inside it.

import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
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

/**
 * Where the portalled popover sits, in viewport coordinates.
 *
 * Measured from the trigger rather than written in CSS because the trigger is at the
 * foot of a rack whose height depends on how many sessions are live, so there is no
 * fixed viewport offset that keeps the two together. `getBoundingClientRect` is the
 * right read here (unlike the camera's rail insets, which deliberately avoid it): the
 * trigger's PAINTED position is exactly what the popover has to line up with.
 */
function anchorFor(trigger: HTMLElement | null): CSSProperties | null {
  if (!trigger) return null;
  const box = trigger.getBoundingClientRect();
  if (box.width === 0) return null;
  const rail = trigger.closest('.wd-fleet') as HTMLElement | null;
  const railBox = rail?.getBoundingClientRect();
  return {
    left: railBox && railBox.width > 0 ? railBox.left : box.left,
    width: railBox && railBox.width > 0 ? railBox.width : undefined,
    bottom: Math.max(8, window.innerHeight - box.top + 8),
  };
}

export function PeersPanel({ onWatchedPeer }: PeersPanelProps = {}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const [anchor, setAnchor] = useState<CSSProperties | null>(null);
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

  // Once on mount as well as on open: the watched peer drives a constellation in the
  // scene, so the list has to be known even while the dock is shut, or a peer selected
  // in a previous session would have no row to resolve against.
  useEffect(() => {
    refreshPeers();
  }, [open, refreshPeers]);

  // Re-anchor while open: the rack grows and shrinks as sessions come and go, and the
  // window resizes, and the popover has to stay welded to the button either way.
  useEffect(() => {
    if (!open) {
      setAnchor(null);
      return;
    }
    const measure = () => setAnchor(anchorFor(triggerRef.current));
    measure();
    const raf = window.requestAnimationFrame(measure);
    window.addEventListener('resize', measure);
    return () => {
      window.cancelAnimationFrame(raf);
      window.removeEventListener('resize', measure);
    };
  }, [open, peers.length, selectedId]);

  // Escape closes the popover. Capture phase, and only while open, so it never
  // competes with the radar's own Escape (deselect, then fit-to-overview).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopImmediatePropagation();
      setOpen(false);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [open]);

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

  const popover = (
    <>
      <button
        type="button"
        className="wd-observe-dock-scrim"
        aria-label="Close the watch list"
        onClick={() => setOpen(false)}
      />

      <aside className="wd-observe-dock" aria-label="Remote peers" style={anchor ?? undefined}>
          {selected ? (
            <>
              <div className="wd-observe-dock-head">
                <button type="button" className="wd-observe-back" onClick={() => setSelectedId(null)}>
                  &#8249; Peers
                </button>
                {/* Leaving the detail view is not the same as stopping watching:
                    the board stays in the scene either way, so the way to put it
                    away has to be its own control rather than a back arrow. */}
                <button type="button" className="wd-observe-btn" onClick={() => setSelectedId(null)}>
                  Stop watching
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
                    <PeerRowView
                    key={p.peerId}
                    peer={p}
                    // Picking a peer hands the answer to the SCENE (their
                    // constellation parks beside yours and the top switcher moves
                    // between the two boards), so the popover gets out of the way.
                    onSelect={() => {
                      setSelectedId(p.peerId);
                      setOpen(false);
                    }}
                    onRemove={() => removePeer(p.peerId)}
                  />
                  ))}
                </ul>
              )}
            </>
          )}
      </aside>
    </>
  );

  return (
    <div className="wd-observe-peers">
      <button
        ref={triggerRef}
        type="button"
        className="wd-observe-trigger wd-observe-watch-trigger"
        aria-expanded={open}
        aria-label="Watch a shared machine"
        onClick={() => setOpen((o) => !o)}
      >
        <span aria-hidden>&#9678;</span> Watch
      </button>

      {/* Out of the rail's transform and into the body. See the header note: this is
          the difference between a popover and a panel stuck inside the rack. */}
      {open ? createPortal(popover, document.body) : null}
    </div>
  );
}

export default PeersPanel;
