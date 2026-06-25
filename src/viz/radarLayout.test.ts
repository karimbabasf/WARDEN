import { describe, expect, it } from 'vitest';
import { layoutRadarScene, radarRadius, TILT_Y, TILT_Z } from './radarLayout';
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

// ── folder constellations + multi-shell siblings ───────────────────────────────

function roots(model: RadarSceneModel) {
  const layout = layoutRadarScene(model);
  return { layout, byId: new Map(layout.nodes.map((n) => [n.id, n])) };
}

describe('layoutRadarScene — folder constellations (roots grouped by cwd)', () => {
  // Two roots in the WARDEN folder + one in JB Hunting: same-folder roots form one
  // constellation, different folders are pushed apart on the plane. Harness is no
  // longer the grouping axis — it is carried by COLOUR — so a folder can hold both.
  function twoFolderForest(): RadarSceneModel {
    return {
      generatedAt: 'T',
      agents: [
        agent({ id: 'w1', cwd: 'WARDEN', contextTokens: 40000 }),
        agent({ id: 'w2', cwd: 'WARDEN', contextTokens: 40000 }),
        agent({ id: 'j1', cwd: 'JB Hunting', contextTokens: 40000 }),
      ],
    };
  }

  it('groups same-folder roots together and pushes different folders apart', () => {
    const { byId } = roots(twoFolderForest());
    const w1 = byId.get('w1')!;
    const w2 = byId.get('w2')!;
    const j1 = byId.get('j1')!;
    const sameFolder = distance(w1.position, w2.position); // both in WARDEN
    const crossFolder = distance(w1.position, j1.position); // WARDEN → JB Hunting
    // a same-folder neighbour is closer than a root in another folder.
    expect(sameFolder).toBeLessThan(crossFolder);
  });

  it('exposes one labelled cluster per folder (for the on-screen constellation label)', () => {
    const { layout } = roots(twoFolderForest());
    const labels = layout.clusters.map((c) => c.label).sort();
    expect(labels).toEqual(['JB Hunting', 'WARDEN']);
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

describe('layoutRadarScene — busy constellations claim more room', () => {
  it('gives a folder whose root has many subagents a larger constellation extent than a barren one', () => {
    // `busy-proj` holds one root with 6 subagents; `barren-proj` holds one barren
    // root. The busy constellation's extent (which drives its lateral spacing) must
    // be wider, so a busy orchestrator's moon halo never crowds its neighbours.
    const a: RadarAgent[] = [
      agent({ id: 'busy', cwd: 'busy-proj', depth: 0, contextTokens: 40000, childCount: 6 }),
      agent({ id: 'barren', cwd: 'barren-proj', depth: 0, contextTokens: 40000, childCount: 0 }),
    ];
    for (let i = 0; i < 6; i++)
      a.push(agent({ id: `busy-kid-${i}`, depth: 1, parentId: 'busy', contextTokens: 5000 }));
    const { layout } = roots({ generatedAt: 'T', agents: a });
    const busyC = layout.clusters.find((c) => c.label === 'busy-proj')!;
    const barrenC = layout.clusters.find((c) => c.label === 'barren-proj')!;
    expect(busyC.radius).toBeGreaterThan(barrenC.radius);
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

    // M-1 strengthening: the radial-spread discriminator must be present in this
    // test so it FAILS on the old single-ring layout (spread ≈ STAGGER_SPAN ≈ 0.18)
    // and PASSES on the multi-shell layout (spread ≥ SHELL_STEP ≈ 1.15 > 0.8).
    const dists = kids.map((k) => Math.hypot(
      k.position.x - hub.position.x,
      k.position.y - hub.position.y,
      k.position.z - hub.position.z,
    ));
    expect(Math.max(...dists) - Math.min(...dists)).toBeGreaterThan(0.8);

    // Recover the orbit radius for each child using the exact inverse of ringPosition:
    //   ringPosition sets x = cx + cos(a)·R,  y = cy + sin(a)·R·TILT_Y
    //   so  R = hypot(dx, dy/TILT_Y)  — exact, no approximation.
    // Bin by orbit/0.5 (0.5 < SHELL_STEP gap of ~0.97) so shells never share a bin.
    type ShellEntry = { polar: number };
    const shellOf = new Map<number, ShellEntry[]>();
    for (const k of kids) {
      const dx = k.position.x - hub.position.x;
      const dy = k.position.y - hub.position.y;
      const dz = k.position.z - hub.position.z;
      const orbit = Math.hypot(dx, dy / TILT_Y);
      const key = Math.round(orbit / 0.5);
      // polar angle — same formula as the updated bearing() helper
      const polar = Math.atan2(dz / TILT_Z, dx);
      const list = shellOf.get(key) ?? [];
      list.push({ polar });
      shellOf.set(key, list);
    }
    // at least two shells exist (18 children overflow the ~12-capacity inner ring)
    expect(shellOf.size).toBeGreaterThanOrEqual(2);
    // within every shell, the closest pair of siblings clears MIN_SIBLING_GAP in the
    // polar (layout) plane — the layout's actual guarantee is 0.52 rad per shell,
    // not a compressed 3D angle. Threshold 0.5 rad < 0.52 rad nominal, giving 4%
    // tolerance for floating-point and stagger while still failing on old single-ring
    // code (18 nodes at 2π/18 ≈ 0.35 rad < 0.5 rad).
    for (const shell of shellOf.values()) {
      if (shell.length < 2) continue;
      let minGap = Infinity;
      for (let i = 0; i < shell.length; i++) {
        for (let j = i + 1; j < shell.length; j++) {
          let gap = Math.abs(shell[i].polar - shell[j].polar) % (2 * Math.PI);
          if (gap > Math.PI) gap = 2 * Math.PI - gap;
          minGap = Math.min(minGap, gap);
        }
      }
      expect(minGap).toBeGreaterThan(0.5);
    }
  });

  it('a small sibling set still fits on a single shell (no premature splitting)', () => {
    const { layout, byId } = roots(fanout(3));
    const hub = byId.get('hub')!;
    const kids = layout.nodes.filter((n) => n.radarAgent!.parentId === 'hub');
    // Recover the orbit radius for each child (exact inverse of ringPosition):
    //   R = hypot(dx, dy / TILT_Y)
    // Three children should share essentially one orbit (only the tiny per-child
    // stagger separates them radially — bounded by STAGGER_SPAN = 0.18). They must
    // NOT be pushed onto a far second shell (SHELL_STEP ≈ 1.15).
    const orbits = kids.map((k) => Math.hypot(
      k.position.x - hub.position.x,
      (k.position.y - hub.position.y) / TILT_Y,
    ));
    expect(Math.max(...orbits) - Math.min(...orbits)).toBeLessThan(0.8);
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
