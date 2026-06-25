// FilterBar.tsx — the interactive severity + harness emphasis filter, lifted out
// of chrome.tsx into its own bottom-centre dock (it replaces the removed StatusDeck
// as the only thing along the bottom). Behaviour is unchanged from the old Legend:
// each chip toggles a single `EmphasisFilter`; a lit chip clears it; matching orbs
// pop while siblings dim (the dim channel is wired in WarRoom). Honest-viz + a11y:
// every chip pairs colour + glyph + text label (colour is never the only signal),
// and harness chips key off the real snake_case harness id so `matchesFilter` lines
// up with the scene nodes. Severity is a per-habit signal → severity chips show on
// the Habits tab only; harness chips show on both.

import { type CSSProperties } from 'react';
import { harnessTheme, severityColor } from './harnessTheme';
import type { OrbSceneModel } from './orbTypes';
import type { EmphasisFilter } from './emphasis';
import type { ConstellationTab } from './NavBar';

type SevBucket = Extract<EmphasisFilter, { kind: 'severity' }>;
const SEVERITY_CHIPS: ReadonlyArray<{ bucket: SevBucket['bucket']; label: string; sev: number; glyph: string }> = [
  { bucket: 'low', label: 'Low', sev: 2, glyph: '○' },
  { bucket: 'med', label: 'Watch', sev: 3, glyph: '◔' },
  { bucket: 'high', label: 'High', sev: 4, glyph: '◑' },
  { bucket: 'crit', label: 'Critical', sev: 5, glyph: '●' },
];

function isSeverityActive(filter: EmphasisFilter, bucket: SevBucket['bucket']): boolean {
  return filter?.kind === 'severity' && filter.bucket === bucket;
}
function isHarnessActive(filter: EmphasisFilter, harness: string): boolean {
  return filter?.kind === 'harness' && filter.harness === harness;
}

export function FilterBar({
  tab,
  model,
  filter,
  onFilter,
}: {
  tab: ConstellationTab;
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
      {tab === 'habits' && (
        <div className="wd-legend-group" aria-label="severity">
          <span className="wd-legend-key">severity</span>
          {SEVERITY_CHIPS.map((c) => {
            const active = isSeverityActive(filter, c.bucket);
            const next: EmphasisFilter = active ? null : { kind: 'severity', bucket: c.bucket };
            return (
              <button
                type="button"
                key={c.bucket}
                className={`wd-chip wd-chip-sev${active ? ' is-active' : ''}`}
                aria-pressed={active}
                aria-label={`${active ? 'Clear' : 'Show only'} ${c.label} severity habits`}
                title={`${c.label} severity${active ? ' (active — click to clear)' : ''}`}
                onClick={() => onFilter(next)}
                style={{ '--chip': severityColor(c.sev) } as CSSProperties}
              >
                <span className="wd-chip-swatch" aria-hidden="true" />
                <span className="wd-chip-glyph" aria-hidden="true">{c.glyph}</span>
                <span className="wd-chip-label">{c.label}</span>
              </button>
            );
          })}
        </div>
      )}
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
