// The summary is the one place in the panel that renders PROSE, which makes it the
// easiest place to accidentally state something the feed never said. These tests are
// mostly about that: every sentence and every number has to be traceable to a row, and
// the forward line ("next") must stay a statement of role and state, never a guess.

import { describe, expect, it } from 'vitest';
import { summarizeAgent } from './agentSummary';
import type { RadarActivity, RadarAgent } from '@/viz/shared/types/radarTypes';

function agent(over: Partial<RadarAgent> = {}): RadarAgent {
  return {
    id: 'a1',
    harness: 'claude_code',
    origin: 'root',
    parentId: null,
    depth: 0,
    label: 'warden',
    nickname: null,
    cwd: '~/Developer/Apps/WARDEN',
    repo: null,
    role: null,
    model: 'claude-opus-5',
    title: null,
    currentAction: null,
    team: null,
    status: 'working',
    contextTokens: 1000,
    maxTokens: 200000,
    fillPct: 0.005,
    contextBreakdown: null,
    composition: {
      exact: { cacheRead: 0, fresh: 0, cacheWrite: 0, output: 0 },
    },
    recentActivity: [],
    childCount: 0,
    startedAt: '2026-06-23T21:00:00Z',
    estCostUsd: null,
    ...over,
  } as unknown as RadarAgent;
}

function row(over: Partial<RadarActivity> = {}): RadarActivity {
  return { ts: '2026-06-23T21:59:00Z', kind: 'read', label: 'Read a.rs', target: '~/w/a.rs', ...over };
}

describe('summarizeAgent: doing (part 3, the one live line)', () => {
  it('leads with the in-flight call, named by its file, aged from startedAt', () => {
    const s = summarizeAgent(
      agent({
        currentAction: {
          kind: 'write',
          tool: 'Edit',
          label: 'Edit agent.rs',
          target: '~/w/src/agent.rs',
          startedAt: '2026-06-23T21:59:30Z',
          elapsedMs: 0,
        },
      }),
    );
    expect(s.inFlight).toBe(true);
    expect(s.doing).toBe('Editing agent.rs');
    expect(s.doingKind).toBe('write');
    expect(s.target).toBe('~/w/src/agent.rs');
    // The clock is a BASE the panel ages against `now`, never a value computed here, so
    // the 1s tick lives outside this pure function.
    expect(s.clockBaseMs).toBe(Date.parse('2026-06-23T21:59:30Z'));
    expect(s.clockLabel).toBe('Elapsed');
    expect(s.clockFixedMs).toBeNull();
    expect(s.lastAction).toBeNull();
  });

  it('falls back to a FIXED backend elapsed when startedAt will not parse, never NaN', () => {
    const s = summarizeAgent(
      agent({
        currentAction: {
          kind: 'run',
          tool: 'Bash',
          label: 'pnpm test',
          target: null,
          startedAt: 'not-a-date',
          elapsedMs: 4_200,
        },
      }),
    );
    expect(s.clockBaseMs).toBeNull();
    expect(s.clockFixedMs).toBe(4_200);
    // No file, so the harness's own label is the line rather than a bare verb.
    expect(s.doing).toBe('pnpm test');
    expect(s.target).toBeNull();
  });

  it('says the plain state when nothing is in flight, with the last step under it', () => {
    const s = summarizeAgent(
      agent({
        status: 'idle',
        currentAction: null,
        recentActivity: [
          row({ ts: '2026-06-23T21:58:00Z', kind: 'read', target: '~/w/old.rs' }),
          row({ ts: '2026-06-23T21:59:00Z', kind: 'write', label: 'Edit new.rs', target: '~/w/new.rs' }),
        ],
      }),
    );
    expect(s.inFlight).toBe(false);
    expect(s.doing).toBe('Idle');
    // The last finished step is context, not a churning headline.
    expect(s.lastAction).toBe('Last edited new.rs');
    expect(s.target).toBe('~/w/new.rs');
    expect(s.clockLabel).toBe('Since');
    expect(s.clockBaseMs).toBe(Date.parse('2026-06-23T21:59:00Z'));
  });

  it('says "Working" between calls (busy, but no live tool call)', () => {
    const s = summarizeAgent(
      agent({ status: 'working', currentAction: null, recentActivity: [row({ kind: 'run', label: 'cargo test', target: null })] }),
    );
    expect(s.doing).toBe('Working');
    expect(s.lastAction).toBe('Last ran a command: cargo test');
  });

  it('reads a finished agent as finished, with the size of what it did', () => {
    const s = summarizeAgent(
      agent({
        status: 'terminated',
        currentAction: null,
        recentActivity: [row(), row({ ts: '2026-06-23T21:58:00Z' })],
      }),
    );
    expect(s.doing).toBe('Finished after 2 actions');
    expect(s.lastAction).toBeNull();
    expect(s.target).toBeNull();
  });
});

