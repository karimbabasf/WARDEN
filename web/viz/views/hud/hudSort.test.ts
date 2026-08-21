import { describe, expect, it } from 'vitest';
import { HUD_MAX_KIDS, hudAgents, hudCellLabel, hudCellTooltip, hudKidCount, hudSummary, hudSummaryLabel, hudTree } from './hudSort';
import { normalizeRadarState } from '@/viz/shared/types/radarTypes';

function agent(over: Record<string, unknown>) {
  return normalizeRadarState({ agents: [{ id: 'x', depth: 0, status: 'idle', ...over }] }).agents[0];
}

function model(...agents: Array<Record<string, unknown>>) {
  return normalizeRadarState({ agents });
}

describe('hudAgents', () => {
  it('puts what needs a human first', () => {
    const m = model(
      { id: 'i', depth: 0, status: 'idle', startedAt: '1' },
      { id: 'w', depth: 0, status: 'working', startedAt: '1' },
      { id: 'a', depth: 0, status: 'awaiting', startedAt: '1' },
    );
    expect(hudAgents(m).map((a) => a.id)).toEqual(['a', 'w', 'i']);
  });

  it('drops subagents: the HUD shows the sessions, not the swarm', () => {
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'sub', depth: 1, parentId: 'root', status: 'working', startedAt: '1' },
    );
    expect(hudAgents(m).map((a) => a.id)).toEqual(['root']);
  });

  it('drops dead sessions so they cannot bury a live one', () => {
    const m = model(
      { id: 'dead', depth: 0, status: 'closed', startedAt: '1' },
      { id: 'gone', depth: 0, status: 'terminated', startedAt: '1' },
      { id: 'live', depth: 0, status: 'idle', startedAt: '1' },
    );
    expect(hudAgents(m).map((a) => a.id)).toEqual(['live']);
  });

  it('adds a new agent at the END of its group, never shuffling the ones on screen', () => {
    const m = model(
      { id: 'old', depth: 0, status: 'working', startedAt: '2026-08-20T09:00:00Z' },
      { id: 'new', depth: 0, status: 'working', startedAt: '2026-08-20T11:00:00Z' },
    );
    expect(hudAgents(m).map((a) => a.id)).toEqual(['old', 'new']);
  });

  it('survives an absent model', () => {
    expect(hudAgents(undefined)).toEqual([]);
  });
});

describe('hudSummary', () => {
  it('counts over exactly what the grid draws', () => {
    const agents = hudAgents(
      model(
        { id: 'a', depth: 0, status: 'awaiting', startedAt: '1' },
        { id: 'w', depth: 0, status: 'working', startedAt: '1' },
        { id: 'x', depth: 0, status: 'working', startedAt: '2' },
        { id: 'i', depth: 0, status: 'idle', startedAt: '1' },
        { id: 'd', depth: 0, status: 'closed', startedAt: '1' },
      ),
    );
    expect(hudSummary(agents)).toEqual({ total: 4, working: 2, awaiting: 1 });
  });
});

describe('hudSummaryLabel', () => {
  it('names awaiting before working', () => {
    expect(hudSummaryLabel({ total: 4, working: 2, awaiting: 1 })).toBe('4 agents · 1 awaiting · 2 working');
  });

  it('says nothing it cannot back up', () => {
    expect(hudSummaryLabel({ total: 0, working: 0, awaiting: 0 })).toBe('No agents running');
    expect(hudSummaryLabel({ total: 1, working: 0, awaiting: 0 })).toBe('1 agent');
  });
});

describe('hudCellLabel', () => {
  it('prefers the name a human gave it', () => {
    expect(hudCellLabel(agent({ nickname: 'switchboard', label: 'l', cwd: 'sw' }))).toBe('switchboard');
  });

  it('names the TASK, not the folder: agents mostly run from home', () => {
    expect(hudCellLabel(agent({ label: 'rebuild the quote builder', cwd: 'karimbaba' })))
      .toBe('rebuild the quote builder');
  });

  it('falls back to the folder only when the session never named itself', () => {
    expect(hudCellLabel(agent({ cwd: 'WARDEN' }))).toBe('WARDEN');
  });

  it('always returns something, even for a bare agent', () => {
    expect(hudCellLabel(agent({ id: 'abcdef123456' }))).toBe('abcdef12');
  });
});

