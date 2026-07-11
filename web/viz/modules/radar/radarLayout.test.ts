import { describe, expect, it } from 'vitest';
import { layoutRadarScene, radarRadius } from './radarLayout';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';

function agent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id'>): RadarAgent {
  return {
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: partial.id,
    nickname: null,
    cwd: null,
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

  it('hangs children below their parent as distinct beads (not stacked)', () => {
    const layout = layoutRadarScene(singleTree());
    const root = layout.nodes.find((n) => n.id === 'root')!;
    const childA = layout.nodes.find((n) => n.id === 'child-a')!;
    const childB = layout.nodes.find((n) => n.id === 'child-b')!;
    // children sit one row below the parent on the board plane
    expect(childA.position.y).toBeLessThan(root.position.y);
    expect(childB.position.y).toBeLessThan(root.position.y);
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

  it('spaces root beads apart on their rail (no two roots collide)', () => {
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

  it('hangs depth-2 sub-subagents below their depth-1 parent, linked down the tree', () => {
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
    expect(leaf.position.y).toBeLessThan(mid.position.y);
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

// ── folder constellations + multi-shell siblings ───────────────────────────────

function roots(model: RadarSceneModel) {
  const layout = layoutRadarScene(model);
  return { layout, byId: new Map(layout.nodes.map((n) => [n.id, n])) };
}

describe('layoutRadarScene — folder constellations (roots grouped by cwd)', () => {
  // Two roots in the WARDEN folder + one in Payments: same-folder roots form one
  // constellation, different folders are pushed apart on the plane. Harness is no
  // longer the grouping axis — it is carried by COLOUR — so a folder can hold both.
  function twoFolderForest(): RadarSceneModel {
    return {
      generatedAt: 'T',
      agents: [
        agent({ id: 'w1', cwd: 'WARDEN', contextTokens: 40000 }),
        agent({ id: 'w2', cwd: 'WARDEN', contextTokens: 40000 }),
        agent({ id: 'j1', cwd: 'Payments', contextTokens: 40000 }),
      ],
    };
  }

  it('puts same-folder roots on one rail and different folders on different rails', () => {
    const { byId } = roots(twoFolderForest());
    const w1 = byId.get('w1')!;
    const w2 = byId.get('w2')!;
    const j1 = byId.get('j1')!;
    // both WARDEN roots share the WARDEN rail (same y).
    expect(w1.position.y).toBe(w2.position.y);
    // the Payments root is on its own rail (a different y).
    expect(j1.position.y).not.toBe(w1.position.y);
  });

  it('exposes one labelled cluster per folder (for the on-screen constellation label)', () => {
    const { layout } = roots(twoFolderForest());
    const labels = layout.clusters.map((c) => c.label).sort();
    expect(labels).toEqual(['Payments', 'WARDEN']);
    for (const c of layout.clusters) {
      expect(Number.isFinite(c.center.x)).toBe(true);
      expect(Number.isFinite(c.center.y)).toBe(true);
      expect(Number.isFinite(c.center.z)).toBe(true);
      expect(c.radius).toBeGreaterThan(0);
    }
  });

  it('keeps a folder driven by BOTH harnesses in one constellation (colour splits them, not position)', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'cl', harness: 'claude_code', cwd: 'WARDEN', contextTokens: 30000 }),
        agent({ id: 'cx', harness: 'codex', origin: 'Codex Desktop', cwd: 'WARDEN', contextTokens: 30000 }),
      ],
    };
    const { layout } = roots(model);
    const warden = layout.clusters.filter((c) => c.label === 'WARDEN');
    expect(warden).toHaveLength(1); // one WARDEN constellation, two hues
  });

  it('is deterministic with a multi-folder forest (nodes AND clusters)', () => {
    const a = layoutRadarScene(twoFolderForest());
    const b = layoutRadarScene(twoFolderForest());
    expect(a.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z])).toEqual(
      b.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z]),
    );
    expect(a.clusters.map((c) => [c.key, c.center.x, c.radius])).toEqual(
      b.clusters.map((c) => [c.key, c.center.x, c.radius]),
    );
  });
});

