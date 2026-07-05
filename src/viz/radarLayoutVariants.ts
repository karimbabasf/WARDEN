// radarLayoutVariants.ts — candidate RADAR geometries for the "stop the line-up"
// redesign, kept OUT of the production layout until one wins the visual compare.
//
// The shipped `layoutRadarScene` places every node on a single TILTED DISC
// (`ringPosition`: y and z are both driven by the same sin(angle), so every point
// satisfies z ≈ 0.577·y — one plane). A flat plane read edge-on projects to a
// LINE, and the free-orbit camera passes through that edge-on azimuth every
// rotation → "the globes all line up in a straight path." Spreading clusters as a
// field (galaxy packing) fixed the left-to-right ROW but not the edge-on collapse,
// and it does nothing for the common case (many subagents under ONE orchestrator in
// ONE folder), because the subagents are placed on flat tilted rings around their
// parent.
//
// The fix here is DIMENSIONAL: distribute nodes on a FIBONACCI SPHERE instead of a
// ring. A Fibonacci sphere spreads K points with near-even angular spacing and — the
// whole point — has NO thin axis, so no camera angle ever collapses it to a line.
// `depthScale` tunes the third (camera-depth) axis so we can render the full
// spectrum and compare: 1.0 = a true isotropic sphere, ~0.5 = a thick lens (more
// screen-plane spread, shallower occlusion, but reintroduces a bad ~90° angle), 0 =
// back to a disc. Pure + deterministic (no RNG), same contract as the shipped layout.

import type { LayoutNode, OrbLink, Vec3 } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { radarHarness } from './radarTheme';
import { isFlatAgent, radarRadius, type RadarCluster, type RadarLayout } from './radarLayout';

// The golden angle — successive Fibonacci points step this far in longitude, which
// is what makes the lattice even (no two points share a meridian, no banding).
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5)); // ≈ 2.399963 rad

/**
 * Unit direction of point `i` of `k` on a Fibonacci sphere, with the depth (z) axis
 * scaled by `depthScale` (1 = sphere, <1 = lens flattened toward the screen plane).
 * `phase` rotates the whole lattice in longitude so sibling families under different
 * parents don't all present the same face. Returns the origin for k ≤ 1 (a lone node
 * sits at its own centre; the caller handles that).
 */
export function fibDirection(i: number, k: number, depthScale: number, phase = 0): Vec3 {
  if (k <= 1) return { x: 0, y: 0, z: 0 };
  const y = 1 - (i / (k - 1)) * 2; // walk the poles: +1 (top) → −1 (bottom)
  const r = Math.sqrt(Math.max(0, 1 - y * y)); // ring radius at this latitude
  const theta = i * GOLDEN_ANGLE + phase;
  return { x: Math.cos(theta) * r, y, z: Math.sin(theta) * r * depthScale };
}

// Empirical nearest-neighbour angular spacing on a Fibonacci sphere of K points
// (radians). Used to size a shell so neighbours never crowd below a target chord.
function neighborAngle(k: number): number {
  return 3.1 / Math.sqrt(Math.max(1, k));
}

/**
 * Radius of a shell that holds `count` items each reaching `itemReach`, so adjacent
 * items keep at least `gap` of clear space between their surfaces. Grows ∝ √count, so
 * a parent with 25 subagents opens a wider shell automatically — no crowding, ever.
 * Never smaller than `minBase` (a small family still sits a comfortable distance out).
 */
export function shellRadius(count: number, itemReach: number, gap: number, minBase: number): number {
  if (count <= 1) return minBase;
  const chord = 2 * itemReach + gap; // surface-to-surface clearance we want as a chord
  // chord ≈ R · Δθ for small Δθ → R = chord / Δθ.
  const needed = chord / neighborAngle(count);
  return Math.max(minBase, needed);
}

// Child depth → tighter hug for deeper moons (mirrors the shipped orbitRadius shrink).
function depthShrink(childDepth: number): number {
  return Math.max(0.5, 1 - (childDepth - 1) * 0.24);
}

// Deterministic longitude phase from an id, so each parent's moon lattice is rotated
// to a stable, distinct face (no RNG — layout must reproduce exactly).
function phaseSeed(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 997;
  return (h / 997) * Math.PI * 2;
}

