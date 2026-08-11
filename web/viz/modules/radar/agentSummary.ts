// agentSummary.ts: the detail panel's four-part read on one agent, in a form you can
// take in at a glance and that does NOT rewrite itself every second.
//
// The panel's first section answers four questions about an agent:
//   1. what it IS           identity, assembled by the panel from stable fields
//   2. what it has DONE      cumulative counts over its whole feed  (tally/files/actions)
//   3. what it is DOING      its one in-flight line, the only live part  (doing/target/clock)
//   4. what it will do NEXT  a grounded forward line, never a guess  (next)
//
// This replaces a single CHURNING `headline`. That line was the whole section, and it
// flipped between "Editing X", "Reading Y" and "Last ran Z" several times a second as
// the in-flight call turned over, so the part that never changes (what this agent is,
// how much it has done) was drowned by the part that changes constantly. Here the live
// call is ONE small line; the identity and the totals carry the section and hold still.
//
// HONEST-VIZ, the whole constraint. This is the one place in the app that renders
// PROSE, so it is the easiest place to state something the feed never said. Every
// count is off the real feed. `doing` is the agent's own status plus its own last
// action. `next` is derived ONLY from structural facts already on the agent (it is a
// subagent, it leads N children, it is idle, it is finished) and NEVER predicts a
// specific action: WARDEN watches, it cannot read an agent's mind. An agent's real
// plan is only knowable when the agent WROTE one (a todo list, a plan), and none is
// surfaced yet, so `next` stays a statement about role and state, not about intent.
//
// Pure and NO injected clock: nothing here depends on `now`. The live stopwatch is the
// panel's job (it owns the 1s tick), so `summarizeAgent` returns clock BASES, not aged
// values, and the whole block memoises on `key`, a value signature of every field it
// depends on. Guarding by value, never by the agent object (the radar poll hands a
// fresh object every 750ms), is the same discipline the camera rig uses next door.

import type { RadarActivity, RadarAgent } from '@/viz/shared/types/radarTypes';

/** The activity kinds worth counting separately in the tally. */
const TALLY_ORDER = ['write', 'run', 'read', 'search', 'tool', 'message', 'thinking'] as const;

const TALLY_NOUN: Record<string, [one: string, many: string]> = {
  read: ['file read', 'files read'],
  write: ['edit', 'edits'],
  search: ['search', 'searches'],
  run: ['command', 'commands'],
  tool: ['tool call', 'tool calls'],
  message: ['message', 'messages'],
  thinking: ['thinking step', 'thinking steps'],
};

/** Present-tense verb for a kind, for the "doing right now" line. */
const ACTIVE_VERB: Record<string, string> = {
  read: 'Reading',
  write: 'Editing',
  search: 'Searching',
  run: 'Running a command',
  tool: 'Running a tool',
  message: 'Writing a message',
  thinking: 'Thinking',
};

/** Past-tense verb for a kind, for the "last thing it did" sub-line. */
const DONE_VERB: Record<string, string> = {
  read: 'Last read',
  write: 'Last edited',
  search: 'Last searched',
  run: 'Last ran a command',
  tool: 'Last ran a tool',
  message: 'Last wrote a message',
  thinking: 'Last thought',
};

export type SummaryTally = { kind: string; count: number; label: string };

