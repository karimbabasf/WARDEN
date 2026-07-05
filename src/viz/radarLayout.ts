// radarLayout.ts — VOLUMETRIC (Fibonacci-sphere) geometry for the RADAR constellation.
//
// Reuses the orb engine's `OrbLayout`/`LayoutNode`/`Vec3` (no fork) so the radar
// scene renders through the same mesh/link path as Habits. Every node carries its
// live `RadarAgent`, and a glowing parent->child link is emitted per non-root agent
// whose parent exists.
//
// WHY A SPHERE (the 2026-07 redesign). The old layout placed every node on a single
// TILTED DISC — `ringPosition` drove y and z from the SAME sin(angle), so every point
// satisfied z ≈ 0.577·y: one plane. A flat plane read edge-on projects to a LINE, and
// the free-orbit camera passes through that edge-on azimuth every rotation → "the
// globes all line up in a straight path." Spreading folders as a field fixed the
// left→right ROW but not the edge-on collapse, and did nothing for the common case
// (a swarm of subagents under ONE orchestrator), whose moons rode a flat ring.
//
// The fix is dimensional: every fan-out is now a FIBONACCI SPHERE. A Fibonacci sphere
// spreads K points with near-even angular spacing and — the whole point — has NO thin
// axis, so no camera angle ever collapses it to a line. Subagents surround their
// orchestrator as a true 3-D halo; multiple roots in a folder ride a small sphere;
// and folder centres ride a framable HORIZONTAL field (never stacked off-screen).
//
// Visual law (spec §7): size = a bounded √(contextTokens) with a HIERARCHY BOOST so a
// depth-0 main reads noticeably larger than its subs. Pure + deterministic (positions
// are a function of the model alone; zero RNG), so it is unit-tested without WebGL.

import type { LayoutNode, OrbLayout, OrbLink, Vec3 } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { radarHarness, RADAR_NEUTRAL } from './radarTheme';

/**
 * One folder constellation — every root sharing a project folder (cwd) is grouped
 * into one cluster laid out as its own little sphere and spread across a horizontal
 * field with a labelled gap from its neighbours. The render draws `label` under `center`.
 */
export type RadarCluster = {
  key: string;
  /** The folder/project name shown under the constellation (e.g. "WARDEN"). */
  label: string;
  /** Dominant harness in the cluster — drives the label hue only (color-blind a11y). */
  harness: string;
  center: Vec3;
  /** Outer extent of the cluster (label placement + camera framing). */
  radius: number;
};

/** `OrbLayout` plus the per-folder cluster metadata the radar label layer reads. */
export type RadarLayout = OrbLayout & { clusters: RadarCluster[] };

// Depth boost: a main planet is biggest; each level down is meaningfully smaller.
// (Index past the table clamps to the last, deepest value.)
const DEPTH_BOOST = [0.62, 0.3, 0.16, 0.1];

function depthBoost(depth: number): number {
  const d = Math.max(0, Math.floor(depth));
  return DEPTH_BOOST[Math.min(d, DEPTH_BOOST.length - 1)];
}

/**
 * Honest-viz flat-globe guard (spec §4.4 / §5). Some agents CANNOT have children,
 * by the nature of their data source:
 *
 *   • VS Code Codex (`origin === 'codex_vscode'`) — that integration spawns no
 *     subagents; the rollout files carry no `parent_thread_id` tree.
 *   • Unknown / empty harness — a schema-drift globe we render neutrally; we have
 *     no hierarchy signal for it, so we never invent one.
 *
 * Such an agent is a FLAT solo globe: even if a malformed payload hands us a child
 * whose `parentId` points at it, the layout refuses to orbit that child under it.
 * Pure + exported so the guarantee is unit-tested directly. (A Codex Desktop agent
 * — `origin: 'Codex Desktop'` or unset — is NOT flat: it legitimately has children.)
 */
export function isFlatAgent(agent: RadarAgent): boolean {
  if (agent.origin === 'codex_vscode') return true;
  return radarHarness(agent.harness) === RADAR_NEUTRAL;
}

/**
 * Globe radius from live context occupancy + hierarchy boost. Monotonic in
 * `contextTokens` (more = bigger) but bounded so one near-full agent can't
 * dominate the scene; depth 0 gets the largest boost so mains > subs at equal load.
 */
