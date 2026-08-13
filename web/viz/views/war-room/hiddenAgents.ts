// hiddenAgents.ts: the board's mute list, and the rule for what a mute takes with it.
//
// Hiding is a WORKSPACE PREFERENCE, not a fact about the agent. It never reaches the
// backend, never touches the transcript, and never changes what the radar computes: it
// filters one level above the model, so the strip and the globe disappear together and
// everything downstream (layout, camera bounds, census, harness chips) agrees without
// being told about it separately.
//
// Two decisions in here are load-bearing.
//
// A hide takes the SUBTREE with it. Hiding a root and leaving its subagents on the board
// would strand globes whose parent edge points at nothing, and the constellation reads an
// unparented globe as a root, so the board would grow a session that does not exist.
//
// The stored record carries the NAME as well as the id. A hidden session keeps running,
// finishes, and ages out of the feed, and at that moment a list of bare ids has nothing
// to render: the row you need in order to un-hide it is the row that just went blank.
// Remembering the name at hide time is what keeps the list readable and reversible.

import type { RadarAgent } from '@/viz/shared/types/radarTypes';

/** One muted agent: the id the board filters on, plus the name to show in the list. */
export type HiddenAgent = { id: string; name: string };

const HIDDEN_KEY = 'warden.fleet.hidden';

/**
 * Read the mute list. Guarded on both sides like the fleet fold: `localStorage` throws
 * outright in a locked-down webview, and a mute that cannot be remembered must still be
 * a mute that works for this session.
 *
 * Every row is validated on the way in. The value is user-writable (devtools, a stale
 * build, a half-written record), and one malformed entry must not take the board with it.
 */
export function readHiddenAgents(): HiddenAgent[] {
  try {
    const raw = window.localStorage.getItem(HIDDEN_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((v): v is HiddenAgent =>
        typeof (v as HiddenAgent)?.id === 'string' &&
        (v as HiddenAgent).id.length > 0 &&
        typeof (v as HiddenAgent)?.name === 'string',
      )
      .map((v) => ({ id: v.id, name: v.name }));
  } catch {
    return [];
  }
}

export function writeHiddenAgents(rows: readonly HiddenAgent[]): void {
  try {
    window.localStorage.setItem(HIDDEN_KEY, JSON.stringify(rows));
  } catch {
    /* no storage: hiding still works, it just does not survive a reload */
  }
}

/** Add one agent, or move an existing row to the name it has now. Newest last. */
export function addHidden(rows: readonly HiddenAgent[], row: HiddenAgent): HiddenAgent[] {
  return [...rows.filter((r) => r.id !== row.id), row];
}

export function removeHidden(rows: readonly HiddenAgent[], id: string): HiddenAgent[] {
  return rows.filter((r) => r.id !== id);
}

/**
 * Every id that is hidden outright or descends from something hidden.
 *
 * Walks parent links upward per agent rather than expanding the tree downward, so it
 * costs one pass and stays correct whatever order the feed lists nodes in. The `seen`
 * set makes a malformed cycle terminate instead of hanging the render.
 */
export function hiddenClosure(
  agents: readonly RadarAgent[],
  hiddenIds: ReadonlySet<string>,
): Set<string> {
  const parentOf = new Map<string, string | null>();
  for (const a of agents) parentOf.set(a.id, a.parentId);

  const out = new Set<string>();
  for (const a of agents) {
    const seen = new Set<string>();
    let cur: string | null = a.id;
    while (cur && !seen.has(cur)) {
      if (hiddenIds.has(cur)) {
        out.add(a.id);
        break;
      }
      seen.add(cur);
      cur = parentOf.get(cur) ?? null;
    }
  }
  return out;
}

/** The board, minus the mute list and everything under it. */
export function visibleAgents(
  agents: readonly RadarAgent[],
  hidden: readonly HiddenAgent[],
): RadarAgent[] {
  if (hidden.length === 0) return agents as RadarAgent[];
  const closure = hiddenClosure(agents, new Set(hidden.map((h) => h.id)));
  return agents.filter((a) => !closure.has(a.id));
}

/**
 * The restore list, in the order the rows are shown.
 *
 * Rows are marked `live` when the agent is still in the feed and `false` when the session
 * has since ended. Honest-viz: the row stays and stays restorable, but the list says
 * which is which rather than implying every muted name is still running. Newest first,
 * because the thing you just hid is the thing you are most likely to want back.
 */
export function hiddenRoster(
  agents: readonly RadarAgent[],
  hidden: readonly HiddenAgent[],
): (HiddenAgent & { live: boolean })[] {
  const present = new Set(agents.map((a) => a.id));
  return hidden.map((h) => ({ ...h, live: present.has(h.id) })).reverse();
}
