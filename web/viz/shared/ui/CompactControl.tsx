// CompactControl.tsx: the ARM switch on an agent's context meter.
//
// "When this agent finishes what it is doing, compact it." The arming is entirely
// WARDEN-side state: the click writes a local record and NOTHING is sent to the
// agent until it actually goes idle. That is what makes cancel free, instant, and
// total, and it is why the cancel affordance is always present while armed rather
// than being a best-effort recall of something already in flight.
//
// Honesty rule, and the reason this is not just a button: for some harnesses there
// is no control channel at all (Cursor has none; a Claude Code session inside an
// IDE panel has none; Automation permission can be denied). In those cases the
// control does NOT disappear and does NOT pretend. It degrades to NOTIFY, says so
// on its face, and still does the genuinely useful half: telling you the moment
// the agent went idle at N% context. `acts` from the backend is what decides which
// of the two it is, so the UI can never claim a delivery the backend cannot make.

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type ArmedRow = {
  agentId: string;
  mode: string; // terminal_applescript | codex_app_server | notify_only
  acts: boolean;
  modeReason: string;
  state: string; // armed | firing | delivered | notified | failed | expired
  lastStatus: string; // busy | idle | shell | unknown | gone
  alive: boolean | null;
  detail: string | null;
};

export type CompactStatus = {
  automation: string;
  automationRecoverableInSettings: boolean;
  armed: ArmedRow[];
};

function normalizeRow(v: any): ArmedRow {
  return {
    agentId: typeof v?.agentId === 'string' ? v.agentId : '',
    mode: typeof v?.mode === 'string' ? v.mode : 'notify_only',
    acts: v?.acts === true,
    modeReason: typeof v?.modeReason === 'string' ? v.modeReason : '',
    state: typeof v?.state === 'string' ? v.state : 'armed',
    lastStatus: typeof v?.lastStatus === 'string' ? v.lastStatus : 'unknown',
    alive: typeof v?.alive === 'boolean' ? v.alive : null,
    detail: typeof v?.detail === 'string' && v.detail.length > 0 ? v.detail : null,
  };
}

export function normalizeCompactStatus(v: any): CompactStatus {
  return {
    automation: typeof v?.automation === 'string' ? v.automation : 'unknown',
    automationRecoverableInSettings: v?.automationRecoverableInSettings === true,
    armed: Array.isArray(v?.armed) ? v.armed.map(normalizeRow) : [],
  };
}

/** What the armed row is currently doing, in words a human reads once. */
export function armedSummary(row: ArmedRow): string {
  switch (row.state) {
    case 'firing':
      return 'Session went idle, compacting now';
    case 'delivered':
      return 'Compacted';
    case 'notified':
      return 'Session went idle, you were notified';
    case 'failed':
      return row.detail ?? 'Could not deliver';
    case 'expired':
      return 'Session ended before it went idle';
    default:
      break;
  }
  if (row.alive === false) return 'Waiting: that session is gone';
  if (row.lastStatus === 'shell') return 'Waiting: running a long command';
  if (row.lastStatus === 'idle') return 'Waiting: confirming it stays idle';
  return row.acts ? 'Armed, waiting for it to finish' : 'Watching, will notify you when it finishes';
}

/** A terminal state is done with: the row stops offering cancel. */
function isFinished(row: ArmedRow): boolean {
  return row.state === 'delivered' || row.state === 'notified' || row.state === 'expired';
}

/**
 * Polls nothing. The backend emits `compact_status` whenever the armed set or a
 * session's status changes, and that push is the only refresh trigger besides the
 * initial read, so an idle machine does no work.
 */
export function CompactControl({ agentId }: { agentId: string }) {
  const [status, setStatus] = useState<CompactStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke('compact_status')
      .then((raw) => setStatus(normalizeCompactStatus(raw)))
      .catch(() => {
        /* no backend (browser sandbox): the control simply does not render */
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const un = listen('compact_status', refresh);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [refresh]);

  const row = status?.armed.find((r) => r.agentId === agentId) ?? null;

  const act = useCallback(
    (cmd: 'compact_arm' | 'compact_cancel') => {
      setBusy(true);
      setError(null);
      invoke(cmd, { agentId })
        .then(() => refresh())
        .catch((err: unknown) => setError(typeof err === 'string' ? err : 'that did not work'))
        .finally(() => setBusy(false));
    },
    [agentId, refresh],
  );

  if (!status) return null;

  const deniedRecoverable = status.automation === 'denied' && status.automationRecoverableInSettings;

  if (!row || isFinished(row)) {
    return (
      <div className="wd-compact" data-compact="idle">
        <button
          type="button"
          className="wd-compact-arm"
          disabled={busy}
          onClick={() => act('compact_arm')}
        >
          <span className="wd-compact-arm-glyph" aria-hidden>
            ◎
          </span>
          Compact when idle
        </button>
        {row && isFinished(row) ? <span className="wd-compact-note">{armedSummary(row)}</span> : null}
        {error ? (
          <span className="wd-compact-error" role="alert">
            {error}
          </span>
        ) : null}
      </div>
    );
  }

  return (
    <div className={`wd-compact is-armed${row.acts ? '' : ' is-notify'}`} data-compact={row.state}>
      <div className="wd-compact-head">
        <span className="wd-compact-badge">{row.acts ? 'Armed' : 'Notify only'}</span>
        <span className="wd-compact-note">{armedSummary(row)}</span>
      </div>

      {!row.acts && row.modeReason ? (
        <p className="wd-compact-why">{row.modeReason}</p>
      ) : null}

      <div className="wd-compact-actions">
        <button
          type="button"
          className="wd-compact-cancel"
          disabled={busy}
          onClick={() => act('compact_cancel')}
        >
          Cancel
        </button>
        {deniedRecoverable ? (
          <button
            type="button"
            className="wd-mini-btn"
            onClick={() => {
              invoke('compact_open_automation_settings').catch(() => {});
            }}
          >
            Allow in System Settings
          </button>
        ) : null}
      </div>

      {error ? (
        <span className="wd-compact-error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}

export default CompactControl;
