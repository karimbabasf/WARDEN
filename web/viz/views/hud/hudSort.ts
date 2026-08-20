// hudSort.ts: what the menu-bar HUD shows, and in what order.
//
// The HUD is a GLANCE surface: the question it answers in half a second is "does
// anything need me right now". So the order is need-first (awaiting), then live
// (working), then quiet (idle), and dead sessions are not shown at all, because a
// board full of terminated agents buries the one that is asking a question.
//
// Only ROOT agents get a globe. Subagents are real, but forty of them would drown
// the four sessions Karim actually drives; each root carries its `childCount`
// instead, which is the same thing the radar already says with its moons.
//
// Pure module: no React, no Three, no DOM. Unit-tested in hudSort.test.ts.

import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';

/** Lower sorts first. Anything not listed is filtered out before it gets here. */
const BUCKET: Record<string, number> = { awaiting: 0, working: 1, idle: 2 };

export type HudSummary = {
  total: number;
  working: number;
  awaiting: number;
};

/** The live roots, need-first. Stable: equal-status agents keep transcript order. */
export function hudAgents(model: RadarSceneModel | undefined): RadarAgent[] {
  const roots = (model?.agents ?? []).filter(
    (a) => a.depth === 0 && a.status in BUCKET,
  );
  // startedAt ascending inside a bucket, so a newly spawned agent joins at the END
  // of its group instead of shuffling the globes someone is already looking at.
  return roots
    .map((a, i) => ({ a, i }))
    .sort((x, y) => {
      const b = BUCKET[x.a.status] - BUCKET[y.a.status];
      if (b !== 0) return b;
      const t = x.a.startedAt.localeCompare(y.a.startedAt);
      return t !== 0 ? t : x.i - y.i;
    })
    .map((e) => e.a);
}

/** The header line's three numbers, counted over the same set the grid draws. */
export function hudSummary(agents: RadarAgent[]): HudSummary {
  let working = 0;
  let awaiting = 0;
  for (const a of agents) {
    if (a.status === 'working') working++;
    else if (a.status === 'awaiting') awaiting++;
  }
  return { total: agents.length, working, awaiting };
}

/** The header's own sentence. Awaiting is named first because it is the only
 *  number that means "come back to the machine". */
export function hudSummaryLabel(s: HudSummary): string {
  if (s.total === 0) return 'No agents running';
  const parts = [`${s.total} agent${s.total === 1 ? '' : 's'}`];
  if (s.awaiting > 0) parts.push(`${s.awaiting} awaiting`);
  if (s.working > 0) parts.push(`${s.working} working`);
  return parts.join(' · ');
}

/**
 * The identity under a globe.
 *
 * `label` (the session's own task line) beats `cwd`, and that order is not arbitrary:
 * agents are routinely started from the home folder, so on a real machine `cwd` reads
 * "karimbaba" for most of the board: eight identical captions under eight different
 * agents. The task is what tells them apart. `cwd` stays as the last resort for a
 * session that never named itself, and the full folder is on the cell's tooltip either
 * way.
 */
export function hudCellLabel(a: RadarAgent): string {
  return a.nickname || a.label || a.title || a.cwd || a.id.slice(0, 8);
}

/** Everything that did not fit in the cell, for the hover tooltip. */
export function hudCellTooltip(a: RadarAgent, harnessLabel: string): string {
  const place = a.repo && a.repo !== a.cwd ? `${a.repo}/${a.cwd}` : a.cwd;
  const bits = [hudCellLabel(a), harnessLabel, place, a.model, a.status];
  if (a.childCount > 0) bits.push(`${a.childCount} subagent${a.childCount === 1 ? '' : 's'}`);
  return bits.filter(Boolean).join(' · ');
}
