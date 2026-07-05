// RangeSection.tsx — §⑤ YOUR RANGE. The breadth of what you drive agents at:
// project archetype cards + the activity heatmap (real token-contribution grid).
//
// Honest-viz: archetype counts and heatmap intensities are the backend's real
// values (heatmap.ts buckets a day's tokens into a log-scaled ramp; level 0 is a
// genuinely blank day). Empty windows say so plainly.

import { useMemo, useState } from 'react';
import type { ActivityCell, ProjectArchetype } from './types';
import { HEATMAP_LEVELS, heatmapFill, heatmapLevel, maxTokens } from './heatmap';

function ArchetypeCard({ a }: { a: ProjectArchetype }) {
  return (
    <div className="arch">
      <div className="arch__name">{a.archetype}</div>
      <div className="arch__count">
        {a.session_count} session{a.session_count === 1 ? '' : 's'}
      </div>
      {a.note ? <div className="arch__note">{a.note}</div> : null}
      {a.projects.length > 0 ? (
        <div className="arch__projects">
          {a.projects.map((p) => (
            <span key={p} className="arch__proj" title={p}>
              {p.split('/').pop() || p}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function Heatmap({ cells }: { cells: ActivityCell[] }) {
  const [hover, setHover] = useState<{ cell: ActivityCell; x: number; y: number } | null>(null);
  const max = useMemo(() => maxTokens(cells), [cells]);

  if (cells.length === 0) {
    return <div className="dossier__empty">No activity in this window.</div>;
  }

  // GitHub-style: columns of 7 (a week each), oldest → newest left to right.
  const sorted = [...cells].sort((a, b) => a.date.localeCompare(b.date));

  return (
    <div>
      <div className="heat__title">Activity — token contribution per day</div>
      <div className="heat__scroll">
        <div className="heat__grid">
          {sorted.map((c) => {
            const level = heatmapLevel(c.total_tokens, max);
            return (
              <div
                key={c.date}
                className="heat__cell"
                style={{ background: heatmapFill(level) }}
                onMouseEnter={(e) => setHover({ cell: c, x: e.clientX, y: e.clientY })}
                onMouseMove={(e) => setHover({ cell: c, x: e.clientX, y: e.clientY })}
                onMouseLeave={() => setHover(null)}
              />
            );
          })}
        </div>
        <div className="heat__legend">
          <span>less</span>
          {Array.from({ length: HEATMAP_LEVELS }, (_, i) => (
            <span key={i} className="sw" style={{ background: heatmapFill(i) }} />
          ))}
          <span>more</span>
        </div>
      </div>

      {hover ? (
        <div
          className="heat__tip"
          style={{
            left: Math.min(hover.x + 14, (typeof window !== 'undefined' ? window.innerWidth : 1024) - 250),
            top: hover.y + 14,
          }}
        >
          <div className="d">{hover.cell.date}</div>
          <div className="t">
            {hover.cell.total_tokens.toLocaleString()} tokens · {hover.cell.session_count} sessions
          </div>
          {hover.cell.by_harness.length > 0
            ? hover.cell.by_harness.map((h) => (
                <div key={h.harness} className="row">
                  <span>{h.harness}</span>
                  <span>
                    {h.tokens.toLocaleString()} · {h.sessions}s
                  </span>
                </div>
              ))
            : null}
        </div>
      ) : null}
    </div>
  );
}

export function RangeSection({
  archetypes,
  cells,
}: {
  archetypes: ProjectArchetype[];
  cells: ActivityCell[];
}) {
  const arch = archetypes ?? [];
  return (
    <section className="dossier__section" aria-label="Your range">
      <div className="dossier__eyebrow">
        <span className="dossier__num">⑤</span>
        <h2 className="dossier__h">Your range</h2>
        <span className="dossier__hint">what you build · when you work</span>
      </div>
      {arch.length === 0 ? (
        <div className="dossier__empty" style={{ marginBottom: 24 }}>
          No project archetypes clustered yet for this window.
        </div>
      ) : (
        <div className="range__cards">
          {arch.map((a) => (
            <ArchetypeCard key={a.archetype} a={a} />
          ))}
        </div>
      )}
      <Heatmap cells={cells} />
    </section>
  );
}

export default RangeSection;
