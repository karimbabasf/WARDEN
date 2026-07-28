// FilterBar.tsx: the harness emphasis filter, a bottom-centre dock over the canvas.
// Each chip toggles a single `EmphasisFilter`; a lit chip clears it; matching globes
// stay full-strength while the rest dim (the dim channel is wired in the
// constellation). Honest-viz plus a11y: every chip pairs colour, glyph, and a text
// label (colour is never the only signal), and chips key off the real snake_case
// harness id so `matchesFilter` lines up with the scene nodes. The dock only exists
// when there is something to choose between: on a single-harness board it renders
// nothing rather than parking one inert chip over the scope.

import { type CSSProperties } from 'react';
import { harnessTheme } from '@/viz/shared/theme/harnessTheme';
import type { EmphasisFilter } from '@/viz/shared/lib/emphasis';

function isHarnessActive(filter: EmphasisFilter, harness: string): boolean {
  return filter?.kind === 'harness' && filter.harness === harness;
}

/**
 * The chips worth showing. One harness on the board is not a filter: every globe
 * matches it, so the chip can only dim everything or nothing, and it sits over the
 * scope reading like a label. Below two harnesses the dock earns no space and the
 * bar renders nothing at all.
 */
export function filterHarnesses(agents: readonly { harness: string }[]): string[] {
  const distinct = Array.from(new Set(agents.map((a) => a.harness)));
  return distinct.length > 1 ? distinct : [];
}

export function FilterBar({
  agents,
  filter,
  onFilter,
}: {
  /** The live agents on the board; the bar shows one chip per distinct harness. */
  agents: readonly { harness: string }[];
  filter: EmphasisFilter;
  onFilter: (f: EmphasisFilter) => void;
}) {
  // Reflect the harnesses actually present, and only while there is more than one
  // of them. Never fabricates a harness, and never renders a chip that filters
  // nothing.
  const harnesses = filterHarnesses(agents);
  if (harnesses.length === 0) return null;

  return (
    <div className="wd-filterbar" role="group" aria-label="Harness filter">
      {harnesses.map((harness) => {
        const t = harnessTheme(harness);
        const active = isHarnessActive(filter, harness);
        const next: EmphasisFilter = active ? null : { kind: 'harness', harness };
        return (
          <button
            type="button"
            key={harness}
            className={`wd-chip wd-chip-harness${active ? ' is-active' : ''}`}
            aria-pressed={active}
            aria-label={`${active ? 'Clear' : 'Show only'} ${t.label} agents`}
            title={`${t.label}${active ? ' (active, click to clear)' : ''}`}
            onClick={() => onFilter(next)}
            style={{ '--chip': t.color } as CSSProperties}
          >
            <span className="wd-chip-glyph" aria-hidden="true">{t.glyph}</span>
            <span className="wd-chip-label">{t.label}</span>
          </button>
        );
      })}
    </div>
  );
}

export default FilterBar;