describe('summarizeAgent: done (part 2, cumulative counts)', () => {
  it('counts the whole feed exactly, and pluralises against the real count', () => {
    const s = summarizeAgent(
      agent({
        recentActivity: [
          row({ kind: 'read', target: '~/w/a.rs' }),
          row({ kind: 'read', target: '~/w/b.rs' }),
          row({ kind: 'read', target: '~/w/c.rs' }),
          row({ kind: 'write', target: '~/w/a.rs' }),
          row({ kind: 'run', target: null, label: 'cargo test' }),
        ],
      }),
    );
    expect(s.actions).toBe(5);
    const labels = s.tally.map((t) => t.label);
    expect(labels).toContain('3 files read');
    expect(labels).toContain('1 edit');
    expect(labels).toContain('1 command');
    expect(s.tally[0].kind).toBe('read'); // biggest first
  });

  it('breaks a count tie toward the kinds that changed or ran something', () => {
    const s = summarizeAgent(
      agent({ recentActivity: [row({ kind: 'read', target: '~/w/a.rs' }), row({ kind: 'write', target: '~/w/b.rs' })] }),
    );
    expect(s.tally.map((t) => t.kind)).toEqual(['write', 'read']);
  });

  it('lists distinct files, newest first, and never repeats one', () => {
    const s = summarizeAgent(
      agent({
        recentActivity: [
          row({ ts: '2026-06-23T21:57:00Z', target: '~/w/a.rs' }),
          row({ ts: '2026-06-23T21:58:00Z', target: '~/w/b.rs' }),
          row({ ts: '2026-06-23T21:59:00Z', target: '~/w/a.rs' }),
        ],
      }),
    );
    expect(s.files).toEqual(['~/w/a.rs', '~/w/b.rs']);
  });

  it('caps the file list for the rail while leaving the counts exact', () => {
    const s = summarizeAgent(
      agent({
        recentActivity: [
          row({ ts: '2026-06-23T21:59:00Z', target: '~/w/a.rs' }),
          row({ ts: '2026-06-23T21:58:00Z', target: '~/w/b.rs' }),
          row({ ts: '2026-06-23T21:57:00Z', target: '~/w/c.rs' }),
        ],
      }),
      2,
    );
    expect(s.files).toEqual(['~/w/a.rs', '~/w/b.rs']);
    expect(s.tally[0].label).toBe('3 files read'); // cap must not shrink the tally
    expect(s.actions).toBe(3);
  });

  it('exposes startedAt as a base for the panel to age, or null when unparseable', () => {
    expect(summarizeAgent(agent({ startedAt: '2026-06-23T21:00:00Z' })).startedAtMs).toBe(
      Date.parse('2026-06-23T21:00:00Z'),
    );
    expect(summarizeAgent(agent({ startedAt: 'nope' })).startedAtMs).toBeNull();
  });

  // Honest-viz: an empty feed says it is empty. It does not guess from the status, the
  // uptime, or the token count.
  it('says so plainly when there is nothing recorded', () => {
    const s = summarizeAgent(agent({ status: 'idle', recentActivity: [], currentAction: null }));
    expect(s.tally).toEqual([]);
    expect(s.files).toEqual([]);
    expect(s.actions).toBe(0);
    expect(s.doing).toBe('Idle');
    expect(s.lastAction).toBeNull();
    expect(s.clockBaseMs).toBeNull();
    expect(s.clockLabel).toBeNull();
  });

  it('survives an unparseable timestamp instead of ordering by NaN', () => {
    const s = summarizeAgent(
      agent({
        status: 'idle',
        currentAction: null,
        recentActivity: [
          row({ ts: 'nonsense', kind: 'write', label: 'Edit broken.rs', target: '~/w/broken.rs' }),
          row({ ts: '2026-06-23T21:59:00Z', kind: 'read', target: '~/w/good.rs' }),
        ],
      }),
    );
    // The row with a real timestamp wins; the unparseable one sinks rather than taking
    // the last-step line with a NaN sort.
    expect(s.lastAction).toBe('Last read good.rs');
    expect(s.actions).toBe(2);
  });
});

