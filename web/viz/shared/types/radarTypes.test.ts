import { describe, expect, it } from 'vitest';
import { normalizeRadarState, radarSubtitle } from './radarTypes';

// The backend emits the frozen `radar_state` contract (camelCase). The frontend
// must survive missing optionals + out-of-range numbers without throwing, so
// `normalizeRadarState` is the one honest seam: raw contract JSON in, a fully
// defaulted `RadarSceneModel` out. Schema drift never crashes the constellation.

function fullAgent() {
  return {
    id: 'root-1',
    harness: 'claude_code',
    origin: 'claude-desktop',
    parentId: null,
    depth: 0,
    label: 'warden',
    nickname: null,
    role: null,
    model: 'claude-opus-4-8',
    status: 'working',
    contextTokens: 120000,
    maxTokens: 200000,
    fillPct: 0.6,
    contextBreakdown: {
      usedTokens: 120000,
      maxTokens: 200000,
      fillPct: 0.6,
      rows: [
        { key: 'messages', label: 'Messages', tokens: 86000, percent: 0.43, count: null },
        { key: 'mcp_tools', label: 'MCP tools', tokens: 12000, percent: 0.06, count: 4 },
        { key: 'free_space', label: 'Free space', tokens: 80000, percent: 0.4, count: null, muted: true },
      ],
    },
    composition: {
      exact: { cacheRead: 90000, fresh: 12000, output: 2620 },
      estimated: { preamble: 7000, conversation: 3000, toolOutput: 1500, thinking: 200 },
    },
    recentActivity: [{ ts: '2026-06-23T22:50:00Z', kind: 'tool', label: 'Read' }],
    childCount: 1,
    startedAt: '2026-06-23T22:00:00Z',
    estCostUsd: 0.42,
  };
}

