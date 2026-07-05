// LeaksSection.tsx — §③ WHERE YOU LOSE. The top-ranked workflow leaks, with
// real token/minute cost and — the point of "by Proof" — evidence promoted to
// first-class: each leak's citations render as a prominent, clickable list
// (EvidenceList), not a tiny toggle.
//
// Honest-viz: rank, cost, and evidence are the backend's real values. Empty →
// a quiet, truthful note.

import type { Leak } from './types';
import { EvidenceList } from './EvidenceList';

function LeakCard({ leak }: { leak: Leak }) {
  return (
    <div className="leak">
      <div className="leak__rank" aria-label={`rank ${leak.rank}`}>
        {leak.rank}
      </div>
      <div className="leak__body">
        <div className="leak__title">{leak.title}</div>
        <div className="leak__cost">
          <span>
            <b>~{leak.est_cost_tokens.toLocaleString()}</b> tokens
          </span>
          <span className="dot">·</span>
          <span>
            <b>~{Math.round(leak.est_cost_minutes)}</b> min
          </span>
        </div>
      </div>
      <EvidenceList evidence={leak.evidence} label="Where it shows" />
    </div>
  );
}

export function LeaksSection({ leaks }: { leaks: Leak[] }) {
  const ordered = [...(leaks ?? [])].sort((a, b) => a.rank - b.rank).slice(0, 5);
  return (
    <section className="dossier__section" aria-label="Where you lose">
      <div className="dossier__eyebrow">
        <span className="dossier__num">③</span>
        <h2 className="dossier__h">Where you lose</h2>
        <span className="dossier__hint">ranked by cost · cited</span>
      </div>
      {ordered.length === 0 ? (
        <div className="dossier__empty">No leaks surfaced for this window — your runs are staying tight.</div>
      ) : (
        <div className="leaks">
          {ordered.map((l) => (
            <LeakCard key={`${l.rank}-${l.title}`} leak={l} />
          ))}
        </div>
      )}
    </section>
  );
}

export default LeaksSection;
