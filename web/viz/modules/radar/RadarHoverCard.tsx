// RadarHoverCard.tsx — the radar quick-glance hover card (Task 17).
//
// Mirrors the Habits hover preview (`wd-preview` / `wd-card-*` in style.css) but
// reads a live `RadarAgent` instead of an anti-pattern, and is tinted by the RADAR
// palette (Claude orange / Codex violet), not the Habits harness map. It is a pure
// presentational island: `RadarConstellation` mounts it inside a drei <Html> at the
// hovered globe's projected position, so it floats screen-space at a constant pixel
// size at any zoom. Colour is ALWAYS paired with the harness glyph + label so the
// card is legible without colour (color-blind a11y), per the locked design.
//
// Every field is a real signal — label, harness, model, fill %, child count, status
// — so the card can never show something the backend did not emit.

import type { CSSProperties } from 'react';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { radarSubtitle, formatTokens } from '@/viz/shared/types/radarTypes';
import { radarHarness } from './radarTheme';

/** Human status words (the `working|idle|closed|terminated` enum is terse; spell it for glance). */
const STATUS_LABEL: Record<RadarAgent['status'], string> = {
  working: 'Working',
  idle: 'Idle',
  closed: 'Closed',
  terminated: 'Terminated',
};

function pct(fill: number): string {
  return `${Math.round(fill * 100)}%`;
}

/** "1 child" / "N children"; nothing for a flat agent (honest-viz: no fake roster). */
function childLine(n: number): string | null {
  if (n <= 0) return null;
  return n === 1 ? '1 child' : `${n} children`;
}

export function RadarHoverCard({ agent }: { agent: RadarAgent }) {
  const theme = radarHarness(agent.harness);
  const label = agent.label || agent.nickname || agent.id;
  const subtitle = radarSubtitle(agent);
  const children = childLine(agent.childCount);
  // Show the live token occupancy, not just the percent: "128k / 200k (64%)" when the
  // model window is known, else the raw count ("128k tokens") when it isn't.
  const context =
    agent.maxTokens > 0
      ? `${formatTokens(agent.contextTokens)} / ${formatTokens(agent.maxTokens)} (${pct(agent.fillPct)})`
      : `${formatTokens(agent.contextTokens)} tokens`;

  return (
    <div
      className="wd-radar-card"
      style={{ '--harness': theme.color } as CSSProperties}
      role="status"
    >
      <div className="wd-card-kicker">
        <span className="wd-card-glyph" aria-hidden>
          {theme.glyph}
        </span>
        {theme.label}
        {agent.role ? <span className="wd-radar-card-role"> · {agent.role}</span> : null}
      </div>

      <div className="wd-card-main">{label}</div>
      {subtitle ? <div className="wd-card-sub">{subtitle}</div> : null}

      <dl className="wd-radar-card-stats">
        <div className="wd-radar-card-context">
          <dt>Context</dt>
          <dd>{context}</dd>
        </div>
        <div>
          <dt>Model</dt>
          <dd>{agent.model ?? '—'}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd className={`wd-radar-status is-${agent.status}`}>{STATUS_LABEL[agent.status]}</dd>
        </div>
        {children ? (
          <div>
            <dt>Fan-out</dt>
            <dd>{children}</dd>
          </div>
        ) : null}
      </dl>
    </div>
  );
}

export default RadarHoverCard;