describe('hudTree', () => {
  it('hangs live subagents under the session they belong to', () => {
    const m = model(
      { id: 'r1', depth: 0, status: 'working', startedAt: '1' },
      { id: 'r2', depth: 0, status: 'working', startedAt: '2' },
      { id: 's1', depth: 1, parentId: 'r1', status: 'working', startedAt: '3' },
      { id: 's2', depth: 1, parentId: 'r2', status: 'idle', startedAt: '4' },
    );
    const t = hudTree(m);
    expect(t.map((n) => n.agent.id)).toEqual(['r1', 'r2']);
    expect(t[0].kids.map((k) => k.id)).toEqual(['s1']);
    expect(t[1].kids.map((k) => k.id)).toEqual(['s2']);
  });

  it('walks a deep chain up to the ROOT, never to the nearest parent', () => {
    // A depth-2 agent's parent is another subagent. Attaching it to that subagent
    // would put it nowhere, since only roots get a cell.
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'mid', depth: 1, parentId: 'root', status: 'working', startedAt: '2' },
      { id: 'deep', depth: 2, parentId: 'mid', status: 'working', startedAt: '3' },
    );
    expect(hudTree(m)[0].kids.map((k) => k.id)).toEqual(['mid', 'deep']);
  });

  it('drops an orphan rather than hanging it off the wrong session', () => {
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'lost', depth: 1, parentId: 'vanished', status: 'working', startedAt: '2' },
    );
    expect(hudTree(m)[0].kids).toEqual([]);
  });

  it('survives a parent chain that loops', () => {
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'a', depth: 1, parentId: 'b', status: 'working', startedAt: '2' },
      { id: 'b', depth: 1, parentId: 'a', status: 'working', startedAt: '3' },
    );
    expect(hudTree(m)[0].kids).toEqual([]);
  });

  it('drops dead subagents, exactly as it drops dead sessions', () => {
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'done', depth: 1, parentId: 'root', status: 'closed', startedAt: '2' },
      { id: 'live', depth: 1, parentId: 'root', status: 'working', startedAt: '3' },
    );
    expect(hudTree(m)[0].kids.map((k) => k.id)).toEqual(['live']);
  });

  it('orders the moons need-first, like the sessions above them', () => {
    const m = model(
      { id: 'root', depth: 0, status: 'working', startedAt: '1' },
      { id: 'i', depth: 1, parentId: 'root', status: 'idle', startedAt: '2' },
      { id: 'a', depth: 1, parentId: 'root', status: 'awaiting', startedAt: '3' },
      { id: 'w', depth: 1, parentId: 'root', status: 'working', startedAt: '4' },
    );
    expect(hudTree(m)[0].kids.map((k) => k.id)).toEqual(['a', 'w', 'i']);
  });

  it('caps the strip and counts the rest instead of losing them', () => {
    const kids = Array.from({ length: HUD_MAX_KIDS + 4 }, (_, i) => ({
      id: `s${i}`, depth: 1, parentId: 'root', status: 'working', startedAt: `${100 + i}`,
    }));
    const t = hudTree(model({ id: 'root', depth: 0, status: 'working', startedAt: '1' }, ...kids));
    expect(t[0].kids).toHaveLength(HUD_MAX_KIDS);
    expect(t[0].hiddenKids).toBe(4);
    expect(hudKidCount(t[0])).toBe(HUD_MAX_KIDS + 4);
  });
});

describe('hudCellTooltip', () => {
  it('carries what the cell had to truncate', () => {
    const a = agent({ label: 'rebuild the quote builder', cwd: 'Pakkr', repo: 'pakkr-main', model: 'claude-opus-5', status: 'working' });
    expect(hudCellTooltip(a, 'Claude', 2)).toBe(
      'rebuild the quote builder · Claude · pakkr-main/Pakkr · claude-opus-5 · working · 2 subagents',
    );
  });

  it('drops what the backend never said, rather than printing a gap', () => {
    expect(hudCellTooltip(agent({ label: 'x', status: 'idle' }), 'Codex')).toBe('x · Codex · idle');
  });
});
