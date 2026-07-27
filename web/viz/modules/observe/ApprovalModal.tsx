// ApprovalModal.tsx: the host-side gate for a first-time connection. Nothing is sent to
// an observer until this resolves (`commands.rs`: "Nothing has been sent to them before
// this resolves"), so the modal has exactly one job: stay in front of the user until they
// make an explicit choice. It never closes on Escape, never closes on an outside click,
// and never times out into an implicit approve. Losing any of those three would turn an
// intercepted token into a silent grant.

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { readableError, type PendingApproval } from './observeTypes';

function normalizePending(v: any): PendingApproval | null {
  const connId = typeof v?.connId === 'string' ? v.connId : typeof v?.conn_id === 'string' ? v.conn_id : '';
  if (!connId) return null;
  return {
    connId,
    fingerprint: typeof v?.fingerprint === 'string' ? v.fingerprint : 'unknown fingerprint',
    grantLabel: typeof v?.grantLabel === 'string' ? v.grantLabel : typeof v?.grant_label === 'string' ? v.grant_label : 'unlabeled grant',
  };
}

export function ApprovalModal() {
  const [queue, setQueue] = useState<PendingApproval[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const enqueue = useCallback((incoming: PendingApproval | null) => {
    if (!incoming) return;
    setQueue((q) => (q.some((p) => p.connId === incoming.connId) ? q : [...q, incoming]));
  }, []);

  // Safety net: an approval created before this component mounted (a fresh window open
  // racing a connection attempt) would otherwise never surface, since it only exists as a
  // one-shot push event. Seed the queue from the backend's own list once on mount.
  useEffect(() => {
    invoke('observe_pending_approvals')
      .then((rows) => {
        if (!mountedRef.current || !Array.isArray(rows)) return;
        rows.map(normalizePending).forEach(enqueue);
      })
      .catch((err) => console.error('observe_pending_approvals failed', err));
  }, [enqueue]);

  useEffect(() => {
    const unlisten = listen('observe:approval', (e) => enqueue(normalizePending(e.payload)));
    return () => {
      unlisten.then((f) => f()).catch(() => {});
    };
  }, [enqueue]);

  const current = queue[0] ?? null;

  const resolve = useCallback(
    async (approve: boolean) => {
      if (!current || busy) return;
      setBusy(true);
      setError(null);
      try {
        await invoke('observe_resolve_approval', { connId: current.connId, approve });
        if (!mountedRef.current) return;
        setQueue((q) => q.filter((p) => p.connId !== current.connId));
      } catch (err) {
        if (!mountedRef.current) return;
        setError(readableError(err));
      } finally {
        if (mountedRef.current) setBusy(false);
      }
    },
    [current, busy],
  );

  if (!current) return null;

  return (
    <div className="wd-observe-approval-scrim" role="presentation">
      <div
        className="wd-observe-approval"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="wd-observe-approval-title"
      >
        <div className="wd-observe-approval-kicker">Connection waiting</div>
        <h2 id="wd-observe-approval-title" className="wd-observe-approval-title">
          Someone wants to watch your radar
        </h2>
        <dl className="wd-observe-approval-grid">
          <div>
            <dt>Grant</dt>
            <dd>{current.grantLabel}</dd>
          </div>
          <div>
            <dt>Fingerprint</dt>
            <dd className="wd-observe-mono">{current.fingerprint}</dd>
          </div>
        </dl>
        <p className="wd-observe-approval-note">
          Nothing is sent to them until you answer. Only approve a fingerprint you recognize.
        </p>
        {error ? (
          <p className="wd-observe-error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="wd-observe-approval-actions">
          <button
            type="button"
            className="wd-observe-btn wd-observe-btn-deny"
            disabled={busy}
            onClick={() => resolve(false)}
          >
            Deny
          </button>
          <button
            type="button"
            className="wd-observe-btn wd-observe-btn-approve"
            disabled={busy}
            onClick={() => resolve(true)}
          >
            Approve
          </button>
        </div>
        {queue.length > 1 ? (
          <p className="wd-observe-approval-more">{queue.length - 1} more waiting</p>
        ) : null}
      </div>
    </div>
  );
}

export default ApprovalModal;