export function radarRadius(contextTokens: number, depth: number): number {
  const tokens = Math.max(0, Number.isFinite(contextTokens) ? contextTokens : 0);
  // √-scaled occupancy term, capped. √200k ≈ 447, so /900 keeps the cap reachable.
  const occupancy = Math.min(0.6, Math.sqrt(tokens) / 900);
  return 0.34 + occupancy + depthBoost(depth);
}

// ── Fibonacci-sphere geometry (all pure; no RNG anywhere) ──────────────────────
//
// The golden angle — successive Fibonacci points step this far in longitude, which is
// what makes the lattice even (no two points share a meridian, no banding).
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5)); // ≈ 2.399963 rad

// Clearance we hold between the SURFACES of two neighbouring globes on a shell, and
// between two constellations. Bigger = airier.
const CHILD_GAP = 0.7;
const ROOT_GAP = 1.0;
const CLUSTER_GAP = 1.8;
// Radial probe step when a folder's hashed ray is congested (galaxy packing).
const GALAXY_STEP = 0.4;

/**
 * Unit direction of point `i` of `k` on a Fibonacci sphere. `phase` rotates the whole
 * lattice in longitude so sibling families under different parents don't all present
 * the same face. Returns the origin for k ≤ 1 (a lone node sits at its own centre; the
 * caller handles that). Exported so tests can reason about the distribution directly.
 */
export function fibDirection(i: number, k: number, phase = 0): Vec3 {
  if (k <= 1) return { x: 0, y: 0, z: 0 };
  const y = 1 - (i / (k - 1)) * 2; // walk the poles: +1 (top) → −1 (bottom)
  const r = Math.sqrt(Math.max(0, 1 - y * y)); // ring radius at this latitude
  const theta = i * GOLDEN_ANGLE + phase;
  return { x: Math.cos(theta) * r, y, z: Math.sin(theta) * r };
}

// Empirical nearest-neighbour angular spacing on a Fibonacci sphere of K points
// (radians): points sit ~this far apart, so a shell sized off it never crowds.
function neighborAngle(k: number): number {
  return 3.1 / Math.sqrt(Math.max(1, k));
}

/**
 * Radius of a shell holding `count` items each reaching `itemReach`, so adjacent items
 * keep at least `gap` of clear space between their surfaces. Grows ∝ √count, so a
 * parent with 25 subagents opens a wider shell automatically — no crowding, ever.
 * Never smaller than `minBase` (a small family still sits a comfortable distance out).
 */
function shellRadius(count: number, itemReach: number, gap: number, minBase: number): number {
  if (count <= 1) return minBase;
  const chord = 2 * itemReach + gap; // surface-to-surface clearance we want, as a chord
  const needed = chord / neighborAngle(count); // chord ≈ R·Δθ → R = chord / Δθ
  return Math.max(minBase, needed);
}

// Child depth → tighter hug for deeper moons (keeps a deep tree compact). The depth-1
// gap (via `minBase` below) is deliberately generous so the parent→child link is a real
// DRAWN tether strand rather than a subagent bundled on top of its parent.
function depthShrink(childDepth: number): number {
  return Math.max(0.5, 1 - (childDepth - 1) * 0.24);
}

// A child's base orbit radius around its parent (before the shell auto-grows it for
// large sibling sets): the parent's size + a generous tether gap, tightened by depth.
function orbitBase(parentRadius: number, childDepth: number): number {
  return (parentRadius + 2.2) * depthShrink(childDepth);
}

// Deterministic angle from an id, so each parent's moon lattice gets a stable, distinct
// phase (no RNG — layout must reproduce exactly). Fine for a phase (any rotation reads
// the same); NOT used for folder rays, which need even spread (see `galaxyAngle`).
function angleSeed(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 997;
  return (h / 997) * Math.PI * 2;
}

// Folder RAY angle — a low-discrepancy (Knuth multiplicative) hash so that
// near-identical keys ("proj-0", "proj-1", …) map to MAXIMALLY separated angles rather
// than a tiny arc that would stack every folder onto one ray (a row). Deterministic and
// append-stable: a pure function of the key, so an existing folder never changes angle.
function galaxyAngle(key: string): number {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (Math.imul(h, 31) + key.charCodeAt(i)) >>> 0;
  // Knuth's multiplicative constant spreads consecutive hashes across the whole circle.
  return ((Math.imul(h, 2654435761) >>> 0) / 4294967296) * Math.PI * 2;
}

