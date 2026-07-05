// DimensionsSection.tsx — §④ WHO YOU ARE. The 7 profile dimensions, but NOT as
// 7 identical hedged paragraphs: strengths vs holes are contrasted in two
// columns, each dimension's claims carry an asserted/emerging status chip +
// confidence. The reader should see, at a glance, what you're good at against
// what you're missing.
//
// Honest-viz: every claim's status ('asserted' | 'emerging') and confidence come
// straight from the backend (by-proof gating already applied server-side — a
// claim with <3 cited sessions is 'emerging', confidence ≤0.5). We render those
// states truthfully, never upgrading them.

import type { Claim, ProfileDimension } from './types';
import { EvidenceList } from './EvidenceList';

// Which side of the split each dimension key lands on. `strengths` and the
// positive-framed dimensions read as strengths; `holes` / `where_you_lose` read
// as the gaps. Anything unrecognized defaults to the "reads" (left) column so a
// schema addition never vanishes.
const HOLE_KEYS = new Set(['holes', 'where_you_lose']);

function ClaimItem({ claim }: { claim: Claim }) {
  const asserted = claim.status === 'asserted';
  return (
    <li className="claim">
      <div className="claim__line">
        <span className={`chip ${asserted ? 'chip--asserted' : 'chip--emerging'}`}>{claim.status}</span>
        <span className="claim__text">{claim.text}</span>
      </div>
      <div className="claim__foot">
        <span className="claim__conf">confidence {Math.round((Number.isFinite(claim.confidence) ? claim.confidence : 0) * 100)}%</span>
        {claim.evidence.length > 0 ? (
          <EvidenceList evidence={claim.evidence} label={`Proof (${claim.evidence.length})`} />
        ) : null}
      </div>
    </li>
  );
}

function DimCard({ dim }: { dim: ProfileDimension }) {
  return (
    <div className="dim">
      <div className="dim__title">{dim.title}</div>
      {dim.narrative ? <div className="dim__narrative">{dim.narrative}</div> : null}
      {dim.claims.length > 0 ? (
        <ul className="dim__claims">
          {dim.claims.map((c, i) => (
            <ClaimItem key={i} claim={c} />
          ))}
        </ul>
      ) : (
        <div className="dossier__empty">No claims cited yet.</div>
      )}
    </div>
  );
}

export function DimensionsSection({ dimensions }: { dimensions: ProfileDimension[] }) {
  const dims = dimensions ?? [];
  const holes = dims.filter((d) => HOLE_KEYS.has(d.key));
  const strengths = dims.filter((d) => !HOLE_KEYS.has(d.key));

  return (
    <section className="dossier__section" aria-label="Who you are">
      <div className="dossier__eyebrow">
        <span className="dossier__num">④</span>
        <h2 className="dossier__h">Who you are</h2>
        <span className="dossier__hint">strengths, contrasted with the gaps</span>
      </div>
      {dims.length === 0 ? (
        <div className="dossier__empty">Your operator profile fills in once the dossier is generated.</div>
      ) : (
        <div className="who">
          <div className="who__col who__col--strength">
            <div className="who__colhead">
              <span aria-hidden>▲</span> How you operate
            </div>
            {strengths.length > 0 ? (
              strengths.map((d) => <DimCard key={d.key} dim={d} />)
            ) : (
              <div className="dossier__empty">—</div>
            )}
          </div>
          <div className="who__col who__col--hole">
            <div className="who__colhead">
              <span aria-hidden>▽</span> Where you're exposed
            </div>
            {holes.length > 0 ? (
              holes.map((d) => <DimCard key={d.key} dim={d} />)
            ) : (
              <div className="dossier__empty">No gaps asserted for this window.</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

export default DimensionsSection;
