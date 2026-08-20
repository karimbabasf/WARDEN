import { describe, expect, it } from 'vitest';
import { hudAgents, hudCellLabel, hudCellTooltip, hudSummary, hudSummaryLabel } from './hudSort';
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

describe('hudCellTooltip', () => {
  it('carries what the cell had to truncate', () => {
    const a = agent({ label: 'rebuild the quote builder', cwd: 'Pakkr', repo: 'pakkr-main', model: 'claude-opus-5', status: 'working', childCount: 2 });
    expect(hudCellTooltip(a, 'Claude')).toBe(
      'rebuild the quote builder · Claude · pakkr-main/Pakkr · claude-opus-5 · working · 2 subagents',
    );
  });

  it('drops what the backend never said, rather than printing a gap', () => {
    expect(hudCellTooltip(agent({ label: 'x', status: 'idle' }), 'Codex')).toBe('x · Codex · idle');
  });
});
