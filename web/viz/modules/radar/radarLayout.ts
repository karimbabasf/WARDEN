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

  // abacus rails: one horizontal rail per folder, stacked top to bottom
  const RAIL_GAP = 3.2; // base vertical space below a rail (before depth adjust)
  const ROW_STEP = 1.5; // vertical drop per subagent depth level
  const BEAD_GAP = 1.6; // min horizontal space after a root bead
  const SIB_GAP = 1.0; // min horizontal space between sibling subagents

  const folderKey = (r: RadarAgent): string => {
    const dir = r.cwd?.trim();
    if (dir) return `dir:${dir}`;
    const label = r.label?.trim();
    if (label) return `task:${label}`;
    return `harness:${r.harness || 'none'}`;
  };
  const folderLabelOf = (r: RadarAgent): string =>
    r.cwd?.trim() || r.label?.trim() || radarHarness(r.harness).label;

  // Group roots into rails. `roots` is already id-sorted (deterministic); a rail
  // appears in the order its first root appears, and holds its members in that
  // same order. Position never depends on activity, so a folder never jumps when
  // an agent inside it changes state (spatial stability).
  const railOrder: string[] = [];
  const railMembers = new Map<string, RadarAgent[]>();
  for (const r of roots) {
    const k = folderKey(r);
    if (!railMembers.has(k)) {
      railMembers.set(k, []);
      railOrder.push(k);
    }
    railMembers.get(k)!.push(r);
  }

  // Width a subtree needs on the board: a leaf takes its own bead footprint; an
  // internal node takes the max of its own footprint and the summed width of its
  // children (plus sibling gaps). Bottom-up, memoised per agent.
  const widthCache = new Map<string, number>();
  function subtreeWidth(a: RadarAgent): number {
    const cached = widthCache.get(a.id);
    if (cached !== undefined) return cached;
    const own = 2 * radarRadius(a.contextTokens, a.depth) + SIB_GAP;
    const kids = childrenOf.get(a.id);
    let w = own;
    if (kids && kids.length > 0) {
      const childrenW = kids.reduce((s, k) => s + subtreeWidth(k), 0);
      w = Math.max(own, childrenW);
    }
    widthCache.set(a.id, w);
    return w;
  }

  // Place a subtree whose block spans [left, left + subtreeWidth(parent)] at row
  // `py`; the parent is centred over its children (or over its own block if leaf).
  function placeSubtree(parent: RadarAgent, left: number, py: number): number {
    const w = subtreeWidth(parent);
    const kids = childrenOf.get(parent.id);
    let parentX: number;
    if (!kids || kids.length === 0) {
      parentX = left + w / 2;
    } else {
      let cursor = left;
      const cy = py - ROW_STEP;
      const centres: number[] = [];
      for (const kid of kids) {
        const cx = placeSubtree(kid, cursor, cy);
        centres.push(cx);
        cursor += subtreeWidth(kid);
      }
      parentX = centres.reduce((s, c) => s + c, 0) / centres.length;
    }
    const node = makeNode(parent, { x: parentX, y: py, z: 0 });
    nodes.push(node);
    const pid = parent.parentId;
    if (pid && childrenOf.has(pid) && childrenOf.get(pid)!.some((k) => k.id === parent.id)) {
      links.push({ source: pid, target: parent.id, kind: 'agent_issue' });
    }
    return parentX;
  }

  const clusters: RadarCluster[] = [];
  let railY = 0;
  for (const k of railOrder) {
    const members = railMembers.get(k)!;
    let x = 0;
    for (const root of members) {
      placeSubtree(root, x, railY);
      x += subtreeWidth(root) + BEAD_GAP;
    }
    // Folder tag sits at the rail head, just left of the first bead.
    clusters.push({
      key: k,
      label: folderLabelOf(members[0]),
      harness: members[0].harness,
      center: { x: -BEAD_GAP, y: railY, z: 0 },
      radius: 1,
    });
    railY -= RAIL_GAP;
  }

  return { nodes, links, clusters };
}
