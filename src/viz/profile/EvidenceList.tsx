// EvidenceList.tsx — evidence promoted to first-class citizens.
//
// This is "by Proof" made visible: every claim and every leak rests on cited
// sessions, and the citation is a prominent, clickable card — not an 11px
// toggle. Clicking a citation resolves it in place via the existing
// `resolve_evidence` command (fallback path), revealing the fuller quote/context
// the backend returns. All EvidenceRef fields may be null; every access guarded.

import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { EvidenceRef } from './types';

/** Last two path segments of source_path, else session id, else a calm default. */
function sourceTail(ev: EvidenceRef): string {
  const where = ev.source_path ?? ev.session_id ?? null;
  if (!where) return 'session';
  const segs = where.split('/').filter(Boolean);
  return segs.length > 0 ? segs.slice(-2).join('/') : where;
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** The `resolve_evidence` return shape (Rust `ResolvedEvidence`). */
interface ResolvedEvidence {
  quote: string | null;
  source_path: string | null;
}

function OneCitation({ ev, idx }: { ev: EvidenceRef; idx: number }) {
  const [resolved, setResolved] = useState<string | null>(null);
  const [state, setState] = useState<'idle' | 'loading' | 'done' | 'unavailable'>('idle');

  const quote = ev.quote ? `“${truncate(ev.quote, 220)}”` : '(no captured quote — open to resolve)';
  // The backend fallback can only recover text when the ref carries an event_id.
  const resolvable = !!ev.event_id && !!ev.session_id;

  const onOpen = useCallback(async () => {
    if (state === 'loading') return;
    // Toggle closed if already open.
    if (state === 'done' || state === 'unavailable') {
      setState('idle');
      setResolved(null);
      return;
    }
    if (!resolvable) {
      setState('unavailable');
      return;
    }
    setState('loading');
    try {
      const res = await invoke<ResolvedEvidence>('resolve_evidence', {
        sessionId: ev.session_id,
        eventId: ev.event_id,
      });
      const text = res?.quote?.trim() || null;
      if (text) {
        setResolved(text);
        setState('done');
      } else {
        setState('unavailable');
      }
    } catch {
      // Off the Tauri runtime (browser preview) or the row is gone — the card
      // still shows its captured quote; surface a calm note.
      setState('unavailable');
    }
  }, [ev, state, resolvable]);

  const label =
    state === 'loading'
      ? 'resolving…'
      : state === 'done' || state === 'unavailable'
        ? 'close'
        : resolvable
          ? 'open →'
          : '';

  return (
    <li>
      <button
        type="button"
        className="evcite"
        onClick={onOpen}
        aria-expanded={state === 'done' || state === 'unavailable'}
      >
        <div className="evcite__quote">{quote}</div>
        <div className="evcite__source">
          <span className="path" title={ev.source_path ?? ev.session_id ?? undefined}>
            {sourceTail(ev)}
          </span>
          {ev.turn_id ? <span>· turn {ev.turn_id}</span> : null}
          <span className="go" aria-hidden>
            {label}
          </span>
          <span className="sr-only">evidence {idx + 1}</span>
        </div>
        {state === 'done' && resolved ? <div className="evcite__resolved">{resolved}</div> : null}
        {state === 'unavailable' ? (
          <div className="evcite__resolved">No richer context available — the captured quote above is the full citation.</div>
        ) : null}
      </button>
    </li>
  );
}

/** A titled, clickable list of citations. Empty → a quiet honest note. */
export function EvidenceList({ evidence, label = 'Evidence' }: { evidence: EvidenceRef[]; label?: string }) {
  if (!evidence || evidence.length === 0) {
    return (
      <div className="evidence">
        <div className="evidence__head">
          <span>{label}</span>
          <span className="count">— none cited</span>
        </div>
      </div>
    );
  }
  return (
    <div className="evidence">
      <div className="evidence__head">
        <span>◈ {label}</span>
        <span className="count">
          {evidence.length} cited session{evidence.length === 1 ? '' : 's'}
        </span>
      </div>
      <ul className="evidence__list">
        {evidence.map((ev, i) => (
          <OneCitation key={`${ev.session_id}-${ev.turn_id ?? ''}-${i}`} ev={ev} idx={i} />
        ))}
      </ul>
    </div>
  );
}

export default EvidenceList;