describe('layoutRadarScene — flat-agent honesty on the board (no fabricated hierarchy)', () => {
  it('promotes every stray under a flat (codex_vscode) root to its own root bead, no links', () => {
    const a: RadarAgent[] = [
      agent({ id: 'vsc', harness: 'codex', origin: 'codex_vscode', depth: 0, childCount: 0 }),
    ];
    for (let i = 0; i < 12; i++)
      a.push(agent({ id: `stray-${i}`, harness: 'codex', parentId: 'vsc', depth: 1, contextTokens: 4000 }));
    const layout = layoutRadarScene({ generatedAt: 'T', agents: a });
    // no fabricated links under the flat parent …
    expect(layout.links).toHaveLength(0);
    // … and every stray still renders as its own node on the board plane, promoted
    // to its own rail head (a moon hung under `vsc` would instead sit one ROW_STEP
    // below it and share its x; a promoted root heads a distinct rail).
    const vsc = layout.nodes.find((n) => n.id === 'vsc')!;
    for (let i = 0; i < 12; i++) {
      const s = layout.nodes.find((n) => n.id === `stray-${i}`)!;
      expect(s).toBeTruthy();
      expect(s.position.z).toBe(0);
      expect(s.position.y).not.toBe(vsc.position.y - 1.5); // never a moon of vsc
    }
  });
});

function twoFolders(): RadarSceneModel {
  return {
    generatedAt: 'T0',
    agents: [
      // folder A (cwd "alpha"): two roots, one with a child + grandchild
      agent({ id: 'a1', depth: 0, parentId: null, cwd: 'alpha', contextTokens: 120000, childCount: 1 }),
      agent({ id: 'a1-c', depth: 1, parentId: 'a1', cwd: 'alpha', contextTokens: 8000, childCount: 1 }),
      agent({ id: 'a1-gc', depth: 2, parentId: 'a1-c', cwd: 'alpha', contextTokens: 3000 }),
      agent({ id: 'a2', depth: 0, parentId: null, cwd: 'alpha', contextTokens: 40000 }),
      // folder B (cwd "beta"): one root
      agent({ id: 'b1', depth: 0, parentId: null, cwd: 'beta', contextTokens: 60000 }),
    ],
  };
}

