// FleetRail.tsx: the left rail, every live agent as a progress STRIP.
//
// The metaphor is an air traffic control strip rack. One slip per agent, always
// in the same column order, so the eye learns where a number lives and reads it
// by position rather than by hunting. That is the whole answer to "so much of
// the info looks compressed": the density is the same, the ZONING is not.
//
// Every field maps to a real backend signal (honest-viz). A strip renders what
// the radar actually knows and leaves a slot visibly empty otherwise; it never
// substitutes a placeholder that reads like data.

import { useMemo, type CSSProperties, type ReactNode } from 'react';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { formatTokens, shortModel } from '@/viz/shared/types/radarTypes';
import { radarHarness } from '@/viz/modules/radar/radarTheme';

/**
 * Per-kind glyph. Colour is never the only signal (a11y).
 *
 * Every one of these is a TEXT-presentation codepoint (Geometric Shapes or
 * Miscellaneous Technical). Gear, pencil and six-pointed-star were emoji-
 * presentation-capable, so they rendered as full-colour emoji on some platforms
 * and broke an otherwise monochrome instrument panel.
 */
const KIND_GLYPH: Record<string, string> = {
  read: '▤',
  write: '◆',
  search: '⌕',
  run: '»',
  tool: '◈',
  message: '≡',
  thinking: '◌',
};

const STATUS_WORD: Record<RadarAgent['status'], string> = {
  working: 'Working',
  idle: 'Idle',
  closed: 'Closed',
  terminated: 'Done',
};

/**
 * The strip's headline. Precedence is deliberate and matches what a human would
 * call the session: the harness's own auto-generated title first (that is the
 * "build frontier website" name), then the radar's derived label, then the
 * nickname, then the id. Never an empty string.
 */
export function stripName(agent: RadarAgent): string {
  return agent.title || agent.label || agent.nickname || agent.id;
}

/**
 * The secondary line: folder, model, and the surface it runs on. Each part is
 * dropped when unknown rather than rendered as a dash, so the line stays short
 * on a sparse agent instead of becoming a row of placeholders.
 */
export function stripMeta(agent: RadarAgent): string {
  const parts: string[] = [];
  if (agent.cwd && agent.cwd !== stripName(agent)) parts.push(agent.cwd);
  const model = shortModel(agent.model);
  if (model) parts.push(model);
  if (agent.surface && agent.surface !== 'cli') parts.push(agent.surface);
  return parts.join(' · ');
}

/** Compact elapsed for the strip's in-flight clock: 42s, 3m, 1h04. */
export function shortElapsed(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  return `${hr}h${String(min % 60).padStart(2, '0')}`;
}

function costLabel(usd: number | null): string | null {
  if (usd == null || !Number.isFinite(usd)) return null;
  return usd < 0.01 ? '<$0.01' : `$${usd.toFixed(2)}`;
}

function Strip({
  agent,
  selected,
  childCount,
  onSelect,
}: {
  agent: RadarAgent;
  selected: boolean;
  childCount: number;
  onSelect: (id: string) => void;
}) {
  const theme = radarHarness(agent.harness);
  const action = agent.currentAction ?? null;
  const fill = Math.max(0, Math.min(1, agent.fillPct));
  const cost = costLabel(agent.estCostUsd);
  // Over 85% the gauge is the alert, not the harness hue: a context window
  // about to overflow is the one thing worth breaking colour discipline for.
  const hot = fill >= 0.85;

  return (
    <li className="wd-strip-item">
      <button
        type="button"
        className={`wd-strip${selected ? ' is-selected' : ''}${hot ? ' is-hot' : ''}`}
        style={{ '--harness': theme.color } as CSSProperties}
        data-strip={agent.id}
        data-status={agent.status}
        aria-pressed={selected}
        onClick={() => onSelect(agent.id)}
      >
        <span className="wd-strip-rule" aria-hidden />

        <span className="wd-strip-head">
          <span className="wd-strip-glyph" aria-hidden>
            {theme.glyph}
          </span>
          <span className="wd-strip-name">{stripName(agent)}</span>
          <span className={`wd-strip-status is-${agent.status}`}>{STATUS_WORD[agent.status]}</span>
        </span>

        {stripMeta(agent) ? <span className="wd-strip-meta">{stripMeta(agent)}</span> : null}

        {action ? (
          <span className="wd-strip-action" data-kind={action.kind}>
            <span className="wd-strip-action-glyph" aria-hidden>
              {KIND_GLYPH[action.kind] ?? '•'}
            </span>
            <span className="wd-strip-action-label">{action.label || action.tool}</span>
            <span className="wd-strip-action-clock">{shortElapsed(action.elapsedMs)}</span>
          </span>
        ) : (
          <span className="wd-strip-action is-idle">
            <span className="wd-strip-action-glyph" aria-hidden>
              ·
            </span>
            <span className="wd-strip-action-label">No action in flight</span>
          </span>
        )}

        <span className="wd-strip-foot">
          <span className="wd-strip-gauge" aria-hidden>
            <span className="wd-strip-gauge-fill" style={{ width: `${Math.round(fill * 100)}%` }} />
          </span>
          <span className="wd-strip-fill-pct">{Math.round(fill * 100)}%</span>
          <span className="wd-strip-tokens">{formatTokens(agent.contextTokens)}</span>
          {cost ? <span className="wd-strip-cost">{cost}</span> : null}
          {childCount > 0 ? (
            <span className="wd-strip-kids" title={`${childCount} subagents`}>
              +{childCount}
            </span>
          ) : null}
        </span>
      </button>
    </li>
  );
}