describe('summarizeAgent: next (part 4, grounded, never a guess)', () => {
  it('names how many subagents a lead is coordinating', () => {
    expect(summarizeAgent(agent({ childCount: 3 })).next).toBe('Coordinating 3 subagents');
    expect(summarizeAgent(agent({ childCount: 1 })).next).toBe('Coordinating 1 subagent');
  });

  it('says a subagent reports to its lead, by state', () => {
    expect(summarizeAgent(agent({ depth: 1, status: 'working', childCount: 0 })).next).toBe(
      'Working, then reports to its lead',
    );
    expect(summarizeAgent(agent({ depth: 1, status: 'idle', childCount: 0 })).next).toBe(
      'Waiting, then reports to its lead',
    );
  });

  it('says a root is waiting when idle, continuing when working', () => {
    expect(summarizeAgent(agent({ depth: 0, status: 'idle', childCount: 0 })).next).toBe(
      'Idle, waiting for the next instruction',
    );
    expect(summarizeAgent(agent({ depth: 0, status: 'working', childCount: 0 })).next).toBe('Continuing its run');
  });

  // A finished agent has no next. Honest-viz: null, not an invented plan.
  it('gives a finished agent no next line', () => {
    expect(summarizeAgent(agent({ status: 'terminated' })).next).toBeNull();
    expect(summarizeAgent(agent({ status: 'closed' })).next).toBeNull();
  });

  // A lead is coordinating whether or not it is mid-call, so children win over status.
  it('prefers the child count over the plain working line', () => {
    expect(summarizeAgent(agent({ depth: 0, status: 'working', childCount: 2 })).next).toBe('Coordinating 2 subagents');
  });
});

describe('summarizeAgent: key (memo signature, kills the churn)', () => {
  it('is identical across two calls with equal data (so a fresh poll object is a no-op)', () => {
    const a = agent({ recentActivity: [row()], status: 'working' });
    const b = agent({ recentActivity: [row()], status: 'working' });
    expect(summarizeAgent(a).key).toBe(summarizeAgent(b).key);
  });

  it('changes when real data changes: a new action, a status flip, a live call', () => {
    const baseKey = summarizeAgent(agent({ recentActivity: [row()] })).key;
    expect(summarizeAgent(agent({ recentActivity: [row(), row({ ts: '2026-06-23T21:58:00Z' })] })).key).not.toBe(baseKey);
    expect(summarizeAgent(agent({ recentActivity: [row()], status: 'idle' })).key).not.toBe(baseKey);
    expect(
      summarizeAgent(
        agent({
          recentActivity: [row()],
          currentAction: { kind: 'write', tool: 'Edit', label: 'x', target: '~/w/x.rs', startedAt: '2026-06-23T21:59:00Z', elapsedMs: 0 },
        }),
      ).key,
    ).not.toBe(baseKey);
  });
});