describe('layoutRadarScene abacus board', () => {
  it('places every node on the board plane (z = 0)', () => {
    const layout = layoutRadarScene(twoFolders());
    for (const n of layout.nodes) expect(n.position.z).toBe(0);
  });

  it('gives each folder its own rail (distinct y), ordered top to bottom', () => {
    const layout = layoutRadarScene(twoFolders());
    const railYalpha = layout.nodes.find((n) => n.id === 'a1')!.position.y;
    const railYbeta = layout.nodes.find((n) => n.id === 'b1')!.position.y;
    expect(railYalpha).not.toBe(railYbeta);
    // folder "alpha" sorts before "beta", so alpha is the top rail (greater y)
    expect(railYalpha).toBeGreaterThan(railYbeta);
    // both roots of "alpha" share the alpha rail y
    expect(layout.nodes.find((n) => n.id === 'a2')!.position.y).toBe(railYalpha);
  });

  it('orders root beads left to right on their rail', () => {
    const layout = layoutRadarScene(twoFolders());
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    const a2 = layout.nodes.find((n) => n.id === 'a2')!;
    expect(a1.position.x).toBeLessThan(a2.position.x);
  });

  it('hangs subagents one row-step below their parent per depth level', () => {
    const layout = layoutRadarScene(twoFolders());
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    const c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const gc = layout.nodes.find((n) => n.id === 'a1-gc')!;
    expect(c.position.y).toBeLessThan(a1.position.y);
    expect(gc.position.y).toBeLessThan(c.position.y);
    // equal steps per level
    expect(a1.position.y - c.position.y).toBeCloseTo(c.position.y - gc.position.y, 5);
  });

  it('centres a single child under its parent', () => {
    const layout = layoutRadarScene(twoFolders());
    const c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    expect(c.position.x).toBeCloseTo(a1.position.x, 5);
  });

  it('centres multiple children on the mean of their parent x', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T0',
      agents: [
        agent({ id: 'p', depth: 0, parentId: null, cwd: 'alpha', childCount: 2 }),
        agent({ id: 'p-c1', depth: 1, parentId: 'p', cwd: 'alpha' }),
        agent({ id: 'p-c2', depth: 1, parentId: 'p', cwd: 'alpha' }),
      ],
    };
    const layout = layoutRadarScene(model);
    const p = layout.nodes.find((n) => n.id === 'p')!;
    const c1 = layout.nodes.find((n) => n.id === 'p-c1')!;
    const c2 = layout.nodes.find((n) => n.id === 'p-c2')!;
    expect((c1.position.x + c2.position.x) / 2).toBeCloseTo(p.position.x, 5);
    expect(c1.position.x).not.toBeCloseTo(c2.position.x, 1); // spread apart
  });

  it('spaces a root with a wide subtree clear of the next root bead', () => {
    // a1 has a subtree; a2 is a bare root. a2 must sit right of a1's subtree.
    const layout = layoutRadarScene(twoFolders());
    const a1c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const a1gc = layout.nodes.find((n) => n.id === 'a1-gc')!;
    const a2 = layout.nodes.find((n) => n.id === 'a2')!;
    const subtreeRight = Math.max(a1c.position.x, a1gc.position.x);
    expect(a2.position.x).toBeGreaterThan(subtreeRight);
  });

  it('leaves more vertical room below a rail that has a deep subtree', () => {
    // rail "alpha" has depth-2; rail "beta" is flat. Measure the gap under each.
    const model: RadarSceneModel = {
      generatedAt: 'T0',
      agents: [
        agent({ id: 'a', depth: 0, parentId: null, cwd: 'alpha', childCount: 1 }),
        agent({ id: 'a-c', depth: 1, parentId: 'a', cwd: 'alpha', childCount: 1 }),
        agent({ id: 'a-gc', depth: 2, parentId: 'a-c', cwd: 'alpha' }),
        agent({ id: 'b', depth: 0, parentId: null, cwd: 'beta' }),
        agent({ id: 'c', depth: 0, parentId: null, cwd: 'gamma' }),
      ],
    };
    const layout = layoutRadarScene(model);
    const yAlpha = layout.nodes.find((n) => n.id === 'a')!.position.y;
    const yBeta = layout.nodes.find((n) => n.id === 'b')!.position.y;
    const yGamma = layout.nodes.find((n) => n.id === 'c')!.position.y;
    const gapUnderAlpha = yAlpha - yBeta; // alpha has depth 2
    const gapUnderBeta = yBeta - yGamma; // beta is flat
    expect(gapUnderAlpha).toBeGreaterThan(gapUnderBeta);
  });
});

describe('layoutRadarScene — frozen output contract (every node carries id + position + radius)', () => {
  it('emits id, finite position and radius on every node (camera framing depends on it)', () => {
    const { layout } = roots({
      generatedAt: 'T',
      agents: [
        agent({ id: 'root', depth: 0, contextTokens: 80000, childCount: 2 }),
        agent({ id: 'a', depth: 1, parentId: 'root', contextTokens: 5000 }),
        agent({ id: 'b', depth: 1, parentId: 'root', contextTokens: 5000 }),
      ],
    });
    for (const n of layout.nodes) {
      expect(typeof n.id).toBe('string');
      expect(n.id.length).toBeGreaterThan(0);
      expect(Number.isFinite(n.position.x)).toBe(true);
      expect(Number.isFinite(n.position.y)).toBe(true);
      expect(Number.isFinite(n.position.z)).toBe(true);
      expect(Number.isFinite(n.radius)).toBe(true);
      expect(n.radius).toBeGreaterThan(0);
    }
    // an id→{pos,radius} map is derivable (Task 9 camera framing)
    const map = new Map(layout.nodes.map((n) => [n.id, { pos: n.position, radius: n.radius }]));
    expect(map.get('root')).toBeTruthy();
    expect(map.size).toBe(layout.nodes.length);
  });
});
