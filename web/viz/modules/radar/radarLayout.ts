// radarLayout.ts — abacus-board geometry for the RADAR constellation.
//
// Reuses the orb engine's `OrbLayout`/`LayoutNode`/`Vec3` (no fork) so the radar
// scene renders through the same mesh/link path as Habits. The board is a stack of
// horizontal rails, one per folder (cwd): a folder's root agents are beads laid out
// left to right along its rail, and each agent's subagents hang one row-step below
// it as a width-aware tidy tree. Every node sits on the z=0 plane and carries its
// live `RadarAgent`; a glowing parent->child link is emitted per non-root agent.
//
// Visual law (spec §7): size = a bounded √(contextTokens) with a HIERARCHY BOOST
// so a depth-0 main reads as noticeably larger than its subs. Pure + deterministic
// (positions are a function of the model alone), so it is unit-tested without WebGL.

import type { LayoutNode, OrbLayout, OrbLink, Vec3 } from '@/viz/shared/types/orbTypes';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { radarHarness, RADAR_NEUTRAL } from './radarTheme';

/**
 * One folder rail: every root sharing a project folder (cwd) is grouped into one
 * cluster (the rail's beads). `center` is the rail HEAD (just left of the first
 * bead, on the rail's y); the render draws `label` there and the rail rod runs from
 * it to the rightmost root bead.
 */
export type RadarCluster = {
  key: string;
  /** The folder/project name shown at the rail head (e.g. "WARDEN"). */
  label: string;
  /** Dominant harness on the rail: drives the label + rail-rod hue (color-blind a11y). */
  harness: string;
  center: Vec3;
  /** Nominal tag extent (label placement); the rod length is computed in the renderer. */
  radius: number;
};

/** `OrbLayout` plus the per-folder cluster metadata the radar label layer reads. */
export type RadarLayout = OrbLayout & { clusters: RadarCluster[] };

// Hierarchy is read as SIZE: a main planet is biggest, and every level down is a
// FIXED FRACTION of it, so a subagent can never masquerade as a root (its load can
// grow it within its own tier, never past its parent's tier).
// (Index past the table clamps to the last, deepest value.)
const DEPTH_SCALE = [1, 0.55, 0.4, 0.3];

function depthScale(depth: number): number {
  const d = Math.max(0, Math.floor(depth));
  return DEPTH_SCALE[Math.min(d, DEPTH_SCALE.length - 1)];
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
  // √-scaled occupancy term, capped. √200k ≈ 447, so /1000 keeps the cap reachable.
  // The whole radius is then scaled DOWN per depth, so a subagent reads as a clear
  // fraction of its parent regardless of how much context it is carrying.
  const occupancy = Math.min(0.5, Math.sqrt(tokens) / 1000);
  return (0.4 + occupancy) * depthScale(depth);
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
 * Lay out the live forest as an abacus board. Roots are deterministically ordered
 * (by id), grouped into per-folder rails stacked top to bottom, and placed as beads
 * along each rail; children hang below their parent as a width-aware tidy subtree.
 * Links are emitted parent->child only when the parent is present in the model (an
 * orphan renders solo, no dangling edge).
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
  const RAIL_GAP = 4.2; // generous vertical air between rails (before depth adjust)
  const ROW_STEP = 2.3; // vertical drop per subagent level: clears parent + child radii
  const BEAD_GAP = 2.0; // horizontal room after a root bead / before the rail title
  const SIB_GAP = 1.1; // min horizontal space between sibling subagents

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

  // Deepest subtree depth (relative to the rail) among a rail's members.
  const relDepth = (a: RadarAgent): number => {
    const kids = childrenOf.get(a.id);
    if (!kids || kids.length === 0) return 0;
    return 1 + Math.max(...kids.map(relDepth));
  };
  const railDepth = (members: RadarAgent[]): number => Math.max(0, ...members.map(relDepth));

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
    railY -= RAIL_GAP + railDepth(members) * ROW_STEP;
  }

  return { nodes, links, clusters };
}
