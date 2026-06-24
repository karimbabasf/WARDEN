import { describe, expect, it } from 'vitest';
import { layoutRadarScene, radarRadius } from './radarLayout';
import type { RadarAgent, RadarSceneModel } from './radarTypes';

function agent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id'>): RadarAgent {
  return {
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: partial.id,
    nickname: null,
    role: null,
    model: null,
    status: 'working',
    contextTokens: 1000,
    maxTokens: 200000,
    fillPct: 0.5,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...partial,
  };
}

function distance(a: { x: number; y: number; z: number }, b: { x: number; y: number; z: number }): number {
  return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
}

function singleTree(): RadarSceneModel {
  return {
    generatedAt: 'T0',
    agents: [
      agent({ id: 'root', depth: 0, parentId: null, contextTokens: 150000, childCount: 2 }),
      agent({ id: 'child-a', depth: 1, parentId: 'root', contextTokens: 8000 }),
      agent({ id: 'child-b', depth: 1, parentId: 'root', contextTokens: 8000 }),
    ],
  };
}

describe('radarRadius', () => {
  it('grows with context occupancy', () => {
    expect(radarRadius(150000, 0)).toBeGreaterThan(radarRadius(1000, 0));
  });

  it('gives a depth-0 root a noticeably larger radius than a depth-1 sub at equal load', () => {
    expect(radarRadius(8000, 0)).toBeGreaterThan(radarRadius(8000, 1));
  });
});

describe('layoutRadarScene', () => {
  it('makes mains noticeably bigger than their subs', () => {
    const layout = layoutRadarScene(singleTree());
    const root = layout.nodes.find((n) => n.id === 'root')!;
    const childA = layout.nodes.find((n) => n.id === 'child-a')!;
    expect(root.radius).toBeGreaterThan(childA.radius);
  });

  it('builds a parent->child link per non-root agent', () => {
    const layout = layoutRadarScene(singleTree());
    expect(layout.links).toEqual(
      expect.arrayContaining([
        { source: 'root', target: 'child-a', kind: 'agent_issue' },
        { source: 'root', target: 'child-b', kind: 'agent_issue' },
      ]),
    );
    expect(layout.links).toHaveLength(2);
  });

  it('orbits children around their own parent (within an orbit band, not on top of it)', () => {
    const layout = layoutRadarScene(singleTree());
    const root = layout.nodes.find((n) => n.id === 'root')!;
    const childA = layout.nodes.find((n) => n.id === 'child-a')!;
    const childB = layout.nodes.find((n) => n.id === 'child-b')!;
    const dA = distance(root.position, childA.position);
    const dB = distance(root.position, childB.position);
    // children sit off the parent (clear gap) but in the same neighbourhood
    expect(dA).toBeGreaterThan(root.radius);
    expect(dA).toBeLessThan(8);
    expect(dB).toBeGreaterThan(root.radius);
    // the two siblings are placed at distinct positions (not stacked)
    expect(distance(childA.position, childB.position)).toBeGreaterThan(0.4);
  });

  it('is deterministic for the same model', () => {
    const a = layoutRadarScene(singleTree());
    const b = layoutRadarScene(singleTree());
    expect(a.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z, n.radius])).toEqual(
      b.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z, n.radius]),
    );
  });

  it('spreads multiple roots apart on a ring (no two roots collide)', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'r1', depth: 0, contextTokens: 50000 }),
        agent({ id: 'r2', depth: 0, contextTokens: 50000 }),
        agent({ id: 'r3', depth: 0, contextTokens: 50000 }),
      ],
    };
    const layout = layoutRadarScene(model);
    const r1 = layout.nodes.find((n) => n.id === 'r1')!;
    const r2 = layout.nodes.find((n) => n.id === 'r2')!;
    const r3 = layout.nodes.find((n) => n.id === 'r3')!;
    expect(distance(r1.position, r2.position)).toBeGreaterThan(r1.radius + r2.radius);
    expect(distance(r1.position, r3.position)).toBeGreaterThan(r1.radius + r3.radius);
    expect(distance(r2.position, r3.position)).toBeGreaterThan(r2.radius + r3.radius);
  });

  it('carries the RadarAgent on each node (for the render + panel)', () => {
    const layout = layoutRadarScene(singleTree());
    const root = layout.nodes.find((n) => n.id === 'root')!;
    expect(root.radarAgent?.id).toBe('root');
    expect(root.radarAgent?.harness).toBe('claude_code');
    expect(root.kind).toBe('hub'); // roots are hubs, subs are issue nodes (reused union)
  });

  it('places depth-2 sub-subagents around their depth-1 parent', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'root', depth: 0, contextTokens: 100000, childCount: 1 }),
        agent({ id: 'mid', depth: 1, parentId: 'root', contextTokens: 20000, childCount: 1 }),
        agent({ id: 'leaf', depth: 2, parentId: 'mid', contextTokens: 5000 }),
      ],
    };
    const layout = layoutRadarScene(model);
    const mid = layout.nodes.find((n) => n.id === 'mid')!;
    const leaf = layout.nodes.find((n) => n.id === 'leaf')!;
    expect(distance(mid.position, leaf.position)).toBeGreaterThan(0);
    expect(distance(mid.position, leaf.position)).toBeLessThan(5);
    expect(layout.links).toEqual(
      expect.arrayContaining([
        { source: 'root', target: 'mid', kind: 'agent_issue' },
        { source: 'mid', target: 'leaf', kind: 'agent_issue' },
      ]),
    );
  });

  it('drops a link whose parent is missing (orphan renders solo, no dangling edge)', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [agent({ id: 'orphan', depth: 1, parentId: 'ghost', contextTokens: 3000 })],
    };
    const layout = layoutRadarScene(model);
    expect(layout.links).toHaveLength(0);
    expect(layout.nodes.find((n) => n.id === 'orphan')).toBeTruthy();
  });
});