export type AgentSummary = {
  // Part 2: what it has done since it began (cumulative, only grows).
  /** Total rows the tally was computed from. Zero means an empty feed. */
  actions: number;
  /** Counts by kind over the agent's whole recorded feed, biggest first. */
  tally: SummaryTally[];
  /** Distinct files this agent has touched, most recent first (capped for the rail). */
  files: string[];
  /** Epoch ms the session started, or null when unparseable. The panel ages it. */
  startedAtMs: number | null;

  // Part 3: what it is doing now (the ONE live line).
  /** The state line: "Editing agent.rs", "Working", "Idle", "Finished after 12 actions". */
  doing: string;
  /** Kind of the in-flight call, for the glyph; null when nothing is in flight. */
  doingKind: string | null;
  /** Whether `doing` describes an IN-FLIGHT call (vs a state with no live call). */
  inFlight: boolean;
  /** The one file `doing`/`lastAction` refers to, for preview + reveal; else null. */
  target: string | null;
  /** The last finished step, shown muted under an idle/working state; null in flight
   * (the live line already names it) or on an empty feed. */
  lastAction: string | null;
  /** Epoch ms to subtract from `now` for the live clock (start of the in-flight call,
   * or the last activity). Null when there is nothing to age. */
  clockBaseMs: number | null;
  /** A FIXED elapsed to show when `clockBaseMs` can't be derived (an unparseable
   * startedAt), so the readout is the backend's count rather than NaN or nothing. */
  clockFixedMs: number | null;
  /** What the clock measures: 'Elapsed' for a live call, 'Since' for the last one. */
  clockLabel: 'Elapsed' | 'Since' | null;

  // Part 4: what it is going to do next (grounded, never fabricated).
  /** A one-liner on what happens next, from role + state only; null when there is
   * genuinely nothing to say (a finished agent has no "next"). */
  next: string | null;

  /** Value signature of every field above. Memoise on THIS, not on the agent object:
   * the radar poll hands a fresh object every 750ms and would bust an identity memo. */
  key: string;
};

function basename(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1] || path;
}

function noun(kind: string, count: number): string {
  const pair = TALLY_NOUN[kind];
  if (!pair) return count === 1 ? kind || 'action' : `${kind || 'action'}s`;
  return count === 1 ? pair[0] : pair[1];
}

/**
 * Newest feed row first. Rows with an unparseable timestamp keep their original order
 * and sink to the end, matching the activity feed's own ordering so the two sections
 * can never disagree about which action was last.
 */
function newestFirst(rows: RadarActivity[]): RadarActivity[] {
  return rows
    .map((a, i) => ({ a, i, t: Date.parse(a.ts) }))
    .sort((x, y) => {
      const xt = Number.isFinite(x.t) ? x.t : -Infinity;
      const yt = Number.isFinite(y.t) ? y.t : -Infinity;
      return yt - xt || x.i - y.i;
    })
    .map(({ a }) => a);
}

/**
 * What happens next, from structural facts ONLY. Never a predicted action: an agent's
 * real plan is only knowable when it wrote one, and none is surfaced yet. A finished
 * agent returns null (there is no next), which is the honest answer.
 */
function nextLine(agent: RadarAgent): string | null {
  const status = agent.status;
  if (status === 'terminated' || status === 'closed') return null;

  const children = Math.max(0, agent.childCount ?? 0);
  if (children > 0) {
    return `Coordinating ${children} subagent${children === 1 ? '' : 's'}`;
  }
  // A subagent (depth > 0) hands its result back to the lead that spawned it. That is
  // a fact of the harness, not a prediction, so it is honest whether it is mid-run or
  // between calls.
  if ((agent.depth ?? 0) > 0) {
    return status === 'idle' ? 'Waiting, then reports to its lead' : 'Working, then reports to its lead';
  }
  // A root session.
  if (status === 'idle') return 'Idle, waiting for the next instruction';
  return 'Continuing its run';
}

/**
 * Everything the summary section renders, derived from one agent. `fileLimit` caps the
 * distinct-file list; the cap is a LAYOUT decision (the rail is narrow), and the count
 * of everything it touched is still exact in `tally`, so nothing is hidden by it that
 * is not also stated.
 */
