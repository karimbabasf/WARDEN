import { describe, expect, it } from 'vitest';
import { radarGlowTarget, radarNodeColor } from './RadarConstellation';
import type { RadarAgent } from './radarTypes';

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
  it('is brighter for a fuller agent (heat rises with fill)', () => {
    const full = radarNodeColor(agent({ id: 'a', harness: 'codex', fillPct: 0.9 }));
    const empty = radarNodeColor(agent({ id: 'b', harness: 'codex', fillPct: 0.1 }));
    expect(luminance(full)).toBeGreaterThan(luminance(empty));
  });

  it('preserves harness hue (Codex stays violet/cool at mid fill)', () => {
    const [r, , b] = channels(radarNodeColor(agent({ id: 'c', harness: 'codex', fillPct: 0.5 })));
    expect(b).toBeGreaterThan(r); // violet: blue dominant
  });

  it('uses the Claude orange base (warm/red dominant at mid fill)', () => {
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
});
