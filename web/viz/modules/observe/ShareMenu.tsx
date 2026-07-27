// ShareMenu.tsx: the host-side "share my machine" control, tucked behind a `...` button
// in the chrome so it stays out of the way until someone wants it. Two tabs:
//   SHARE   start/stop sharing (lazily binds the endpoint on first press), mint a grant,
//           show the once-only token, preview what an observer would see.
//   ACCESS  the grant table (AccessTab.tsx), refreshed on the `observe:grants` push.
//
// Every `invoke` here is caught: a rejected promise becomes a readable error string in
// state, never an unhandled rejection.

import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  GRANT_TTL_OPTIONS,
  normalizeGrantRow,
  normalizeNewGrant,
  normalizeSharingStatus,
  readableError,
  type GrantRow,
  type NewGrant,
  type SharingStatus,
} from './observeTypes';
import { AccessTab } from './AccessTab';
import { PreviewModal } from './PreviewModal';

type Tab = 'share' | 'access';

function ShareTab({
  status,
  statusBusy,
  statusError,
  onStart,
  onStop,
  label,
  onLabelChange,
  ttlSecs,
  onTtlChange,
  minting,
  mintError,
  onMint,
  newGrant,
  copied,
  onCopy,
  onPreview,
}: {
  status: SharingStatus | null;
  statusBusy: boolean;
  statusError: string | null;
  onStart: () => void;
  onStop: () => void;
  label: string;
  onLabelChange: (v: string) => void;
  ttlSecs: number;
  onTtlChange: (v: number) => void;
  minting: boolean;
  mintError: string | null;
  onMint: (e: FormEvent) => void;
  newGrant: NewGrant | null;
  copied: boolean;
  onCopy: () => void;
  onPreview: () => void;
}) {
  const sharing = status?.sharing === true;
  return (
    <div className="wd-observe-share" data-tab="share">
      <div className="wd-observe-toggle-row">
        <div>
          <div className="wd-observe-toggle-label">Share this machine</div>
          <p className="wd-observe-toggle-note">
            {sharing
              ? 'Nothing else changes until you mint a grant for a friend below.'
              : 'Nothing is exposed until you press this. No socket opens and no relay is contacted before then.'}
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={sharing}
          className={`wd-observe-switch${sharing ? ' is-on' : ''}`}
          disabled={statusBusy}
          onClick={sharing ? onStop : onStart}
        >
          <span className="wd-observe-switch-knob" aria-hidden />
        </button>
      </div>

      {statusError ? (
        <p className="wd-observe-error" role="alert">
          {statusError}
        </p>
      ) : null}

      {sharing && status?.fingerprint ? (
        <div className="wd-observe-fingerprint">
          <span className="wd-observe-fingerprint-label">Your fingerprint</span>
          <span className="wd-observe-mono">{status.fingerprint}</span>
        </div>
      ) : null}

      {sharing ? (
        <>
          <form className="wd-observe-form" onSubmit={onMint}>
            <label className="wd-observe-field">
              <span>Friend label</span>
              <input
                type="text"
                value={label}
                maxLength={80}
                placeholder="e.g. Askhat"
                onChange={(e) => onLabelChange(e.target.value)}
              />
            </label>
            <label className="wd-observe-field">
              <span>Expires in</span>
              <select value={ttlSecs} onChange={(e) => onTtlChange(Number(e.target.value))}>
                {GRANT_TTL_OPTIONS.map((o) => (
                  <option key={o.secs} value={o.secs}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
            <button type="submit" className="wd-observe-btn wd-observe-btn-primary" disabled={minting || !label.trim()}>
              {minting ? 'Minting...' : 'Create grant'}
            </button>
          </form>
          {mintError ? (
            <p className="wd-observe-error" role="alert">
              {mintError}
            </p>
          ) : null}

          {newGrant ? (
            <div className="wd-observe-token" data-new-grant>
              <p className="wd-observe-token-warn">
                This token is shown once. It cannot be retrieved again, copy it now.
              </p>
              <div className="wd-observe-token-row">
                <code className="wd-observe-mono">{newGrant.token}</code>
                <button type="button" className="wd-observe-btn wd-observe-btn-ghost" onClick={onCopy}>
                  {copied ? 'Copied' : 'Copy'}
                </button>
              </div>
            </div>
          ) : null}

          <button type="button" className="wd-observe-btn wd-observe-btn-ghost wd-observe-preview-btn" onClick={onPreview}>
            Preview what others see
          </button>
        </>
      ) : null}
    </div>
  );
}

export function ShareMenu() {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Tab>('share');
  const rootRef = useRef<HTMLDivElement>(null);

  const [status, setStatus] = useState<SharingStatus | null>(null);
  const [statusBusy, setStatusBusy] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);

  const [label, setLabel] = useState('');
  const [ttlSecs, setTtlSecs] = useState<number>(GRANT_TTL_OPTIONS[0].secs);
  const [minting, setMinting] = useState(false);
  const [mintError, setMintError] = useState<string | null>(null);
  const [newGrant, setNewGrant] = useState<NewGrant | null>(null);
  const [copied, setCopied] = useState(false);

  const [grants, setGrants] = useState<GrantRow[]>([]);
  const [grantsError, setGrantsError] = useState<string | null>(null);
  const [revokingId, setRevokingId] = useState<string | null>(null);

  const [previewOpen, setPreviewOpen] = useState(false);

  const refreshStatus = useCallback(() => {
    invoke('observe_sharing_status')
      .then((raw) => setStatus(normalizeSharingStatus(raw)))
      .catch((err) => setStatusError(readableError(err)));
  }, []);

  const refreshGrants = useCallback(() => {
    invoke('observe_list_grants')
      .then((rows) => setGrants(Array.isArray(rows) ? rows.map(normalizeGrantRow) : []))
      .catch((err) => setGrantsError(readableError(err)));
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    if (!open) return;
    refreshStatus();
    refreshGrants();
  }, [open, refreshStatus, refreshGrants]);

  useEffect(() => {
    const unlisten = listen('observe:grants', refreshGrants);
    return () => {
      unlisten.then((f) => f()).catch(() => {});
    };
  }, [refreshGrants]);

  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const startSharing = useCallback(async () => {
    setStatusBusy(true);
    setStatusError(null);
    try {
      await invoke('observe_start_sharing');
      refreshStatus();
    } catch (err) {
      setStatusError(readableError(err));
    } finally {
      setStatusBusy(false);
    }
  }, [refreshStatus]);

  const stopSharing = useCallback(async () => {
    setStatusBusy(true);
    setStatusError(null);
    try {
      await invoke('observe_stop_sharing');
      refreshStatus();
    } catch (err) {
      setStatusError(readableError(err));
    } finally {
      setStatusBusy(false);
    }
  }, [refreshStatus]);

  const mintGrant = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      const trimmed = label.trim();
      if (!trimmed) return;
      setMinting(true);
      setMintError(null);
      setNewGrant(null);
      setCopied(false);
      try {
        const raw = await invoke('observe_create_grant', { label: trimmed, ttlSecs });
        setNewGrant(normalizeNewGrant(raw));
        setLabel('');
        refreshGrants();
      } catch (err) {
        setMintError(readableError(err));
      } finally {
        setMinting(false);
      }
    },
    [label, ttlSecs, refreshGrants],
  );

  const revoke = useCallback(
    async (grantId: string) => {
      setRevokingId(grantId);
      setGrantsError(null);
      try {
        await invoke('observe_revoke_grant', { grantId });
        refreshGrants();
      } catch (err) {
        setGrantsError(readableError(err));
      } finally {
        setRevokingId(null);
      }
    },
    [refreshGrants],
  );

  const copyToken = useCallback(() => {
    if (!newGrant) return;
    const clipboard = navigator.clipboard;
    if (!clipboard) return;
    clipboard
      .writeText(newGrant.token)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      })
      .catch(() => {
        /* clipboard denied: the token stays selectable on screen either way */
      });
  }, [newGrant]);

  return (
    <div className="wd-observe-menu" ref={rootRef}>
      <button
        type="button"
        className="wd-observe-trigger"
        aria-haspopup="true"
        aria-expanded={open}
        aria-label="Remote observation menu"
        onClick={() => setOpen((o) => !o)}
      >
        <span aria-hidden>&#8942;</span>
      </button>

      {open ? (
        <div className="wd-observe-popover" role="menu">
          <div className="wd-observe-tabs" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'share'}
              className={`wd-observe-tab${tab === 'share' ? ' is-active' : ''}`}
              onClick={() => setTab('share')}
            >
              Share
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'access'}
              className={`wd-observe-tab${tab === 'access' ? ' is-active' : ''}`}
              onClick={() => setTab('access')}
            >
              Access
            </button>
          </div>

          {tab === 'share' ? (
            <ShareTab
              status={status}
              statusBusy={statusBusy}
              statusError={statusError}
              onStart={startSharing}
              onStop={stopSharing}
              label={label}
              onLabelChange={setLabel}
              ttlSecs={ttlSecs}
              onTtlChange={setTtlSecs}
              minting={minting}
              mintError={mintError}
              onMint={mintGrant}
              newGrant={newGrant}
              copied={copied}
              onCopy={copyToken}
              onPreview={() => setPreviewOpen(true)}
            />
          ) : (
            <AccessTab grants={grants} error={grantsError} revokingId={revokingId} onRevoke={revoke} />
          )}
        </div>
      ) : null}

      {previewOpen ? <PreviewModal onClose={() => setPreviewOpen(false)} /> : null}
    </div>
  );
}

export default ShareMenu;
