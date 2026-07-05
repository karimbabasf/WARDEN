import { describe, expect, it } from 'vitest';
import { buildRadarRoster, type HarnessGroup } from './rosterTree';
import type { RadarAgent } from './radarTypes';

// rosterTree is the PURE source of the sidebar list: it groups the live forest /
// the habit orbs by harness (one source of truth = harnessColors) and, for radar,
// nests subagents under their root so a 20-25 agent fleet reads as a scannable
// tree. No React, no colour-to-CSS here — the component maps status/severity to the
// dot. Honest-viz: every agent/issue maps to a row, an orphaned subagent is never
// dropped, and an empty harness simply yields no group.

function agent(over: Partial<RadarAgent> = {}): RadarAgent {
  return {
    id: 'a',
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: 'task',
    nickname: null,
    cwd: null,
    cwdPath: null,
    role: null,
    model: null,
    status: 'idle',
    contextTokens: 0,
    maxTokens: 0,
    fillPct: 0,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...over,
  };
}

const ids = (g: HarnessGroup) => g.rows.map((r) => r.id);

describe('buildRadarRoster — harness grouping', () => {
  it('orders groups Claude, Codex, other (alpha), Unknown last — present harnesses only', () => {
    const groups = buildRadarRoster([
      agent({ id: 'u', harness: 'unknown' }),
      agent({ id: 'g', harness: 'gemini' }),
      agent({ id: 'x', harness: 'codex' }),
      agent({ id: 'c', harness: 'claude_code' }),
    ]);
    expect(groups.map((g) => g.harness)).toEqual(['claude_code', 'codex', 'gemini', 'unknown']);
  });

  it('omits a harness with no agents (never a fabricated empty group)', () => {
    const groups = buildRadarRoster([agent({ id: 'c', harness: 'claude_code' })]);
    expect(groups.map((g) => g.harness)).toEqual(['claude_code']);
  });

  it('labels each group from the single harness-colour source (Claude ◆ #ff8636)', () => {
    const [claude] = buildRadarRoster([agent({ harness: 'claude_code' })]);
    expect(claude.label).toBe('Claude');
    expect(claude.glyph).toBe('◆');
    expect(claude.color).toBe('#ff8636');
  });
});

describe('buildRadarRoster — root ordering + subagent nesting', () => {
  it('orders roots working-first, then by title', () => {
    const groups = buildRadarRoster([
      agent({ id: 'idle-z', label: 'zeta', status: 'idle' }),
      agent({ id: 'work-b', label: 'beta', status: 'working' }),
      agent({ id: 'idle-a', label: 'alpha', status: 'idle' }),
    ]);
    expect(ids(groups[0])).toEqual(['work-b', 'idle-a', 'idle-z']);
  });

  it('nests subagents directly under their root in DFS order, indented by depth', () => {
    const groups = buildRadarRoster([
      agent({ id: 'root', label: 'root', depth: 0 }),
      agent({ id: 'kid-b', label: 'kid-b', parentId: 'root', depth: 1 }),
      agent({ id: 'kid-a', label: 'kid-a', parentId: 'root', depth: 1, status: 'working' }),
      agent({ id: 'grandkid', label: 'gk', parentId: 'kid-a', depth: 2 }),
    ]);
    // root, then its working child (kid-a) + that child's grandkid, then kid-b.
    expect(ids(groups[0])).toEqual(['root', 'kid-a', 'grandkid', 'kid-b']);
    const byId = Object.fromEntries(groups[0].rows.map((r) => [r.id, r.depth]));
    expect(byId).toMatchObject({ root: 0, 'kid-a': 1, grandkid: 2, 'kid-b': 1 });
  });

  it('still lists an orphan subagent whose parent is absent (never drops an agent)', () => {
    const groups = buildRadarRoster([
      agent({ id: 'root', label: 'root' }),
      agent({ id: 'orphan', label: 'orphan', parentId: 'ghost', depth: 1 }),
    ]);
    expect(ids(groups[0]).sort()).toEqual(['orphan', 'root']);
  });
});

describe('buildRadarRoster — row content', () => {
  it('titles a row by nickname when present, else the label, and carries status', () => {
    const [g] = buildRadarRoster([
      agent({ id: 'n', label: 'the-task', nickname: 'Curie', status: 'working' }),
    ]);
    expect(g.rows[0].title).toBe('Curie');
    expect(g.rows[0].status).toBe('working');
  });

  it('falls back to the label, then a placeholder, so a row is never blank', () => {
    const [g] = buildRadarRoster([agent({ id: 'x', label: '', nickname: null })]);
    expect(g.rows[0].title.length).toBeGreaterThan(0);
  });

  it('derives the folder · model subtitle (null when it adds nothing)', () => {
    const [withSub] = buildRadarRoster([
      agent({ id: 's', label: 'task', cwd: 'WARDEN', model: 'claude-opus-4-8' }),
    ]);
    expect(withSub.rows[0].subtitle).toBe('WARDEN · opus');
    const [noSub] = buildRadarRoster([agent({ id: 'p', label: 'WARDEN', cwd: 'WARDEN', model: null })]);
    expect(noSub.rows[0].subtitle).toBeNull();
  });
});

