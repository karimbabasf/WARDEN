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

import type { LayoutNode, OrbLayout, OrbLink, Vec3 } from '@/viz/shared/types/orbTypes';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { radarHarness, RADAR_NEUTRAL } from './radarTheme';

/**
 * One folder constellation — every root sharing a project folder (cwd) is grouped
 * into one cluster, laid out as its own little loop and spread across the plane
 * with a labelled gap from its neighbours. The render draws `label` under `center`.
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

// ── sector + shell tuning (all pure constants; no RNG anywhere) ────────────────
//
// Roots are grouped into per-harness ANGULAR SECTORS on the root ring, so a forest
// of 15+ agents reads as "Claude over here, Codex over there" instead of an
// interleaved clump. Within a sector each root claims an angular slice whose width
// scales with its descendant count (busy orchestrators get more room). Siblings
// that overflow one orbital ring spill onto additional concentric SHELLS so they
// never cram below a readable angular gap.

// Smallest comfortable angular gap (radians) between two same-shell siblings. A
// shell holds floor(2π / MIN_SIBLING_GAP) children before the next shell opens.
const MIN_SIBLING_GAP = 0.52; // ≈ 30° → up to 12 children on the innermost ring
// Radial step between consecutive sibling shells (must dominate the per-shell
// stagger span so shells stay visually distinct / bucketable).
const SHELL_STEP = 1.15;
// Total radial stagger SPAN across one shell so co-shell moons don't all sit on a
// perfectly flat circle. Kept well under SHELL_STEP so the shell banding (used by
// camera framing to group a subtree) is never blurred, regardless of shell size.
const STAGGER_SPAN = 0.18;

// Local LOOP radius for a folder's roots: the ring each root sits on inside its
// constellation, sized so a root plus its whole moon halo (`maxFoot`) clears its
// neighbours. The chord between two adjacent roots on the loop is 2·R·sin(π/n), so
// we invert that against the largest footprint. One root → 0 (it sits dead centre).
function localRingRadius(count: number, maxFoot: number): number {
  if (count <= 1) return 0;
  return Math.max(maxFoot, maxFoot / Math.sin(Math.PI / count));
}

// A child's base orbit radius around its parent — scaled by the parent's size and
// the child's depth so deeper moons hug tighter. The depth-1 gap is deliberately
// generous so the parent→child link is a real DRAWN tether strand (the Habits look)
// rather than a subagent bundled on top of its parent; deeper levels shrink back in
// to keep the tree compact.
function orbitRadius(parentRadius: number, childDepth: number): number {
  const base = parentRadius + 2.2;
  const shrink = Math.max(0.46, 1 - (childDepth - 1) * 0.26);
  return base * shrink;
}

// Shared tilt plane for the whole constellation (roots AND children). A ROUNDER
// ring (closer to 1.0) spreads a parent's moons in a clear circle around it so the
// parent→child tether reads as a drawn strand — a very flat ring squashed the moons
// almost onto the parent, which is what made subagents look "bundled". Still tilted
// (not a flat 1.0) so the disk keeps a 3-D read. Exported so tests can recover the
// true polar angle from tilted positions.
export const TILT_Y = 0.52;
export const TILT_Z = 0.3;

// Polar placement on a tilted ring (mirrors orbLayout.satellitePosition): start
// at 12 o'clock, flatten Y a touch and push Z for a 3D read. `angle` is supplied by
// the caller (sector- or shell-aware) rather than derived from a bare index.
function ringPosition(center: Vec3, angle: number, ring: number): Vec3 {
  return {
    x: center.x + Math.cos(angle) * ring,
    y: center.y + Math.sin(angle) * ring * TILT_Y,
    z: center.z + Math.sin(angle) * ring * TILT_Z,
  };
}

// Deterministic small angle from an id so each parent's children fan out at a
// stable, distinct phase (no RNG — layout must reproduce exactly).
function angleSeed(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 997;
  return (h / 997) * Math.PI * 2;
}

// Per-shell capacity at the minimum sibling gap. At least 1 so a shell always makes
// progress (degenerate tiny gaps can't stall the distribution).
function shellCapacity(): number {
  return Math.max(1, Math.floor((Math.PI * 2) / MIN_SIBLING_GAP));
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

  // ── multi-shell child placement ──────────────────────────────────────────────
  // Children are sorted (deterministic) then sliced into concentric shells, each
  // holding at most `shellCapacity()` siblings so no two co-shell moons fall below
  // MIN_SIBLING_GAP. Each shell sits one SHELL_STEP farther out than the last; a
  // bounded STAGGER_SPAN ripples within a shell. A per-parent seed phases the whole
  // fan so sibling families don't all align to 12 o'clock.
  function placeChildren(parent: RadarAgent, parentCenter: Vec3, parentRadius: number) {
    const kids = childrenOf.get(parent.id);
    if (!kids || kids.length === 0) return;
    const phase = angleSeed(parent.id);
    const cap = shellCapacity();
    const baseOrbit = orbitRadius(parentRadius, kids[0].depth);
    kids.forEach((kid, i) => {
      const shell = Math.floor(i / cap);
      const indexInShell = i % cap;
      // how many siblings actually share THIS shell (last shell may be partial)
      const countInShell = Math.min(cap, kids.length - shell * cap);
      // bounded intra-shell stagger: spread across STAGGER_SPAN total (NOT per-child
      // accumulation), so even a full shell stays within a hair of its base radius
      // and the SHELL_STEP banding between shells is preserved.
      const stagger = countInShell > 1 ? (indexInShell / (countInShell - 1)) * STAGGER_SPAN : 0;
      const orbit = baseOrbit + shell * SHELL_STEP + stagger;
      // even spacing within the shell (≥ MIN_SIBLING_GAP since countInShell ≤ cap),
      // with alternating shells half-phased so radial spokes don't stack.
      const shellPhase = phase + (shell % 2) * (Math.PI / Math.max(1, countInShell));
      const angle = -Math.PI / 2 + shellPhase + (indexInShell / Math.max(1, countInShell)) * Math.PI * 2;
      const pos = ringPosition(parentCenter, angle, orbit);
      const node = makeNode(kid, pos);
      nodes.push(node);
      links.push({ source: parent.id, target: kid.id, kind: 'agent_issue' });
      placeChildren(kid, pos, node.radius);
    });
  }

  // ── folder constellations ────────────────────────────────────────────────────
  // Roots are grouped into per-FOLDER clusters (by cwd): every agent you're running
  // in one project forms one constellation — its roots arranged on a local loop, its
  // subagents tethered out as moons. Clusters are spread across the plane with a
  // labelled gap between them. Harness identity is carried by COLOUR (Claude orange /
  // Codex blue), never by position, so a folder driven by both harnesses reads as a
  // single constellation in two hues. Deterministic: folders + members are sorted.

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

  // The farthest a root reaches from its own centre: its globe plus (if it has
  // children) the outermost moon shell + that moon's globe. Drives both the local
  // loop radius and the inter-cluster spacing so nothing overlaps.
  const rootReach = (r: RadarAgent): number => {
    const rr = radarRadius(r.contextTokens, 0);
    const kids = childrenOf.get(r.id);
    if (!kids || kids.length === 0) return rr;
    const shells = Math.floor((kids.length - 1) / shellCapacity());
    const childR = kids.reduce((m, k) => Math.max(m, radarRadius(k.contextTokens, k.depth)), 0.34);
    return orbitRadius(rr, kids[0].depth) + shells * SHELL_STEP + childR;
  };

  const CLUSTER_MARGIN = 0.8; // breathing room around each root's reach
  const CLUSTER_GAP = 1.8; // empty lateral space between adjacent constellations
  const CLUSTER_ARC_DEPTH = 1.4; // shallow camera-facing bow so clusters don't recede flat

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
  // same-harness roots sit adjacent on the loop; a local ring radius; an outer
  // extent; a display label; and the dominant harness (for the label hue only).
  type ClusterPlan = {
    key: string;
    label: string;
    harness: string;
    members: RadarAgent[];
    ringR: number;
    extent: number;
  };
  const plans: ClusterPlan[] = folderKeys.map((key) => {
    const members = folderMap
      .get(key)!
      .slice()
      .sort((a, b) => a.harness.localeCompare(b.harness) || a.id.localeCompare(b.id));
    const maxFoot = members.reduce((m, r) => Math.max(m, rootReach(r) + CLUSTER_MARGIN), 0.5);
    const ringR = localRingRadius(members.length, maxFoot);
    const counts = new Map<string, number>();
    for (const m of members) counts.set(m.harness, (counts.get(m.harness) ?? 0) + 1);
    const harness = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0];
    return { key, label: folderLabelOf(members[0]), harness, members, ringR, extent: ringR + maxFoot };
  });

  // Lay the constellations left→right, centred on the origin, each pulled back along
  // a shallow camera-facing arc (mirrors the Habits zone arc) so a row of folders
  // reads side-by-side rather than marching into the distance.
  const totalWidth =
    plans.reduce((s, p) => s + p.extent * 2, 0) + CLUSTER_GAP * Math.max(0, plans.length - 1);
  const halfSpan = totalWidth / 2;
  const lastIdx = Math.max(1, plans.length - 1);
  const rawZ = plans.map((_, i) => -CLUSTER_ARC_DEPTH * (1 - Math.cos((i / lastIdx) * (Math.PI / 2))));
  const meanZ = rawZ.reduce((a, b) => a + b, 0) / Math.max(1, rawZ.length);

  const clusters: RadarCluster[] = [];
  let cursor = -halfSpan;
  plans.forEach((plan, i) => {
    const cx = cursor + plan.extent; // cluster centre on the lateral axis
    cursor += plan.extent * 2 + CLUSTER_GAP;
    const center: Vec3 = { x: cx, y: 0, z: rawZ[i] - meanZ };

    const place = (root: RadarAgent, pos: Vec3) => {
      const node = makeNode(root, pos);
      nodes.push(node);
      placeChildren(root, pos, node.radius);
    };

    if (plan.members.length === 1) {
      // a lone root sits dead-centre in its constellation.
      place(plan.members[0], center);
    } else {
      // roots ride a local loop around the cluster centre (the "loop link").
      const n = plan.members.length;
      const phase = angleSeed(plan.key);
      plan.members.forEach((root, j) => {
        const angle = phase + (j / n) * Math.PI * 2;
        place(root, ringPosition(center, angle, plan.ringR));
      });
    }

    clusters.push({ key: plan.key, label: plan.label, harness: plan.harness, center, radius: plan.extent });
  });

  return { nodes, links, clusters };
}
