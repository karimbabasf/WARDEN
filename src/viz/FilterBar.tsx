// FilterBar.tsx — the interactive harness emphasis filter, in its own bottom-centre
// dock. Each chip toggles a single `EmphasisFilter`; a lit chip clears it; matching
// orbs pop while siblings dim (the dim channel is wired in WarRoom). Honest-viz +
// a11y: every chip pairs colour + glyph + text label (colour is never the only
// signal), and harness chips key off the real snake_case harness id so `matchesFilter`
// lines up with the live fleet nodes. Severity buckets were a Habits-only signal and
// went with the Habits scene — RADAR filters by harness alone.

import { type CSSProperties } from 'react';
import { harnessTheme } from './harnessTheme';
import type { OrbSceneModel } from './orbTypes';
import type { EmphasisFilter } from './emphasis';

function isHarnessActive(filter: EmphasisFilter, harness: string): boolean {
  return filter?.kind === 'harness' && filter.harness === harness;
}

export function FilterBar({
  model,
  filter,
  onFilter,
}: {
  model: OrbSceneModel;
  filter: EmphasisFilter;
  onFilter: (f: EmphasisFilter) => void;
}) {
  // Harness chips reflect the agents actually present; fall back to a quiet Unknown
  // chip so the bar is never empty (and never fabricates a harness).
  const agents = model.agents.length
    ? model.agents
    : [{ id: 'unknown', harness: 'unknown', label: 'Unknown', glyph: '●', color: '#76ff9d', sessions: 0, eventCount: 0, totalLoad: 0 }];
  const harnesses = Array.from(new Map(agents.map((a) => [a.harness, a])).values());

  return (
    <div className="wd-filterbar" role="group" aria-label="Emphasis filter">
      <span className="wd-legend-key wd-filterbar-lead">filter</span>
      <div className="wd-legend-group" aria-label="harness">
        <span className="wd-legend-key">harness</span>
        {harnesses.map((a) => {
          const t = harnessTheme(a.harness);
          const active = isHarnessActive(filter, a.harness);
          const next: EmphasisFilter = active ? null : { kind: 'harness', harness: a.harness };
          return (
            <button
              type="button"
              key={a.harness}
              className={`wd-chip wd-chip-harness${active ? ' is-active' : ''}`}
              aria-pressed={active}
              aria-label={`${active ? 'Clear' : 'Show only'} ${t.label} agents`}
              title={`${t.label}${active ? ' (active — click to clear)' : ''}`}
              onClick={() => onFilter(next)}
              style={{ '--chip': t.color } as CSSProperties}
            >
              <span className="wd-chip-swatch" aria-hidden="true" />
              <span className="wd-chip-glyph" aria-hidden="true">{t.glyph}</span>
              <span className="wd-chip-label">{t.label}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export default FilterBar;
