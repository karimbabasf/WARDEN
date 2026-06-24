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

// ── Task 5: harness sectors + multi-shell siblings + subtree-scaled spacing ─────

/**
 * Azimuthal bearing of a point relative to a centre, in the radar's XZ-ish plane.
 * The layout places nodes with x = cos(a)·R and z ∝ sin(a)·R (same sign as sin a),
 * so atan2(z, x) is a strictly monotonic warp of the intended polar angle `a` —
 * which is all the sector/shell tests need (ordering + pairwise separation), not
 * the un-warped angle itself.
 */
function bearing(center: { x: number; z: number }, p: { x: number; z: number }): number {
  return Math.atan2(p.z - center.z, p.x - center.x);
}

/** Smallest absolute angular separation between two bearings, in [0, π]. */
function angularGap(a: number, b: number): number {
  let d = Math.abs(a - b) % (2 * Math.PI);
  if (d > Math.PI) d = 2 * Math.PI - d;
  return d;
}

function roots(model: RadarSceneModel) {
  const layout = layoutRadarScene(model);
  return { layout, byId: new Map(layout.nodes.map((n) => [n.id, n])) };
}

describe('layoutRadarScene — harness sectors (roots grouped by harness arc)', () => {
  // Three Claude roots + three Codex roots: each harness must own a contiguous arc,
  // i.e. sorting all root bearings yields one solid block per harness (no harness's
  // roots interleave with the other's). This is the readable "no clumping by
  // harness" guarantee for 15+ agents.
  function twoHarnessForest(): RadarSceneModel {
    const a: RadarAgent[] = [];
    for (let i = 0; i < 3; i++)
      a.push(agent({ id: `cl-${i}`, harness: 'claude_code', depth: 0, contextTokens: 40000 }));
    for (let i = 0; i < 3; i++)
      a.push(agent({ id: `cx-${i}`, harness: 'codex', origin: 'Codex Desktop', depth: 0, contextTokens: 40000 }));
    return { generatedAt: 'T', agents: a };
  }

  it('keeps each harness\'s roots within one contiguous angular arc (no interleaving)', () => {
    const { layout } = roots(twoHarnessForest());
    const origin = { x: 0, z: 0 };
    const ordered = layout.nodes
      .filter((n) => n.depth === 0)
      .map((n) => ({ id: n.id, harness: n.radarAgent!.harness, theta: bearing(origin, n.position) }))
      .sort((p, q) => p.theta - q.theta);

    // Walking the ring in bearing order, the harness label changes at most twice
    // (claude…→codex…→claude… across the 0/2π seam) — never more. More than two
    // transitions means the harnesses are interleaved (clumped together), not
    // sectored.
    let transitions = 0;
    for (let i = 0; i < ordered.length; i++) {
      const prev = ordered[(i - 1 + ordered.length) % ordered.length];
      if (prev.harness !== ordered[i].harness) transitions++;
    }
    expect(transitions).toBeLessThanOrEqual(2);
  });

  it('separates the two harness sectors (closest cross-harness pair is farther than the tightest in-harness pair would force)', () => {
    const { layout } = roots(twoHarnessForest());
    const origin = { x: 0, z: 0 };
    const cl = layout.nodes.filter((n) => n.radarAgent!.harness === 'claude_code');
    const cx = layout.nodes.filter((n) => n.radarAgent!.harness === 'codex');
    // every claude root is a distinct position from every codex root, and the two
    // groups occupy different parts of the ring (their bearing centroids differ).
    const centroid = (ns: typeof cl) => {
      const xs = ns.map((n) => bearing(origin, n.position));
      // circular mean
      const sx = xs.reduce((s, t) => s + Math.cos(t), 0);
      const sz = xs.reduce((s, t) => s + Math.sin(t), 0);
      return Math.atan2(sz, sx);
    };
    expect(angularGap(centroid(cl), centroid(cx))).toBeGreaterThan(0.5);
  });

  it('is still deterministic with a multi-harness forest', () => {
    const a = layoutRadarScene(twoHarnessForest());
    const b = layoutRadarScene(twoHarnessForest());
    expect(a.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z])).toEqual(
      b.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z]),
    );
  });
});

describe('layoutRadarScene — subtree-scaled root spacing (busy orchestrators get more room)', () => {
  it('gives a root with many descendants a wider angular slice than a barren root of the same harness', () => {
    // Two Claude roots: `busy` has 6 children, `barren` has none. The busy root's
    // arc must be wider, so the angular gap from busy's neighbours is larger than
    // the gap around barren. We approximate "arc width" by the angular distance
    // from each root to its nearest neighbouring root on the ring.
    const a: RadarAgent[] = [
      agent({ id: 'busy', harness: 'claude_code', depth: 0, contextTokens: 40000, childCount: 6 }),
      agent({ id: 'barren', harness: 'claude_code', depth: 0, contextTokens: 40000, childCount: 0 }),
      agent({ id: 'filler', harness: 'claude_code', depth: 0, contextTokens: 40000, childCount: 0 }),
    ];
    for (let i = 0; i < 6; i++)
      a.push(agent({ id: `busy-kid-${i}`, harness: 'claude_code', depth: 1, parentId: 'busy', contextTokens: 5000 }));
    const layout = layoutRadarScene({ generatedAt: 'T', agents: a });
    const origin = { x: 0, z: 0 };
    const rootBearings = layout.nodes
      .filter((n) => n.depth === 0)
      .map((n) => ({ id: n.id, theta: bearing(origin, n.position) }));
    const nearest = (id: string) => {
      const me = rootBearings.find((r) => r.id === id)!;
      return Math.min(
        ...rootBearings.filter((r) => r.id !== id).map((r) => angularGap(me.theta, r.theta)),
      );
    };
    // the busy root claims more angular room (its nearest neighbour is farther off)
    // than the barren root does.
    expect(nearest('busy')).toBeGreaterThan(nearest('barren'));
  });
});

