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
import { radarHarness, RADAR_NEUTRAL } from './radarTheme';

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

// Roots ring: enough circumference that planets (plus their moon halos) don't
// collide. A floor keeps a lone/duo root from sitting on the origin awkwardly.
// `weightSum` (Σ of per-root subtree weights) widens the ring when the forest is
// busy, so subtree-scaled slices below have room to breathe.
function rootRingRadius(count: number, maxRootR: number, weightSum: number): number {
  if (count <= 1) return 0;
  const circumferenceNeed = (weightSum * (maxRootR * 2 + 3.4)) / (2 * Math.PI);
  return Math.max(4.0, circumferenceNeed);
}

// A child's base orbit radius around its parent — scaled by the parent's size and
// the child's depth so deeper moons hug tighter. Bounded to keep the tree compact.
function orbitRadius(parentRadius: number, childDepth: number): number {
  const base = parentRadius + 1.5;
  const shrink = Math.max(0.5, 1 - (childDepth - 1) * 0.22);
  return base * shrink;
}

// Polar placement on a tilted ring (mirrors orbLayout.satellitePosition): start
// at 12 o'clock, flatten Y a touch and push Z for a 3D read. `angle` is supplied by
// the caller (sector- or shell-aware) rather than derived from a bare index.
function ringPosition(center: Vec3, angle: number, ring: number): Vec3 {
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
export function layoutRadarScene(model: RadarSceneModel): OrbLayout {
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

  // Transitive descendant count over the RESOLVED tree only (flat parents own no
  // subtree, so their strays never inflate anyone's weight). Memoised, and guarded
  // against a malformed cyclic payload (`seen`) so schema drift can never spin the
  // layout — honest-viz: a drifted forest degrades gracefully, never crashes.
  const descCache = new Map<string, number>();
  function descendantCount(id: string, seen: Set<string> = new Set()): number {
    const cached = descCache.get(id);
    if (cached !== undefined) return cached;
    if (seen.has(id)) return 0; // cycle: stop counting, don't recurse forever
    seen.add(id);
    const kids = childrenOf.get(id) ?? [];
    let total = kids.length;
    for (const k of kids) total += descendantCount(k.id, seen);
    seen.delete(id);
    descCache.set(id, total);
    return total;
  }

  // Roots: depth 0, OR any agent whose declared parent does not resolve (absent OR
  // flat) — promoted to a solo root so it still renders (honest, never dropped),
  // but never fabricated as a moon under a globe that cannot have one.
  const roots = agents
    .filter((a) => a.depth === 0 || !resolvesParent(a))
    .sort((x, y) => x.id.localeCompare(y.id));

  const maxRootR = roots.reduce((m, r) => Math.max(m, radarRadius(r.contextTokens, 0)), 0.34);

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

  // ── harness-sectored, subtree-weighted root placement ────────────────────────
  // Each root's angular slice (and a small radial push) scales with its descendant
  // count; roots are partitioned into per-harness sectors so harnesses don't
  // interleave. Sectors and roots-within-sector are deterministically ordered.
  const rootWeight = (r: RadarAgent): number => 1 + descendantCount(r.id);
  const weightSum = roots.reduce((s, r) => s + rootWeight(r), 0);
  const ring = rootRingRadius(roots.length, maxRootR, Math.max(roots.length, weightSum));

  // Group roots by harness key; order the harness sectors deterministically.
  const sectorMap = new Map<string, RadarAgent[]>();
  for (const r of roots) {
    const key = r.harness || '∅';
    const list = sectorMap.get(key) ?? [];
    list.push(r);
    sectorMap.set(key, list);
  }
  const sectorKeys = [...sectorMap.keys()].sort((a, b) => a.localeCompare(b));

  const placeRoot = (root: RadarAgent, angle: number) => {
    if (roots.length <= 1) {
      const node = makeNode(root, { x: 0, y: 0, z: 0 });
      nodes.push(node);
      placeChildren(root, node.position, node.radius);
      return;
    }
    // busy roots ride a touch farther out so their wider moon halo clears neighbours.
    const push = ring * (1 + Math.min(0.25, descendantCount(root.id) * 0.02));
    const center: Vec3 = {
      x: Math.cos(angle) * push,
      y: Math.sin(angle) * push * 0.34,
      z: Math.sin(angle) * push * 0.22,
    };
    const node = makeNode(root, center);
    nodes.push(node);
    placeChildren(root, center, node.radius);
  };

  // Walk the full circle, handing each harness sector an arc proportional to the
  // combined weight of its roots, and each root within it a slice proportional to
  // its own weight. Place every root at the centre of its slice.
  let cursor = -Math.PI / 2; // start at 12 o'clock, sweep clockwise (increasing angle)
  for (const key of sectorKeys) {
    const sectorRoots = sectorMap.get(key)!; // already in id order (roots was sorted)
    for (const root of sectorRoots) {
      const slice = (rootWeight(root) / Math.max(1, weightSum)) * Math.PI * 2;
      // Place the root at the centre of its slice: a wider slice (busier subtree)
      // therefore leaves more clear air to each neighbouring root.
      placeRoot(root, cursor + slice / 2);
      cursor += slice;
    }
  }

  return { nodes, links };
}