describe('normalizeRadarState', () => {
  it('passes a full contract agent through unchanged', () => {
    const model = normalizeRadarState({ generatedAt: 'T0', agents: [fullAgent()] });
    expect(model.generatedAt).toBe('T0');
    expect(model.agents).toHaveLength(1);
    const a = model.agents[0];
    expect(a.id).toBe('root-1');
    expect(a.harness).toBe('claude_code');
    expect(a.depth).toBe(0);
    expect(a.parentId).toBeNull();
    expect(a.contextTokens).toBe(120000);
    expect(a.fillPct).toBeCloseTo(0.6);
    expect(a.contextBreakdown).toEqual({
      usedTokens: 120000,
      maxTokens: 200000,
      fillPct: 0.6,
      rows: [
        { key: 'messages', label: 'Messages', tokens: 86000, percent: 0.43, count: null, muted: false },
        { key: 'mcp_tools', label: 'MCP tools', tokens: 12000, percent: 0.06, count: 4, muted: false },
        { key: 'free_space', label: 'Free space', tokens: 80000, percent: 0.4, count: null, muted: true },
      ],
    });
    expect(a.composition.exact).toEqual({ cacheRead: 90000, fresh: 12000, output: 2620 });
    expect(a.composition.estimated).toEqual({ preamble: 7000, conversation: 3000, toolOutput: 1500, thinking: 200 });
    expect(a.recentActivity[0]).toEqual({ ts: '2026-06-23T22:50:00Z', kind: 'tool', label: 'Read' });
    expect(a.estCostUsd).toBeCloseTo(0.42);
  });

  it('carries the cwd (folder subtitle) through and defaults it to null when missing', () => {
    const withCwd = normalizeRadarState({ agents: [{ ...fullAgent(), cwd: 'WARDEN' }] });
    expect(withCwd.agents[0].cwd).toBe('WARDEN');
    const without = normalizeRadarState({ agents: [{ id: 'a', harness: 'codex', status: 'idle' }] });
    expect(without.agents[0].cwd).toBeNull();
  });

  it('defaults missing optionals (nickname/role/origin → null, estimated → null, estCostUsd → null)', () => {
    const model = normalizeRadarState({
      agents: [
        {
          id: 'a1',
          harness: 'codex',
          status: 'idle',
          // everything else missing
        },
      ],
    });
    const a = model.agents[0];
    expect(a.origin).toBeNull();
    expect(a.parentId).toBeNull();
    expect(a.nickname).toBeNull();
    expect(a.role).toBeNull();
    expect(a.model).toBeNull();
    expect(a.depth).toBe(0);
    expect(a.contextTokens).toBe(0);
    expect(a.maxTokens).toBe(0);
    expect(a.fillPct).toBe(0);
    expect(a.contextBreakdown).toEqual({ usedTokens: 0, maxTokens: 0, fillPct: 0, rows: [] });
    expect(a.childCount).toBe(0);
    expect(a.composition.exact).toEqual({ cacheRead: 0, fresh: 0, output: 0 });
    expect(a.composition.estimated).toBeNull();
    expect(a.recentActivity).toEqual([]);
    expect(a.estCostUsd).toBeNull();
    expect(a.startedAt).toBe('');
  });

  it('clamps fillPct into [0,1]', () => {
    const over = normalizeRadarState({ agents: [{ id: 'x', harness: 'claude_code', status: 'working', fillPct: 4.2 }] });
    expect(over.agents[0].fillPct).toBe(1);
    const under = normalizeRadarState({ agents: [{ id: 'y', harness: 'codex', status: 'idle', fillPct: -3 }] });
    expect(under.agents[0].fillPct).toBe(0);
  });

  it('coerces an unknown status to idle and a non-finite token count to 0', () => {
    const model = normalizeRadarState({
      agents: [{ id: 'z', harness: 'codex', status: 'bogus', contextTokens: Number.NaN, maxTokens: 'lots' }],
    });
    expect(model.agents[0].status).toBe('idle');
    expect(model.agents[0].contextTokens).toBe(0);
    expect(model.agents[0].maxTokens).toBe(0);
  });

  it('normalizes snake_case context-window rows and clamps row percentages', () => {
    const model = normalizeRadarState({
      agents: [
        {
          id: 'ctx',
          harness: 'codex',
          status: 'working',
          context_breakdown: {
            used_tokens: 66_000,
            max_tokens: 100_000,
            fill_pct: 0.66,
            rows: [
              { key: 'messages', label: 'Messages', tokens: 40_000, percent: 2, count: 9 },
              { key: 'free_space', label: 'Free space', tokens: 34_000, percent: -1, muted: true },
              'bad-row',
            ],
          },
        },
      ],
    });
    expect(model.agents[0].contextBreakdown).toEqual({
      usedTokens: 66_000,
      maxTokens: 100_000,
      fillPct: 0.66,
      rows: [
        { key: 'messages', label: 'Messages', tokens: 40_000, percent: 1, count: 9, muted: false },
        { key: 'free_space', label: 'Free space', tokens: 34_000, percent: 0, count: null, muted: true },
      ],
    });
  });

  it('tolerates a missing/garbage payload without throwing (empty forest)', () => {
    expect(normalizeRadarState(undefined).agents).toEqual([]);
    expect(normalizeRadarState(null).agents).toEqual([]);
    expect(normalizeRadarState({ agents: 'nope' }).agents).toEqual([]);
    expect(normalizeRadarState({}).generatedAt).toBe('');
  });

  it('builds a "folder · model" subtitle only when the folder adds info beyond the label', () => {
    // Claude root: label is the task, so the folder + short model is useful context.
    expect(radarSubtitle({ label: 'Polish the M3 radar fixes', cwd: 'WARDEN', model: 'claude-opus-4-8' })).toBe(
      'WARDEN · opus',
    );
    // Codex: label already IS the folder → no redundant subtitle.
    expect(radarSubtitle({ label: 'github project', cwd: 'github project', model: 'openai' })).toBeNull();
    // No folder → no subtitle.
    expect(radarSubtitle({ label: 'x', cwd: null, model: 'claude-haiku-4-5-20251001' })).toBeNull();
    // Folder present but model unknown → folder alone.
    expect(radarSubtitle({ label: 'do a thing', cwd: 'MOBIUS', model: null })).toBe('MOBIUS');
  });

  it('drops only the estimated lens when malformed but keeps exact', () => {
    const model = normalizeRadarState({
      agents: [
        {
          id: 'p',
          harness: 'claude_code',
          status: 'working',
          composition: { exact: { cacheRead: 5, fresh: 6, output: 7 }, estimated: 'broken' },
        },
      ],
    });
    expect(model.agents[0].composition.exact).toEqual({ cacheRead: 5, fresh: 6, output: 7 });
    expect(model.agents[0].composition.estimated).toBeNull();
  });
});
