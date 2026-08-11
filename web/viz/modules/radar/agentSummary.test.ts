// The summary is the one place in the panel that renders PROSE, which makes it the
// easiest place to accidentally state something the feed never said. These tests are
// mostly about that: every sentence and every number has to be traceable to a row.

import { describe, expect, it } from 'vitest';
import { summarizeAgent } from './agentSummary';
import type { RadarActivity, RadarAgent } from '@/viz/shared/types/radarTypes';

const NOW = Date.parse('2026-06-23T22:00:00.000Z');

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
    startedAt: '2026-06-23T21:00:00Z',
    estCostUsd: null,
    ...over,
  } as unknown as RadarAgent;
}

function row(over: Partial<RadarActivity> = {}): RadarActivity {
  return { ts: '2026-06-23T21:59:00Z', kind: 'read', label: 'Read a.rs', target: '~/w/a.rs', ...over };
}

describe('summarizeAgent', () => {
  it('leads with the in-flight call, named by its file', () => {
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
      NOW,
    );
    expect(s.inFlight).toBe(true);
    expect(s.headline).toBe('Editing agent.rs');
    expect(s.target).toBe('~/w/src/agent.rs');
    // Elapsed is re-derived from startedAt so it ages between backend snapshots.
    expect(s.elapsedMs).toBe(30_000);
  });

  it('falls back to the backend elapsed when startedAt will not parse, never NaN', () => {
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
      NOW,
    );
    expect(s.elapsedMs).toBe(4_200);
    // No file, so the harness's own label is the headline rather than a bare verb.
    expect(s.headline).toBe('pnpm test');
    expect(s.target).toBeNull();
  });

  it('says what the agent LAST did when nothing is in flight', () => {
    const s = summarizeAgent(
      agent({
        currentAction: null,
        recentActivity: [
          row({ ts: '2026-06-23T21:58:00Z', kind: 'read', target: '~/w/old.rs' }),
          row({ ts: '2026-06-23T21:59:00Z', kind: 'write', label: 'Edit new.rs', target: '~/w/new.rs' }),
        ],
      }),
      NOW,
    );
    expect(s.inFlight).toBe(false);
    expect(s.headline).toBe('Last edited new.rs');
    expect(s.lastActivityMs).toBe(60_000);
  });

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
      NOW,
    );
    expect(s.actions).toBe(5);
    const labels = s.tally.map((t) => t.label);
    expect(labels).toContain('3 files read');
    expect(labels).toContain('1 edit');
    expect(labels).toContain('1 command');
    // Biggest first, so the dominant kind of work reads first.
    expect(s.tally[0].kind).toBe('read');
  });

  it('breaks a count tie toward the kinds that changed or ran something', () => {
    const s = summarizeAgent(
      agent({
        recentActivity: [
          row({ kind: 'read', target: '~/w/a.rs' }),
          row({ kind: 'write', target: '~/w/b.rs' }),
        ],
      }),
      NOW,
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
      NOW,
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
      NOW,
      2,
    );
    expect(s.files).toEqual(['~/w/a.rs', '~/w/b.rs']);
    // The cap is a layout decision, so it must not quietly shrink the tally too.
    expect(s.tally[0].label).toBe('3 files read');
    expect(s.actions).toBe(3);
  });

  // Honest-viz: an empty feed says it is empty. It does not guess from the status,
  // the uptime, or the token count.
  it('says so plainly when there is nothing recorded', () => {
    const s = summarizeAgent(agent({ recentActivity: [], currentAction: null }), NOW);
    expect(s.headline).toBe('No recorded actions yet');
    expect(s.tally).toEqual([]);
    expect(s.files).toEqual([]);
    expect(s.actions).toBe(0);
    expect(s.lastActivityMs).toBeNull();
  });

  it('reads a finished agent as finished, with the size of what it did', () => {
    const s = summarizeAgent(
      agent({
        status: 'terminated',
        currentAction: null,
        recentActivity: [row(), row({ ts: '2026-06-23T21:58:00Z' })],
      }),
      NOW,
    );
    expect(s.headline).toBe('Finished after 2 actions');
  });

  it('survives an unparseable timestamp instead of ordering by NaN', () => {
    const s = summarizeAgent(
      agent({
        currentAction: null,
        recentActivity: [
          row({ ts: 'nonsense', kind: 'write', label: 'Edit broken.rs', target: '~/w/broken.rs' }),
          row({ ts: '2026-06-23T21:59:00Z', kind: 'read', target: '~/w/good.rs' }),
        ],
      }),
      NOW,
    );
    // The row with a real timestamp wins; the unparseable one sinks rather than
    // taking the headline with a NaN sort.
    expect(s.headline).toBe('Last read good.rs');
    expect(s.actions).toBe(2);
  });
});