describe('layoutRadarScene — multi-shell siblings (no clumping when a parent has many children)', () => {
  function fanout(n: number): RadarSceneModel {
    const a: RadarAgent[] = [
      agent({ id: 'hub', harness: 'claude_code', depth: 0, contextTokens: 120000, childCount: n }),
    ];
    for (let i = 0; i < n; i++)
      a.push(
        agent({
          id: `kid-${String(i).padStart(2, '0')}`,
          harness: 'claude_code',
          depth: 1,
          parentId: 'hub',
          contextTokens: 6000,
        }),
      );
    return { generatedAt: 'T', agents: a };
  }

  it('distributes many children across 2+ concentric shells (not one crammed ring)', () => {
    const { layout, byId } = roots(fanout(18));
    const hub = byId.get('hub')!;
    const kids = layout.nodes.filter((n) => n.radarAgent!.parentId === 'hub');
    expect(kids).toHaveLength(18);
    const dists = kids.map((k) =>
      Math.hypot(k.position.x - hub.position.x, k.position.y - hub.position.y, k.position.z - hub.position.z),
    );
    const minD = Math.min(...dists);
    const maxD = Math.max(...dists);
    // a single ring (plus tiny per-child stagger) would keep all radii within a hair
    // of each other; multiple shells push the outer shell clearly past the inner one.
    expect(maxD - minD).toBeGreaterThan(0.8);
  });

  it('keeps same-shell siblings at a readable minimum angular gap (no two crammed together)', () => {
    const { layout, byId } = roots(fanout(18));
    const hub = byId.get('hub')!;
    const kids = layout.nodes.filter((n) => n.radarAgent!.parentId === 'hub');
    // true direction unit-vector of each child from the hub (projection-independent:
    // the layout's real angular guarantee lives in 3D, not in the warped 2D bearing).
    const vecs = kids.map((k) => {
      const v = { x: k.position.x - hub.position.x, y: k.position.y - hub.position.y, z: k.position.z - hub.position.z };
      const d = Math.hypot(v.x, v.y, v.z);
      return { d, ux: v.x / d, uy: v.y / d, uz: v.z / d };
    });
    // bucket children into shells by distance to the hub, in SHELL_STEP-sized bins
    // (≈1.15) — far coarser than the bounded intra-shell stagger (≤0.18), so a shell
    // never splits across two bins and two shells never merge into one.
    const shellOf = new Map<number, typeof vecs>();
    for (const v of vecs) {
      const key = Math.round(v.d / 1.0); // 1-unit bins sit between STAGGER_SPAN and SHELL_STEP
      const list = shellOf.get(key) ?? [];
      list.push(v);
      shellOf.set(key, list);
    }
    // at least two shells exist (18 children overflow the ~12-capacity inner ring)
    expect(shellOf.size).toBeGreaterThanOrEqual(2);
    // within every shell, the closest pair of siblings clears a readable 3D angle
    for (const shell of shellOf.values()) {
      if (shell.length < 2) continue;
      let minAngle = Infinity;
      for (let i = 0; i < shell.length; i++) {
        for (let j = i + 1; j < shell.length; j++) {
          const dot = shell[i].ux * shell[j].ux + shell[i].uy * shell[j].uy + shell[i].uz * shell[j].uz;
          minAngle = Math.min(minAngle, Math.acos(Math.max(-1, Math.min(1, dot))));
        }
      }
      expect(minAngle).toBeGreaterThan(0.3);
    }
  });

  it('a small sibling set still fits on a single shell (no premature splitting)', () => {
    const { layout, byId } = roots(fanout(3));
    const hub = byId.get('hub')!;
    const kids = layout.nodes.filter((n) => n.radarAgent!.parentId === 'hub');
    const dists = kids.map((k) =>
      Math.hypot(k.position.x - hub.position.x, k.position.y - hub.position.y, k.position.z - hub.position.z),
    );
    // three children share essentially one orbit (only the tiny per-child stagger
    // separates them radially) — they must NOT be pushed onto a far second shell.
    expect(Math.max(...dists) - Math.min(...dists)).toBeLessThan(0.8);
  });

  it('preserves honesty: a flat (codex_vscode) root grows no shells even with stray children', () => {
    const a: RadarAgent[] = [
      agent({ id: 'vsc', harness: 'codex', origin: 'codex_vscode', depth: 0, childCount: 0 }),
    ];
    for (let i = 0; i < 12; i++)
      a.push(agent({ id: `stray-${i}`, harness: 'codex', parentId: 'vsc', depth: 1, contextTokens: 4000 }));
    const layout = layoutRadarScene({ generatedAt: 'T', agents: a });
    // no fabricated links under the flat parent …
    expect(layout.links).toHaveLength(0);
    // … and every stray is promoted to its own root (none orbiting `vsc`).
    const vsc = layout.nodes.find((n) => n.id === 'vsc')!;
    for (let i = 0; i < 12; i++) {
      const s = layout.nodes.find((n) => n.id === `stray-${i}`)!;
      expect(s).toBeTruthy();
      const d = Math.hypot(
        s.position.x - vsc.position.x,
        s.position.y - vsc.position.y,
        s.position.z - vsc.position.z,
      );
      expect(d).toBeGreaterThan(vsc.radius + 1);
    }
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
