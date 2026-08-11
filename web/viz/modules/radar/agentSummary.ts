// agentSummary.ts: "what is this agent actually doing", in one readable block.
//
// This replaces the CURRENT ACTION hero, which answered a narrower question than the
// one people ask of it. The in-flight tool call is a single frame of a session: it is
// null for every idle agent, it is null in the gaps between calls, and even when it is
// there it says "Edit" and a path without saying that this is the fourteenth edit in a
// run of work on the same file. The panel's first section should answer "what has this
// thing been doing", and the live call is one line of that answer, not all of it.
//
// HONEST-VIZ, which is the whole constraint here. A summary is the easiest place in
// the app to invent something: it is prose, and prose reads as authoritative. So every
// sentence below is a COUNT or a LABEL that came off the real feed. There is no model
// in this file, nothing is inferred about intent, and nothing is smoothed over. When
// the feed is empty the summary says the feed is empty. `headline` is assembled from
// the agent's own status plus its own last action, so the worst case is that it is
// terse, never that it is wrong.
//
// Pure and injectable-clock, so the whole thing is unit-tested without a backend.

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

/** Present-tense verb for a kind, for the "doing right now" headline. */
const ACTIVE_VERB: Record<string, string> = {
  read: 'Reading',
  write: 'Editing',
  search: 'Searching',
  run: 'Running a command',
  tool: 'Running a tool',
  message: 'Writing a message',
  thinking: 'Thinking',
};

/** Past-tense verb for a kind, for the "last thing it did" headline. */
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
  /** One sentence: what it is doing, or what it last did. Never fabricated. */
  headline: string;
  /** The file or command the headline refers to, when there is exactly one. */
  target: string | null;
  /** Whether `headline` describes something IN FLIGHT (vs the last finished step). */
  inFlight: boolean;
  /** How long the in-flight call has been running, in ms. Null when nothing is. */
  elapsedMs: number | null;
  /** Counts by kind over the agent's whole recorded feed, biggest first. */
  tally: SummaryTally[];
  /** Distinct files this agent has touched, most recent first. */
  files: string[];
  /** Total rows the tally was computed from. Zero means an empty feed. */
  actions: number;
  /** Age of the newest feed row in ms, or null when there is none. */
  lastActivityMs: number | null;
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
 * Newest feed row first. Rows with an unparseable timestamp keep their original
 * order and sink to the end, matching the activity feed's own ordering so the two
 * sections can never disagree about which action was last.
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
 * Everything the summary section renders, derived from one agent.
 *
 * `now` is injected so the elapsed/idle readouts are testable. `fileLimit` caps the
 * distinct-file list; the cap is a LAYOUT decision (the rail is narrow), and the
 * count of everything it touched is still exact in `tally`, so nothing is hidden by
 * it that is not also stated.
 */
export function summarizeAgent(
  agent: RadarAgent,
  now: number = Date.now(),
  fileLimit = 4,
): AgentSummary {
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
      // Stable, meaningful tie-break: the kinds that say the most about what an
      // agent did (it CHANGED things, it RAN things) come before the ones that
      // only say it looked around.
      const ia = TALLY_ORDER.indexOf(a.kind as (typeof TALLY_ORDER)[number]);
      const ib = TALLY_ORDER.indexOf(b.kind as (typeof TALLY_ORDER)[number]);
      return (ia < 0 ? TALLY_ORDER.length : ia) - (ib < 0 ? TALLY_ORDER.length : ib);
    });

  const newest = rows[0] ?? null;
  const newestTs = newest ? Date.parse(newest.ts) : NaN;
  const lastActivityMs = Number.isFinite(newestTs) ? Math.max(0, now - newestTs) : null;

  // An in-flight call outranks everything: it is happening right now, and it is the
  // one fact the panel used to lead with. Elapsed is re-derived from `startedAt` so
  // it ages between backend snapshots, falling back to the backend's own count when
  // the timestamp will not parse (never NaN, never a frozen zero).
  if (action) {
    const startMs = Date.parse(action.startedAt);
    const elapsedMs = Number.isFinite(startMs) ? Math.max(0, now - startMs) : action.elapsedMs;
    const verb = ACTIVE_VERB[action.kind] ?? `Running ${action.tool}`;
    const headline = action.target ? `${verb} ${basename(action.target)}` : action.label || verb;
    return {
      headline,
      target: action.target,
      inFlight: true,
      elapsedMs,
      tally,
      files: files.slice(0, fileLimit),
      actions: rows.length,
      lastActivityMs,
    };
  }

  // Nothing in flight. Say what it last did rather than only that it is idle: "Idle"
  // alone is the line that made people open the feed to find out anything at all.
  let headline: string;
  let target: string | null = null;
  if (agent.status === 'terminated' || agent.status === 'closed') {
    headline = rows.length > 0 ? `Finished after ${rows.length} action${rows.length === 1 ? '' : 's'}` : 'Finished';
  } else if (newest) {
    target = newest.target ?? null;
    const verb = DONE_VERB[newest.kind] ?? 'Last action';
    headline = target ? `${verb} ${basename(target)}` : `${verb}: ${newest.label || newest.kind}`;
  } else {
    headline = 'No recorded actions yet';
  }

  return {
    headline,
    target,
    inFlight: false,
    elapsedMs: null,
    tally,
    files: files.slice(0, fileLimit),
    actions: rows.length,
    lastActivityMs,
  };
}