export function summarizeAgent(agent: RadarAgent, fileLimit = 4): AgentSummary {
  const rows = newestFirst(agent.recentActivity ?? []);
  const action = agent.currentAction ?? null;

  const counts = new Map<string, number>();
  const files: string[] = [];
  for (const row of rows) {
    counts.set(row.kind, (counts.get(row.kind) ?? 0) + 1);
    const target = row.target ?? null;
    if (target && !files.includes(target)) files.push(target);
  }

  const tally: SummaryTally[] = [...counts.entries()]
    .map(([kind, count]) => ({ kind, count, label: `${count} ${noun(kind, count)}` }))
    .sort((a, b) => {
      if (b.count !== a.count) return b.count - a.count;
      // Stable, meaningful tie-break: the kinds that say the most about what an agent
      // did (it CHANGED things, it RAN things) come before the ones that only say it
      // looked around.
      const ia = TALLY_ORDER.indexOf(a.kind as (typeof TALLY_ORDER)[number]);
      const ib = TALLY_ORDER.indexOf(b.kind as (typeof TALLY_ORDER)[number]);
      return (ia < 0 ? TALLY_ORDER.length : ia) - (ib < 0 ? TALLY_ORDER.length : ib);
    });

  const newest = rows[0] ?? null;
  const newestTs = newest ? Date.parse(newest.ts) : NaN;
  const newestMs = Number.isFinite(newestTs) ? newestTs : null;
  const startedTs = Date.parse(agent.startedAt);
  const startedAtMs = Number.isFinite(startedTs) ? startedTs : null;

  const base = {
    actions: rows.length,
    tally,
    files: files.slice(0, fileLimit),
    startedAtMs,
    next: nextLine(agent),
  };

  // An in-flight call outranks everything: it is happening right now, and it is the one
  // fact people used to lead with. The clock ages from `startedAt` in the panel; the
  // backend's own elapsed is the fallback when that timestamp will not parse.
  if (action) {
    const startMs = Date.parse(action.startedAt);
    const clockBaseMs = Number.isFinite(startMs) ? startMs : null;
    const verb = ACTIVE_VERB[action.kind] ?? `Running ${action.tool}`;
    const doing = action.target ? `${verb} ${basename(action.target)}` : action.label || verb;
    return {
      ...base,
      doing,
      doingKind: action.kind,
      inFlight: true,
      target: action.target,
      lastAction: null,
      clockBaseMs,
      clockFixedMs: clockBaseMs === null ? Math.max(0, action.elapsedMs) : null,
      clockLabel: 'Elapsed',
      key: summaryKey(agent, base.actions, newestMs, action.kind, `${action.target ?? action.label}@${action.startedAt}`),
    };
  }

  // Nothing in flight. `doing` is the plain STATE; the last finished step rides under it
  // as muted context, so an idle agent still says what it was doing without a churning
  // verb up top. A finished agent states the SIZE of what it did instead.
  const finished = agent.status === 'terminated' || agent.status === 'closed';
  let doing: string;
  if (finished) {
    doing = rows.length > 0 ? `Finished after ${rows.length} action${rows.length === 1 ? '' : 's'}` : 'Finished';
  } else {
    doing = agent.status === 'working' ? 'Working' : 'Idle';
  }

  let lastAction: string | null = null;
  if (!finished && newest) {
    const t = newest.target ?? null;
    const verb = DONE_VERB[newest.kind] ?? 'Last action';
    lastAction = t ? `${verb} ${basename(t)}` : `${verb}: ${newest.label || newest.kind}`;
  }

  return {
    ...base,
    doing,
    doingKind: !finished && newest ? newest.kind : null,
    inFlight: false,
    target: finished ? null : (newest?.target ?? null),
    lastAction,
    clockBaseMs: newestMs,
    clockFixedMs: null,
    clockLabel: newestMs !== null ? 'Since' : null,
    key: summaryKey(agent, base.actions, newestMs, null, ''),
  };
}

/** The memo signature. Time is bucketed out entirely (the live stopwatch is rendered
 * outside the memo from `now`), so nothing here changes on a tick, only on real data. */
function summaryKey(
  agent: RadarAgent,
  actions: number,
  newestMs: number | null,
  actionKind: string | null,
  actionSig: string,
): string {
  return [
    agent.id,
    agent.status,
    actions,
    newestMs ?? '',
    actionKind ?? '',
    actionSig,
    agent.childCount ?? 0,
    agent.depth ?? 0,
  ].join('|');
}