/**
 * The rack's one-line census, shared by the open head and the folded tab so the
 * two can never disagree about how many sessions are live.
 */
export function fleetSummary(sessions: number, working: number): string {
  const head = `${sessions} session${sessions === 1 ? '' : 's'}`;
  return working > 0 ? `${head} · ${working} working` : head;
}

export type FleetRailProps = {
  agents: RadarAgent[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** Folded: the rack is off the board, down to a tab that brings it back. */
  collapsed: boolean;
  onToggleCollapsed: () => void;
  /** Rendered under the fleet: the observer dock (watch someone else's swarm). */
  footer?: ReactNode;
};

/**
 * Roots only. Subagents are reachable from their parent's roster and from the
 * constellation itself; listing them flat here would double-count the fleet and
 * make the rack unreadable at 20 agents.
 *
 * Order is STABLE by start time, not by status. A rack that re-sorts itself
 * every time an agent flips working/idle is unusable: you lose the row you were
 * reading. Position is the index the eye trusts.
 */
export function FleetRail({
  agents,
  selectedId,
  onSelect,
  collapsed,
  onToggleCollapsed,
  footer,
}: FleetRailProps) {
  const roots = useMemo(
    () =>
      agents
        .filter((a) => a.parentId === null)
        .slice()
        .sort((a, b) => {
          const at = Date.parse(a.startedAt);
          const bt = Date.parse(b.startedAt);
          const av = Number.isFinite(at) ? at : Number.POSITIVE_INFINITY;
          const bv = Number.isFinite(bt) ? bt : Number.POSITIVE_INFINITY;
          return av - bv || a.id.localeCompare(b.id);
        }),
    [agents],
  );

  const childCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const a of agents) {
      if (a.parentId) m.set(a.parentId, (m.get(a.parentId) ?? 0) + 1);
    }
    return m;
  }, [agents]);

  const working = roots.filter((a) => a.status === 'working').length;
  const summary = fleetSummary(roots.length, working);

  // Folded, the rack is a tab and nothing else: a fixed-width pill in the exact
  // slot the head used to occupy, so unfolding does not make anything jump. The
  // census drops to the raw session count and moves into the label, because the
  // tab's whole job is to hand the board back its left third.
  //
  // The body is HIDDEN, never unmounted. The observer dock lives in the footer and
  // holds the live peer watch (its selection, its `observe:frame` listener) in its
  // own state, so unmounting it on a fold would silently freeze a peer's board on
  // its last frame. Folding is chrome, not a teardown.
  return (
    <aside className={`wd-fleet${collapsed ? ' is-folded' : ''}`} aria-label="Fleet">
      {collapsed ? (
        <button
          type="button"
          className="wd-fleet-tab"
          aria-expanded={false}
          aria-label={`Show the fleet: ${summary}`}
          title={`${summary} (F)`}
          onClick={onToggleCollapsed}
        >
          <span className="wd-card-kicker">Fleet</span>
          <span className="wd-fleet-count">{roots.length}</span>
          <span className="wd-fleet-fold-glyph" aria-hidden>
            »
          </span>
        </button>
      ) : null}

      <div className="wd-fleet-body">
        <div className="wd-fleet-head">
          <span className="wd-card-kicker">Fleet</span>
          <span className="wd-fleet-count">{summary}</span>
          <button
            type="button"
            className="wd-fleet-fold"
            aria-expanded
            aria-label="Hide the fleet"
            title="Hide the fleet (F)"
            onClick={onToggleCollapsed}
          >
            <span className="wd-fleet-fold-glyph" aria-hidden>
              «
            </span>
          </button>
        </div>

        {roots.length === 0 ? (
          <p className="wd-fleet-empty">
            No sessions yet. Open Claude Code or Codex and they appear here.
          </p>
        ) : (
          <ul className="wd-fleet-list">
            {roots.map((a) => (
              <Strip
                key={a.id}
                agent={a}
                selected={a.id === selectedId}
                childCount={childCounts.get(a.id) ?? 0}
                onSelect={onSelect}
              />
            ))}
          </ul>
        )}

        {footer ? <div className="wd-fleet-footer">{footer}</div> : null}
      </div>
    </aside>
  );
}

export default FleetRail;
