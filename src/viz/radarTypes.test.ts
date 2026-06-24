import { describe, expect, it } from 'vitest';
import { normalizeRadarState } from './radarTypes';

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
    expect(a.composition.exact).toEqual({ cacheRead: 90000, fresh: 12000, output: 2620 });
    expect(a.composition.estimated).toEqual({ preamble: 7000, conversation: 3000, toolOutput: 1500, thinking: 200 });
    expect(a.recentActivity[0]).toEqual({ ts: '2026-06-23T22:50:00Z', kind: 'tool', label: 'Read' });
    expect(a.estCostUsd).toBeCloseTo(0.42);
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

  it('tolerates a missing/garbage payload without throwing (empty forest)', () => {
    expect(normalizeRadarState(undefined).agents).toEqual([]);
    expect(normalizeRadarState(null).agents).toEqual([]);
    expect(normalizeRadarState({ agents: 'nope' }).agents).toEqual([]);
    expect(normalizeRadarState({}).generatedAt).toBe('');
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