// A point on the HORIZONTAL ground field (y = 0). Folder centres ride this so a busy
// fleet spreads left↔right / front↔back across the frame — always framable, never
// stacked vertically off the top or bottom of the screen. The 3-D read comes from the
// spheres WITHIN each folder, not from tilting the field.
function horizontalPoint(theta: number, ray: number): Vec3 {
  return { x: Math.cos(theta) * ray, y: 0, z: Math.sin(theta) * ray };
}

const add = (a: Vec3, b: Vec3): Vec3 => ({ x: a.x + b.x, y: a.y + b.y, z: a.z + b.z });
const scale = (a: Vec3, s: number): Vec3 => ({ x: a.x * s, y: a.y * s, z: a.z * s });

function makeNode(agent: RadarAgent, position: Vec3): LayoutNode {
  return {
    id: agent.id,
    // Reuse the existing union: roots → 'hub' (planet), subs → 'issue' (moon).
    kind: agent.depth === 0 ? 'hub' : 'issue',
    position,
    radius: radarRadius(agent.contextTokens, agent.depth),
    agentId: agent.id,
    harness: agent.harness,
    radarAgent: agent,
    depth: agent.depth,
  };
}

/**
 * Lay out the live forest. Roots are deterministically ordered (by id) and grouped by
 * folder; each folder is a small sphere of roots on a horizontal field; each root's
 * subagents surround it as a Fibonacci sphere, recursively depth-N. Links are emitted
 * parent->child only when the parent is present (an orphan renders solo — no dangling edge).
 */
