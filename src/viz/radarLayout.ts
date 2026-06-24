// radarLayout.ts — depth-N geometry for the RADAR constellation.
//
// Reuses the orb engine's `OrbLayout`/`LayoutNode`/`Vec3` (no fork) so the radar
// scene renders through the same mesh/link path as Habits. Roots are planets
// spread on a ring; their subagents orbit them as moons; sub-subagents orbit the
// moons — recursively, depth-N. Every node carries its live `RadarAgent`, and a
// glowing parent->child link is emitted per non-root agent whose parent exists.
//
// Visual law (spec §7): size = a bounded √(contextTokens) with a HIERARCHY BOOST
// so a depth-0 main reads as noticeably larger than its subs. Pure + deterministic
// (positions are a function of the model alone), so it is unit-tested without WebGL.

import type { LayoutNode, OrbLayout, OrbLink, Vec3 } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';

// Depth boost: a main planet is biggest; each level down is meaningfully smaller.
// (Index past the table clamps to the last, deepest value.)
const DEPTH_BOOST = [0.62, 0.3, 0.16, 0.1];

function depthBoost(depth: number): number {
  const d = Math.max(0, Math.floor(depth));
  return DEPTH_BOOST[Math.min(d, DEPTH_BOOST.length - 1)];
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

// Roots ring: enough circumference that planets (plus their moon halos) don't
// collide. A floor keeps a lone/duo root from sitting on the origin awkwardly.
function rootRingRadius(count: number, maxRootR: number): number {
  if (count <= 1) return 0;
  const circumferenceNeed = (count * (maxRootR * 2 + 2.6)) / (2 * Math.PI);
  return Math.max(3.2, circumferenceNeed);
}

// A child's orbit radius around its parent — scaled by the parent's size and the
// child's depth so deeper moons hug tighter. Bounded to keep the tree compact.
function orbitRadius(parentRadius: number, childDepth: number): number {
  const base = parentRadius + 1.1;
  const shrink = Math.max(0.5, 1 - (childDepth - 1) * 0.22);
  return base * shrink;
}

// Polar placement on a tilted ring (mirrors orbLayout.satellitePosition): start
// at 12 o'clock, go clockwise, flatten Y a touch and push Z for a 3D read. A
// per-parent angular offset (seeded by id) keeps sibling rings from all aligning.
function orbitPosition(center: Vec3, index: number, total: number, ring: number, angleOffset: number): Vec3 {
  const angle = -Math.PI / 2 + angleOffset + (index / Math.max(1, total)) * Math.PI * 2;
  return {
    x: center.x + Math.cos(angle) * ring,
    y: center.y + Math.sin(angle) * ring * 0.6,
    z: center.z + Math.sin(angle) * ring * 0.5,
  };
}

// Deterministic small angle from an id so each parent's children fan out at a
// stable, distinct phase (no RNG — layout must reproduce exactly).
function angleSeed(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 997;
  return (h / 997) * Math.PI * 2;
}

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
 * Lay out the live forest. Roots are deterministically ordered (by id) and placed
 * on a ring; children are placed by a recursive descent that orbits each parent's
 * resolved centre. Links are emitted parent->child only when the parent is present
 * in the model (an orphan renders solo — no dangling edge).
 */
export function layoutRadarScene(model: RadarSceneModel): OrbLayout {
  const agents = model.agents;
  const byId = new Map(agents.map((a) => [a.id, a]));
  const childrenOf = new Map<string, RadarAgent[]>();
  for (const a of agents) {
    const pid = a.parentId;
    if (pid && byId.has(pid)) {
      const list = childrenOf.get(pid) ?? [];
      list.push(a);
      childrenOf.set(pid, list);
    }
  }
  for (const list of childrenOf.values()) list.sort((x, y) => x.id.localeCompare(y.id));

  // Roots: depth 0, OR any agent whose declared parent is absent (orphan promoted
  // to a solo root so it still renders — honest, never dropped).
  const roots = agents
    .filter((a) => a.depth === 0 || !a.parentId || !byId.has(a.parentId))
    .sort((x, y) => x.id.localeCompare(y.id));

  const maxRootR = roots.reduce((m, r) => Math.max(m, radarRadius(r.contextTokens, 0)), 0.34);
  const ring = rootRingRadius(roots.length, maxRootR);

  const nodes: LayoutNode[] = [];
  const links: OrbLink[] = [];

  function placeChildren(parent: RadarAgent, parentCenter: Vec3, parentRadius: number) {
    const kids = childrenOf.get(parent.id);
    if (!kids || kids.length === 0) return;
    const offset = angleSeed(parent.id);
    kids.forEach((kid, i) => {
      const orbit = orbitRadius(parentRadius, kid.depth);
      const pos = orbitPosition(parentCenter, i, kids.length, orbit, offset);
      const node = makeNode(kid, pos);
      nodes.push(node);
      links.push({ source: parent.id, target: kid.id, kind: 'agent_issue' });
      placeChildren(kid, pos, node.radius);
    });
  }

  roots.forEach((root, i) => {
    const center: Vec3 =
      roots.length <= 1
        ? { x: 0, y: 0, z: 0 }
        : (() => {
            const a = (i / roots.length) * Math.PI * 2 - Math.PI / 2;
            return { x: Math.cos(a) * ring, y: Math.sin(a) * ring * 0.34, z: Math.sin(a) * ring * 0.22 };
          })();
    const node = makeNode(root, center);
    nodes.push(node);
    placeChildren(root, center, node.radius);
  });

  return { nodes, links };
}
