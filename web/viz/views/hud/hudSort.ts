// hudSort.ts: what the menu-bar HUD shows, and in what order.
//
// The HUD is a GLANCE surface: the question it answers in half a second is "does
// anything need me right now". So the order is need-first (awaiting), then live
// (working), then quiet (idle), and dead sessions are not shown at all, because a
// board full of terminated agents buries the one that is asking a question.
//
// Only ROOT agents are PICKABLE. A root is a session you can go to; a subagent is
// work happening inside one, with no window of its own to raise, so it is shown and
// not offered. Each root carries its live descendants as a small strip of moons
// under it (see `hudTree`), which is the same thing the radar says with its orbits.
//
// Pure module: no React, no Three, no DOM. Unit-tested in hudSort.test.ts.

import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';

/** Lower sorts first. Anything not listed is filtered out before it gets here. */
const BUCKET: Record<string, number> = { awaiting: 0, working: 1, idle: 2 };

/** Moons drawn under one root before the rest stop being drawn. Two lines of four
 *  is the most an 88px cell can carry and still read as one session's work; the cell's
 *  own count line keeps stating the true total, so nothing is silently lost. */
export const HUD_MAX_KIDS = 8;

/** Guard for a malformed forest: a parent chain that loops must not hang the HUD. */
const MAX_ANCESTRY = 16;

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

/** Need-first, then oldest-first inside a bucket. The one comparator both roots and
 *  their moons are ordered by, so a strip never sorts differently from the grid. */
function byNeedThenAge(a: RadarAgent, b: RadarAgent): number {
  const bucket = BUCKET[a.status] - BUCKET[b.status];
  if (bucket !== 0) return bucket;
  return a.startedAt.localeCompare(b.startedAt);
}

/** One root, plus the live work running inside it. */
export type HudNode = {
  agent: RadarAgent;
  /** Live descendants at ANY depth, need-first, capped at HUD_MAX_KIDS. */
  kids: RadarAgent[];
  /** Live descendants the cap dropped. Surfaced as a "+N", never silently lost. */
  hiddenKids: number;
};

/**
 * The board: every live root, with its live subagents hanging under it.
 *
 * Descendants are flattened onto their ROOT rather than kept as a tree. A subagent
 * that spawned a subagent is still work happening inside one session, and the HUD's
 * question is "what is this session doing", not "what is the shape of its swarm".
 * The war room is where the shape lives.
 *
 * A node whose parent chain does not reach a live root is dropped, not re-parented:
 * an orphan moon under the wrong session is worse than one that is not drawn.
 */
export function hudTree(model: RadarSceneModel | undefined): HudNode[] {
  const all = model?.agents ?? [];
  const roots = hudAgents(model);
  const nodes: HudNode[] = roots.map((agent) => ({ agent, kids: [], hiddenKids: 0 }));
  const slotByRoot = new Map(nodes.map((n) => [n.agent.id, n]));
  const byId = new Map(all.map((a) => [a.id, a]));

  for (const a of all) {
    if (a.depth === 0 || !(a.status in BUCKET)) continue;
    // Walk UP to the session this work belongs to. `depth` is not enough on its own:
    // a depth-2 agent's parent is another subagent, and only the chain names the root.
    let cursor: RadarAgent | undefined = a;
    let hops = 0;
    while (cursor && cursor.depth !== 0 && hops++ < MAX_ANCESTRY) {
      cursor = cursor.parentId ? byId.get(cursor.parentId) : undefined;
    }
    const slot = cursor && cursor.depth === 0 ? slotByRoot.get(cursor.id) : undefined;
    if (slot) slot.kids.push(a);
  }

  for (const n of nodes) {
    n.kids.sort(byNeedThenAge);
    if (n.kids.length > HUD_MAX_KIDS) {
      n.hiddenKids = n.kids.length - HUD_MAX_KIDS;
      n.kids.length = HUD_MAX_KIDS;
    }
  }
  return nodes;
}

/** Every live subagent under this root, drawn or not. The cell's own count line reads
 *  THIS and not `childCount`, so the number can never disagree with the moons beside
 *  it (`childCount` counts direct children, dead ones included). */
export function hudKidCount(n: HudNode): number {
  return n.kids.length + n.hiddenKids;
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

/** Everything that did not fit in the cell, for the hover tooltip. `kids` is the LIVE
 *  count (see `hudKidCount`), so the tooltip and the moons under the cell agree. */
export function hudCellTooltip(a: RadarAgent, harnessLabel: string, kids = 0): string {
  const place = a.repo && a.repo !== a.cwd ? `${a.repo}/${a.cwd}` : a.cwd;
  const bits = [hudCellLabel(a), harnessLabel, place, a.model, a.status];
  if (kids > 0) bits.push(`${kids} subagent${kids === 1 ? '' : 's'}`);
  return bits.filter(Boolean).join(' · ');
}
