import { describe, expect, it } from 'vitest';
import {
  radarGlowTarget,
  radarLivenessColorScale,
  radarLinkFadeFactor,
  radarModelWithoutGone,
  radarNodeColor,
} from './RadarConstellation';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';

function luminance(hex: string): number {
  const n = parseInt(/^#?([0-9a-f]{6})$/i.exec(hex.trim())![1], 16);
  const r = (n >> 16) & 0xff;
  const g = (n >> 8) & 0xff;
  const b = n & 0xff;
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}
function channels(hex: string): [number, number, number] {
  const n = parseInt(/^#?([0-9a-f]{6})$/i.exec(hex.trim())![1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

function agent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id' | 'harness' | 'fillPct'>): RadarAgent {
  return {
    origin: null,
    parentId: null,
    depth: 0,
    label: partial.id,
    nickname: null,
    cwd: null,
    role: null,
    model: null,
    status: 'working',
    contextTokens: 0,
    maxTokens: 0,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...partial,
  };
}

describe('radarNodeColor', () => {
  it('is FLAT — the hue does not change with fill (size/brightness carry the signals)', () => {
    // Colour is identity ONLY: fill drives SIZE and liveness drives BRIGHTNESS, so a
    // near-empty and a near-full agent of the same harness share an identical hue.
    const full = radarNodeColor(agent({ id: 'a', harness: 'codex', fillPct: 0.9 }));
    const empty = radarNodeColor(agent({ id: 'b', harness: 'codex', fillPct: 0.1 }));
    expect(full).toBe(empty);
    expect(luminance(full)).toBe(luminance(empty));
  });

  it('is the Codex cyan-ice hue (cool/blue dominant)', () => {
    const [r, , b] = channels(radarNodeColor(agent({ id: 'c', harness: 'codex', fillPct: 0.5 })));
    expect(b).toBeGreaterThan(r); // cyan-ice: blue dominant
  });

  it('is the Claude tangerine hue (warm/red dominant)', () => {
    const [r, , b] = channels(radarNodeColor(agent({ id: 'd', harness: 'claude_code', fillPct: 0.5 })));
    expect(r).toBeGreaterThan(b);
  });

  it('falls back to the neutral hue for an unknown harness', () => {
    const c = radarNodeColor(agent({ id: 'e', harness: 'gemini', fillPct: 0.5 }));
    // neutral is neither the Claude nor Codex base — just assert it is a valid hex
    expect(/^#[0-9a-f]{6}$/i.test(c)).toBe(true);
  });

  it('makes working agents visibly brighter than idle agents', () => {
    const working = radarGlowTarget({
      agent: agent({ id: 'working', harness: 'codex', fillPct: 0.45, status: 'working' }),
      isRoot: true,
      emphasis: false,
      selected: false,
      hovered: false,
    });
    const idle = radarGlowTarget({
      agent: agent({ id: 'idle', harness: 'codex', fillPct: 0.45, status: 'idle' }),
      isRoot: true,
      emphasis: false,
      selected: false,
      hovered: false,
    });

    expect(working).toBeGreaterThan(idle * 5);
  });

  it('keeps idle roots and subagents as quiet background embers', () => {
    const idleRoot = radarGlowTarget({
      agent: agent({ id: 'idle-root', harness: 'codex', fillPct: 0.45, status: 'idle' }),
      isRoot: true,
      emphasis: false,
      selected: false,
      hovered: false,
    });
    const idleSubagent = radarGlowTarget({
      agent: agent({ id: 'idle-sub', harness: 'codex', fillPct: 0.45, status: 'idle' }),
      isRoot: false,
      emphasis: false,
      selected: false,
      hovered: false,
    });
    const workingRoot = radarGlowTarget({
      agent: agent({ id: 'working-root', harness: 'codex', fillPct: 0.45, status: 'working' }),
      isRoot: true,
      emphasis: false,
      selected: false,
      hovered: false,
    });

    expect(idleRoot).toBeCloseTo(0.22);
    expect(idleSubagent).toBeCloseTo(0.16);
    expect(workingRoot).toBeGreaterThan(idleRoot * 12);
  });

  it('damps idle material colour and restores full colour as an agent wakes up', () => {
    expect(radarLivenessColorScale(0)).toBeLessThan(0.7);
    expect(radarLivenessColorScale(0.5)).toBeGreaterThan(radarLivenessColorScale(0));
    expect(radarLivenessColorScale(1)).toBe(1);
  });
});

describe('radarModelWithoutGone', () => {
  it('removes locally gone nodes before layout while preserving the original model', () => {
    const model = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'root', harness: 'claude_code', fillPct: 0.4 }),
        agent({ id: 'sub', harness: 'claude_code', fillPct: 0.2, parentId: 'root', depth: 1, status: 'terminated' }),
      ],
    };

    const filtered = radarModelWithoutGone(model, new Set(['sub']));

    expect(filtered.agents.map((a) => a.id)).toEqual(['root']);
    expect(model.agents.map((a) => a.id)).toEqual(['root', 'sub']);
  });
});

describe('radarLinkFadeFactor', () => {
  it('keeps a live link at full brightness when both endpoints have no lifecycle entry yet', () => {
    expect(radarLinkFadeFactor({}, {})).toBe(1);
  });

  it('fades with the dimmest imploding endpoint using a softened scale curve', () => {
    const factor = radarLinkFadeFactor(
      { entry: { phase: 'alive', t: 1, scale: 1 } },
      { entry: { phase: 'imploding', t: 0.2, scale: 0.5 } },
    );

    expect(factor).toBeLessThan(0.5);
    expect(factor).toBeCloseTo(Math.pow(0.5, 1.4), 5);
  });

  it('drops immediately to zero when either endpoint was pruned as gone', () => {
    expect(radarLinkFadeFactor({ gone: true }, { entry: { phase: 'alive', t: 1, scale: 1 } })).toBe(0);
    expect(radarLinkFadeFactor({ entry: { phase: 'alive', t: 1, scale: 1 } }, { gone: true })).toBe(0);
  });
});
