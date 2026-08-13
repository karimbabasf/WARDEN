// @vitest-environment jsdom
//
// The mute list is one level above the radar model, so a mistake here does not throw:
// it silently shows or drops an agent. These tests pin the three properties that would
// fail quietly otherwise: a hide takes its subtree, an ended session stays restorable,
// and a corrupt stored value never takes the board down with it.

import { beforeEach, describe, expect, it } from 'vitest';
import {
  addHidden,
  hiddenClosure,
  hiddenRoster,
  readHiddenAgents,
  removeHidden,
  visibleAgents,
  writeHiddenAgents,
} from './hiddenAgents';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';

function agent(id: string, parentId: string | null = null): RadarAgent {
  return {
    id,
    harness: 'claude_code',
    origin: 'cli',
    parentId,
    depth: parentId ? 1 : 0,
    label: id,
    nickname: null,
    repo: null,
    cwd: null,
    role: null,
    model: null,
    status: 'working',
    contextTokens: 0,
    maxTokens: 200_000,
    fillPct: 0,
    composition: { exact: null, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '2026-08-12T10:00:00Z',
    estCostUsd: null,
  } as unknown as RadarAgent;
}

describe('hiddenClosure: a hide takes the subtree with it', () => {
  // Leaving a subagent on the board after hiding its lead strands a globe whose
  // parent edge points at nothing, and the constellation reads an unparented globe
  // as a root: the board would grow a session that does not exist.
  const forest = [agent('root'), agent('kid', 'root'), agent('grandkid', 'kid'), agent('other')];

  it('drops descendants at every depth, not just direct children', () => {
    const closure = hiddenClosure(forest, new Set(['root']));
    expect([...closure].sort()).toEqual(['grandkid', 'kid', 'root']);
  });

  it('leaves an unrelated tree alone', () => {
    expect(visibleAgents(forest, [{ id: 'root', name: 'root' }]).map((a) => a.id)).toEqual(['other']);
  });

  it('hides a mid-tree agent without touching its ancestors', () => {
    const visible = visibleAgents(forest, [{ id: 'kid', name: 'kid' }]).map((a) => a.id);
    expect(visible).toEqual(['root', 'other']);
  });

  it('terminates on a malformed parent cycle rather than hanging the render', () => {
    const cycle = [agent('a', 'b'), agent('b', 'a')];
    expect(() => hiddenClosure(cycle, new Set(['a']))).not.toThrow();
    expect(hiddenClosure(cycle, new Set(['a'])).has('a')).toBe(true);
  });

  it('is a no-op when nothing is muted', () => {
    expect(visibleAgents(forest, [])).toHaveLength(4);
  });
});

describe('hiddenRoster: the list you restore from', () => {
  const forest = [agent('live-one')];

  it('marks a muted session that has since left the feed rather than dropping its row', () => {
    const rows = hiddenRoster(forest, [
      { id: 'live-one', name: 'warden' },
      { id: 'gone-one', name: 'pakkr' },
    ]);
    // Newest first: the thing just hidden is the thing most likely wanted back.
    expect(rows.map((r) => r.id)).toEqual(['gone-one', 'live-one']);
    expect(rows.find((r) => r.id === 'gone-one')?.live).toBe(false);
    expect(rows.find((r) => r.id === 'live-one')?.live).toBe(true);
    // The name survives the session, which is the whole reason it is stored.
    expect(rows.find((r) => r.id === 'gone-one')?.name).toBe('pakkr');
  });
});

describe('addHidden / removeHidden', () => {
  it('re-hiding an agent refreshes its name instead of queueing a duplicate row', () => {
    const once = addHidden([], { id: 'a', name: 'old name' });
    const twice = addHidden(once, { id: 'a', name: 'new name' });
    expect(twice).toEqual([{ id: 'a', name: 'new name' }]);
  });

  it('removes by id and leaves the rest in order', () => {
    const rows = [
      { id: 'a', name: 'a' },
      { id: 'b', name: 'b' },
      { id: 'c', name: 'c' },
    ];
    expect(removeHidden(rows, 'b').map((r) => r.id)).toEqual(['a', 'c']);
  });
});

describe('readHiddenAgents: the stored value is untrusted', () => {
  beforeEach(() => window.localStorage.clear());

  it('round-trips what was written', () => {
    writeHiddenAgents([{ id: 'a', name: 'warden' }]);
    expect(readHiddenAgents()).toEqual([{ id: 'a', name: 'warden' }]);
  });

  it('returns an empty list for absent, non-JSON, or non-array storage', () => {
    expect(readHiddenAgents()).toEqual([]);
    window.localStorage.setItem('warden.fleet.hidden', 'not json');
    expect(readHiddenAgents()).toEqual([]);
    window.localStorage.setItem('warden.fleet.hidden', '{"id":"a"}');
    expect(readHiddenAgents()).toEqual([]);
  });

  it('drops malformed rows instead of letting one bad entry hide the whole board', () => {
    window.localStorage.setItem(
      'warden.fleet.hidden',
      JSON.stringify([{ id: 'good', name: 'ok' }, { id: '' }, null, { name: 'no id' }, 7]),
    );
    expect(readHiddenAgents()).toEqual([{ id: 'good', name: 'ok' }]);
  });
});