export function layoutRadarScene(model: RadarSceneModel): RadarLayout {
  const agents = model.agents;
  const byId = new Map(agents.map((a) => [a.id, a]));

  // A parentId only RESOLVES if the parent is present AND not flat. A flat parent
  // (VS Code Codex / unknown harness) cannot own children, so a child pointing at
  // one is treated exactly like an orphan: no orbit, no link, promoted to a solo
  // root below. One predicate so the linkage and the roots filter never disagree.
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

  // Roots: depth 0, OR any agent whose declared parent does not resolve (absent OR
  // flat) — promoted to a solo root so it still renders (honest, never dropped),
  // but never fabricated as a moon under a globe that cannot have one.
  const roots = agents
    .filter((a) => a.depth === 0 || !resolvesParent(a))
    .sort((x, y) => x.id.localeCompare(y.id));

  const nodes: LayoutNode[] = [];
  const links: OrbLink[] = [];

  // ── children as a Fibonacci sphere around their parent ───────────────────────
  // The sibling set is sorted (deterministic) and spread over a sphere shell whose
  // radius auto-grows with the sibling count (√count), so 3 moons or 30 both stay
  // evenly spaced and clear of one another. A per-parent phase rotates each family to
  // a distinct face so sibling families never present identically.
  function placeChildren(parent: RadarAgent, parentCenter: Vec3, parentRadius: number) {
    const kids = childrenOf.get(parent.id);
    if (!kids || kids.length === 0) return;
    const phase = angleSeed(parent.id);
    const k = kids.length;
    const childReach = kids.reduce((m, kid) => Math.max(m, radarRadius(kid.contextTokens, kid.depth)), 0.34);
    const minBase = orbitBase(parentRadius, kids[0].depth);
    const shell = shellRadius(k, childReach, CHILD_GAP, minBase);
    kids.forEach((kid, i) => {
      // A lone child sits up-and-OUT from its parent along a per-parent-phased
      // direction — so a single-child chain (root→sub→sub-sub) arcs through space
      // instead of stacking into a dead-vertical line (each parent has its own phase).
      const dir =
        k === 1
          ? { x: Math.cos(phase) * 0.6, y: 0.72, z: Math.sin(phase) * 0.6 }
          : fibDirection(i, k, phase);
      const pos = add(parentCenter, scale(dir, shell));
      const node = makeNode(kid, pos);
      nodes.push(node);
      links.push({ source: parent.id, target: kid.id, kind: 'agent_issue' });
      placeChildren(kid, pos, node.radius);
    });
  }

  // ── folder constellations ────────────────────────────────────────────────────
  // Roots are grouped into per-FOLDER clusters (by cwd): every agent you're running
  // in one project forms one constellation. Harness identity is carried by COLOUR
  // (Claude orange / Codex blue), never by position, so a folder driven by both
  // harnesses reads as a single constellation in two hues. Deterministic: folders +
  // members are sorted.

  // Folder key: a real cwd first; else the agent's own label (a Claude root's task);
  // else its harness. So two roots in the same project cluster together, and a
  // cwd-less stray still gets its own constellation rather than a shared bucket.
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
  // its moon shell + the moon's globe. Drives both the in-folder sphere and the
  // inter-folder field so nothing overlaps.
  const rootReach = (r: RadarAgent): number => {
    const rr = radarRadius(r.contextTokens, 0);
    const kids = childrenOf.get(r.id);
    if (!kids || kids.length === 0) return rr;
    const childR = kids.reduce((m, k) => Math.max(m, radarRadius(k.contextTokens, k.depth)), 0.34);
    return shellRadius(kids.length, childR, CHILD_GAP, orbitBase(rr, kids[0].depth)) + childR;
  };

  // Group roots into folders (deterministic order).
  const folderMap = new Map<string, RadarAgent[]>();
  for (const r of roots) {
    const k = folderKey(r);
    const list = folderMap.get(k) ?? [];
    list.push(r);
    folderMap.set(k, list);
  }
  const folderKeys = [...folderMap.keys()].sort((a, b) => a.localeCompare(b));

  // Resolve each folder into a cluster plan: members ordered (harness then id) so
  // same-harness roots sit adjacent; an inner sphere radius for the roots; an outer
  // extent; a display label; and the dominant harness (for the label hue only).
  type ClusterPlan = {
    key: string;
    label: string;
    harness: string;
    members: RadarAgent[];
    innerShell: number;
    extent: number;
  };
  const plans: ClusterPlan[] = folderKeys.map((key) => {
    const members = folderMap
      .get(key)!
      .slice()
      .sort((a, b) => a.harness.localeCompare(b.harness) || a.id.localeCompare(b.id));
    const maxReach = members.reduce((m, r) => Math.max(m, rootReach(r)), 0.5);
    const innerShell = shellRadius(members.length, maxReach, ROOT_GAP, 0);
    const counts = new Map<string, number>();
    for (const m of members) counts.set(m.harness, (counts.get(m.harness) ?? 0) + 1);
    const harness = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0];
    return { key, label: folderLabelOf(members[0]), harness, members, innerShell, extent: innerShell + maxReach };
  });

  // ── galaxy packing on a horizontal field ─────────────────────────────────────
  // Constellations spread around the origin on the ground plane, never a vertical
  // stack: each folder claims a stable ray (angle hashed from its key) and slides
  // outward from the centre only as far as needed to clear everything already placed,
  // plus a breathing gap. Properties, all by construction:
  //   • overlap-free for arbitrarily mixed constellation sizes;
  //   • deterministic (key-ordered placement, hashed angles, zero RNG);
  //   • a NEW folder lands on its own ray and — placed in key order — at most nudges
  //     later-keyed neighbours outward; it never reshuffles the field (the render also
  //     damps toward layout, so any shift glides);
  //   • horizontal, so a busy fleet fills the frame left↔right instead of marching off
  //     the top/bottom — the 3-D depth lives inside each folder's spheres.
  const origin: Vec3 = { x: 0, y: 0, z: 0 };
  const dist3 = (a: Vec3, b: Vec3): number => Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
  const placedClusters: { center: Vec3; extent: number }[] = [];

  const clusters: RadarCluster[] = [];
  plans.forEach((plan) => {
    const theta = galaxyAngle(plan.key);
    let ray = 0;
    let center = origin;
    const collides = (c: Vec3): boolean =>
      placedClusters.some((p) => dist3(c, p.center) < p.extent + plan.extent + CLUSTER_GAP);
    while (collides(center)) {
      ray += GALAXY_STEP;
      center = horizontalPoint(theta, ray);
    }
    placedClusters.push({ center, extent: plan.extent });

    const place = (root: RadarAgent, pos: Vec3) => {
      const node = makeNode(root, pos);
      nodes.push(node);
      placeChildren(root, pos, node.radius);
    };

    if (plan.members.length === 1) {
      // a lone root sits dead-centre in its constellation.
      place(plan.members[0], center);
    } else {
      // roots ride a small Fibonacci sphere around the cluster centre.
      const n = plan.members.length;
      const phase = angleSeed(plan.key);
      plan.members.forEach((root, j) => {
        const dir = fibDirection(j, n, phase);
        place(root, add(center, scale(dir, plan.innerShell)));
      });
    }

    clusters.push({ key: plan.key, label: plan.label, harness: plan.harness, center, radius: plan.extent });
  });

  return { nodes, links, clusters };
}