function makeNode(agent: RadarAgent, position: Vec3): LayoutNode {
  return {
    id: agent.id,
    kind: agent.depth === 0 ? 'hub' : 'issue',
    position,
    radius: radarRadius(agent.contextTokens, agent.depth),
    agentId: agent.id,
    harness: agent.harness,
    radarAgent: agent,
    depth: agent.depth,
  };
}

const add = (a: Vec3, b: Vec3): Vec3 => ({ x: a.x + b.x, y: a.y + b.y, z: a.z + b.z });
const scale = (a: Vec3, s: number): Vec3 => ({ x: a.x * s, y: a.y * s, z: a.z * s });

export type VolumetricOpts = {
  /** 1 = a true sphere (isotropic, orbit-robust); <1 flattens the depth axis. */
  depthScale: number;
};

/**
 * Volumetric radar layout. Same graph resolution as the shipped `layoutRadarScene`
 * (flat-agent guard, orphan promotion, folder clustering, parent→child links) — only
 * the GEOMETRY changes: every fan-out (subagents around a parent, roots within a
 * folder, folders around the origin) is a Fibonacci shell instead of a tilted ring.
 */
export function layoutRadarVolumetric(model: RadarSceneModel, opts: VolumetricOpts): RadarLayout {
  const { depthScale } = opts;
  const agents = model.agents;
  const byId = new Map(agents.map((a) => [a.id, a]));

  // A parentId only RESOLVES if the parent is present AND not flat (identical rule to
  // the shipped layout, so linkage and roots never disagree).
  const resolvesParent = (a: RadarAgent): boolean => {
    const pid = a.parentId;
    if (!pid) return false;
    const parent = byId.get(pid);
    return Boolean(parent) && !isFlatAgent(parent!);
  };

  const childrenOf = new Map<string, RadarAgent[]>();
  for (const a of agents) {
    if (resolvesParent(a)) {
      const list = childrenOf.get(a.parentId!) ?? [];
      list.push(a);
      childrenOf.set(a.parentId!, list);
    }
  }
  for (const list of childrenOf.values()) list.sort((x, y) => x.id.localeCompare(y.id));

  const roots = agents
    .filter((a) => a.depth === 0 || !resolvesParent(a))
    .sort((x, y) => x.id.localeCompare(y.id));

  const nodes: LayoutNode[] = [];
  const links: OrbLink[] = [];

  // ── children as a Fibonacci shell around their parent ───────────────────────
  function placeChildren(parent: RadarAgent, parentCenter: Vec3, parentRadius: number) {
    const kids = childrenOf.get(parent.id);
    if (!kids || kids.length === 0) return;
    const phase = phaseSeed(parent.id);
    const k = kids.length;
    const childReach = kids.reduce((m, kid) => Math.max(m, radarRadius(kid.contextTokens, kid.depth)), 0.34);
    const minBase = (parentRadius + 2.2) * depthShrink(kids[0].depth);
    const shell = shellRadius(k, childReach, 0.7, minBase);
    kids.forEach((kid, i) => {
      const dir = k === 1 ? { x: 0, y: 0.62, z: 0 } : fibDirection(i, k, depthScale, phase);
      const pos = add(parentCenter, scale(dir, shell));
      const node = makeNode(kid, pos);
      nodes.push(node);
      links.push({ source: parent.id, target: kid.id, kind: 'agent_issue' });
      placeChildren(kid, pos, node.radius);
    });
  }

  // ── folder clustering (identical keys to the shipped layout) ────────────────
  const folderKey = (r: RadarAgent): string => {
    const dir = r.cwd?.trim();
    if (dir) return `dir:${dir}`;
    const label = r.label?.trim();
    if (label) return `task:${label}`;
    return `harness:${r.harness || '∅'}`;
  };
  const folderLabelOf = (r: RadarAgent): string =>
    r.cwd?.trim() || r.label?.trim() || radarHarness(r.harness).label;

  // The farthest a root reaches from its own centre: its globe plus (if it has kids)
  // its moon shell + the moon's globe. Drives both the in-folder shell and the
  // inter-folder shell so nothing overlaps.
  const rootReach = (r: RadarAgent): number => {
    const rr = radarRadius(r.contextTokens, 0);
    const kids = childrenOf.get(r.id);
    if (!kids || kids.length === 0) return rr;
    const childR = kids.reduce((m, kid) => Math.max(m, radarRadius(kid.contextTokens, kid.depth)), 0.34);
    const minBase = (rr + 2.2) * depthShrink(kids[0].depth);
    return shellRadius(kids.length, childR, 0.7, minBase) + childR;
  };

  const folderMap = new Map<string, RadarAgent[]>();
  for (const r of roots) {
    const kf = folderKey(r);
    const list = folderMap.get(kf) ?? [];
    list.push(r);
    folderMap.set(kf, list);
  }
  const folderKeys = [...folderMap.keys()].sort((a, b) => a.localeCompare(b));

  type ClusterPlan = {
    key: string;
    label: string;
    harness: string;
    members: RadarAgent[];
    innerShell: number; // Fibonacci shell the roots ride within the folder
    extent: number; // outer reach of the whole constellation
  };
  const plans: ClusterPlan[] = folderKeys.map((key) => {
    const members = folderMap
      .get(key)!
      .slice()
      .sort((a, b) => a.harness.localeCompare(b.harness) || a.id.localeCompare(b.id));
    const maxReach = members.reduce((m, r) => Math.max(m, rootReach(r)), 0.5);
    const innerShell = shellRadius(members.length, maxReach, 1.0, 0);
    const counts = new Map<string, number>();
    for (const m of members) counts.set(m.harness, (counts.get(m.harness) ?? 0) + 1);
    const harness = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0];
    return { key, label: folderLabelOf(members[0]), harness, members, innerShell, extent: innerShell + maxReach };
  });

  // ── constellations as a Fibonacci shell around the origin ───────────────────
  // Each folder claims a Fibonacci direction (indexed by key order) and sits on a
  // shell sized so the constellations never touch. One folder → dead centre.
  const K = plans.length;
  const maxExtent = plans.reduce((m, p) => Math.max(m, p.extent), 0.5);
  const galaxyShell = shellRadius(K, maxExtent, 2.0, 0);

  const clusters: RadarCluster[] = [];
  plans.forEach((plan, ci) => {
    const dir = K === 1 ? { x: 0, y: 0, z: 0 } : fibDirection(ci, K, depthScale);
    const center = add({ x: 0, y: 0, z: 0 }, scale(dir, galaxyShell));

    const place = (root: RadarAgent, pos: Vec3) => {
      const node = makeNode(root, pos);
      nodes.push(node);
      placeChildren(root, pos, node.radius);
    };

    const n = plan.members.length;
    if (n === 1) {
      place(plan.members[0], center);
    } else {
      const phase = phaseSeed(plan.key);
      plan.members.forEach((root, j) => {
        const dirR = fibDirection(j, n, depthScale, phase);
        place(root, add(center, scale(dirR, plan.innerShell)));
      });
    }

    clusters.push({ key: plan.key, label: plan.label, harness: plan.harness, center, radius: plan.extent });
  });

  return { nodes, links, clusters };
}

/** Variant A — a flat disc (depth axis fully collapsed): reproduces the shipped-before
 *  failure mode. Face-on it looks fine; orbit ~90° and it flattens to a vertical line. */
export const layoutRadarDiscFlat = (model: RadarSceneModel): RadarLayout =>
  layoutRadarVolumetric(model, { depthScale: 0 });

/** Variant B — a true isotropic sphere. No thin axis → no camera angle collapses it. */
export const layoutRadarSphere = (model: RadarSceneModel): RadarLayout =>
  layoutRadarVolumetric(model, { depthScale: 1.0 });

/** Variant C — a thick lens: more screen-plane spread + shallower occlusion, but a
 *  flattened depth axis, so it still shows a bad ~90° edge (the compare proves this). */
export const layoutRadarLens = (model: RadarSceneModel): RadarLayout =>
  layoutRadarVolumetric(model, { depthScale: 0.5 });
