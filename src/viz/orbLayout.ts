import type { LayoutNode, OrbAgent, OrbIssue, OrbLayout, OrbSceneModel, Vec3 } from './orbTypes';

// Issue orb radius from persistence count, hub radius from total load. Both
// monotonic (more = bigger) and bounded so one outlier can't dominate.
function radiusFromCount(count: number): number {
  return 0.34 + Math.min(0.36, Math.sqrt(Math.max(1, count)) * 0.11);
}

function hubRadius(load: number): number {
  return 0.55 + Math.min(0.42, Math.sqrt(Math.max(0, load)) * 0.09);
}

// Golden angle — the azimuth step of a Fibonacci sphere, the most even way to
// scatter N points over a sphere without them lining up or clumping. Reused for
// both the issue shell (3D) and the hub phyllotaxis within a zone (2D disc).
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

// Shell radius so N issues, Fibonacci-spread, keep a clear gap between orbs. The
// nearest-neighbour spacing on a Fibonacci sphere is ≈ R·3.09/√N, so we invert
// that for the smallest R that guarantees surface-to-surface clearance. It grows
// with √N — a busy agent simply gets a bigger sphere, never a crowded one — and
// is floored so issues always clear the hub globe at the centre. The √N law is
// also the minimum-angular-gap guarantee: more issues ⇒ bigger shell ⇒ the same
// arc-gap between neighbours, so high-count agents fan out instead of bunching.
function shellRadius(count: number, maxIssueR: number, hubR: number): number {
  const clearance = hubR + maxIssueR + 0.9;
  if (count <= 1) return Math.max(1.9, clearance);
  const need = maxIssueR * 2 + 0.9; // wanted gap between two neighbouring orbs
  const fromSpacing = (need * Math.sqrt(count)) / 2.3; // invert R·3.09/√N, with safety
  return Math.max(2.0, clearance, fromSpacing);
}

// Fibonacci-sphere placement. Issues arrive worst-first, so index 0 lands at the
// top of the shell and index N-1 at the bottom: severity maps straight to
// latitude (critical up top, calm below), same-severity issues share a band, and
// the golden-angle azimuth fans every issue a different way in true 3D — never
// one flat plane.
function shellPosition(center: Vec3, index: number, total: number, R: number): Vec3 {
  const denom = Math.max(1, total);
  const y = 1 - ((index + 0.5) / denom) * 2; // +1 (top) .. -1 (bottom)
  const rXZ = Math.sqrt(Math.max(0, 1 - y * y));
  const theta = GOLDEN_ANGLE * index;
  return {
    x: center.x + R * Math.cos(theta) * rXZ,
    y: center.y + R * y,
    z: center.z + R * Math.sin(theta) * rXZ,
  };
}

// ─── Harness zoning ──────────────────────────────────────────────────────────
// The constellation reads as a grammar: zone(harness) → agent cluster → issue
// severity. Zones are laid on a shallow camera-facing arc so the harnesses sit
// decodably side-by-side instead of receding into one long front-to-back line.

// Stable harness order so the arc is deterministic and the primary harness leads.
const HARNESS_ORDER = ['claude_code', 'codex', 'unknown'];
function harnessRank(h: string): number {
  const i = HARNESS_ORDER.indexOf(h);
  return i === -1 ? HARNESS_ORDER.length : i;
}

// Packing geometry.
const HUB_PHYLLO_SPACING = 1.18; // disc growth per √index — sets cluster density
const HUB_FOOTPRINT_MARGIN = 0.5; // breathing room added around each agent's reach
const SEP_ITERS = 24; // bounded pairwise separation pass (brief: ≤ 24)
const SEP_MAX_STEP = 0.6; // clamp on per-iteration displacement (stability)
const SEP_EPS = 1e-4; // treat sub-epsilon overlap as resolved
const ZONE_GAP = 4.0; // empty lateral space between adjacent zones
const ARC_DEPTH = 2.2; // peak Z pull-back of the shallow camera-facing arc

type PackedAgent = {
  agent: OrbAgent;
  issues: OrbIssue[];
  shell: number;
  hubR: number;
  maxIssueR: number;
  foot: number; // footprint radius: the agent's whole reach (hub + issue shell)
  local: { x: number; y: number }; // zone-local hub centre (camera-facing plane)
};

type Zone = {
  harness: string;
  members: PackedAgent[];
  halfWidth: number; // lateral half-extent of the packed cluster (+footprints)
};

// Resolve one agent into its issue set (worst-first) and footprint reach.
function resolveAgent(agent: OrbAgent, scene: OrbSceneModel): PackedAgent {
  const issues = scene.issues
    .filter((i) => i.agentId === agent.id)
    .sort((a, b) => b.severity - a.severity || b.count - a.count || a.id.localeCompare(b.id));
  const maxIssueR = issues.reduce((m, i) => Math.max(m, radiusFromCount(i.count)), 0.4);
  const hubR = hubRadius(agent.totalLoad);
  const shell = shellRadius(issues.length, maxIssueR, hubR);
  // Footprint = the farthest any orb of this agent reaches from its hub centre.
  // For ≥1 issue that is the shell radius + the issue orb radius; with no issues
  // it is just the hub globe. Margin guarantees a visible gap between agents and
  // — because two agents are kept ≥ foot_i + foot_j apart — provably prevents any
  // issue of one agent from touching any orb of another.
  const reach = (issues.length ? shell + maxIssueR : hubR) + HUB_FOOTPRINT_MARGIN;
  return { agent, issues, shell, hubR, maxIssueR, foot: reach, local: { x: 0, y: 0 } };
}

// Golden-angle phyllotaxis seeds + bounded pairwise separation. Packs agent hubs
// into a compact, collision-free disc in the zone's local (camera-facing) plane.
// Deterministic: members arrive in a fixed (sorted) order and there is no RNG.
// Returns the cluster's lateral half-width (footprints included).
function packZone(members: PackedAgent[]): number {
  const n = members.length;
  if (n === 0) return 0;
  if (n === 1) {
    members[0].local = { x: 0, y: 0 };
    return members[0].foot;
  }

  // Seed: golden-angle spiral. Radius scales with √index and with the mean
  // footprint so denser/bigger agents start farther apart — fewer separation
  // iterations needed, and the spiral never starts inside itself.
  const meanFoot = members.reduce((s, m) => s + m.foot, 0) / n;
  const spread = HUB_PHYLLO_SPACING * Math.max(1, meanFoot);
  members.forEach((m, i) => {
    const r = spread * Math.sqrt(i + 0.5);
    const a = i * GOLDEN_ANGLE;
    m.local = { x: r * Math.cos(a), y: r * Math.sin(a) };
  });

  // Bounded pairwise separation: while any two footprints overlap, push the pair
  // apart along their centre-line by half the overlap each, clamped per step.
  for (let iter = 0; iter < SEP_ITERS; iter++) {
    let moved = false;
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const a = members[i].local;
        const b = members[j].local;
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let d = Math.hypot(dx, dy);
        const minD = members[i].foot + members[j].foot;
        if (d >= minD - SEP_EPS) continue;
        // Degenerate coincidence → deterministic split along a fixed axis derived
        // from the index (no RNG), so two stacked seeds always part the same way.
        if (d < SEP_EPS) {
          dx = Math.cos(i + j);
          dy = Math.sin(i + j);
          d = Math.hypot(dx, dy) || 1;
        }
        const overlap = minD - d;
        const step = Math.min(SEP_MAX_STEP, overlap / 2);
        const ux = dx / d;
        const uy = dy / d;
        a.x -= ux * step;
        a.y -= uy * step;
        b.x += ux * step;
        b.y += uy * step;
        moved = true;
      }
    }
    if (!moved) break;
  }

  // Centre the cluster on its own centroid and measure its lateral half-width
  // (each hub's footprint included, so the zone bounds enclose every issue orb).
  let cx = 0;
  let cy = 0;
  for (const m of members) {
    cx += m.local.x;
    cy += m.local.y;
  }
  cx /= n;
  cy /= n;
  let halfWidth = 0;
  for (const m of members) {
    m.local.x -= cx;
    m.local.y -= cy;
    halfWidth = Math.max(halfWidth, Math.abs(m.local.x) + m.foot);
  }
  return halfWidth;
}

export function layoutOrbScene(scene: OrbSceneModel): OrbLayout {
  // Hub→issue links pass through untouched (Constellation reads source/target/
  // kind only). Filtering to agent_issue keeps the contract.
  const links = scene.links.filter((link) => link.kind === 'agent_issue');

  // ── Group agents into harness zones (only non-empty), deterministic order ──
  const byHarness = new Map<string, OrbAgent[]>();
  for (const agent of scene.agents) {
    if (!byHarness.has(agent.harness)) byHarness.set(agent.harness, []);
    byHarness.get(agent.harness)!.push(agent);
  }
  const harnesses = [...byHarness.keys()].sort(
    (a, b) => harnessRank(a) - harnessRank(b) || a.localeCompare(b),
  );

  const zones: Zone[] = harnesses.map((harness) => {
    const members = byHarness
      .get(harness)!
      .slice()
      .sort((a, b) => a.id.localeCompare(b.id))
      .map((agent) => resolveAgent(agent, scene));
    const halfWidth = packZone(members);
    return { harness, members, halfWidth };
  });

  // ── Lay zones left→right with a gap, then bow them onto a camera-facing arc ──
  // Total lateral footprint is the sum of zone widths + inter-zone gaps; the √N
  // hub packing keeps each zone compact, so the whole scene stays bounded
  // instead of stretching into a long receding line. Laying zones by cumulative
  // half-widths + ZONE_GAP makes their X intervals disjoint by construction.
  const totalWidth =
    zones.reduce((sum, z) => sum + z.halfWidth * 2, 0) + ZONE_GAP * Math.max(0, zones.length - 1);
  const halfSpan = totalWidth / 2;

  // Shallow camera-facing arc. Each zone gets an angle stepped monotonically by
  // its order along the arc (φ: 0 → φ_max), pulled back by z = -k·(1 - cos φ).
  // Monotonic (not symmetric) so two zones never share a depth — the formation
  // sweeps continuously toward the camera like a fanned hand of cards. Depths are
  // re-centred about their mean so the arc stays balanced around z = 0, and the
  // curvature is mild (k = ARC_DEPTH) so lateral X always dominates the depth
  // offset — the zones read side-by-side, never stacked front-to-back.
  const lastZone = Math.max(1, zones.length - 1);
  const rawZoneZ = zones.map((_, zi) => {
    const phi = (zi / lastZone) * (Math.PI / 2); // 0 .. π/2 across the zones
    return -ARC_DEPTH * (1 - Math.cos(phi));
  });
  const meanZ = rawZoneZ.reduce((a, b) => a + b, 0) / rawZoneZ.length;

  const nodes: LayoutNode[] = [];
  let cursor = -halfSpan;
  zones.forEach((zone, zi) => {
    const centerX = cursor + zone.halfWidth; // zone centre on the lateral axis
    cursor += zone.halfWidth * 2 + ZONE_GAP;
    const zoneCenter: Vec3 = { x: centerX, y: 0, z: rawZoneZ[zi] - meanZ };

    for (const m of zone.members) {
      // Zone-local plane is the camera-facing X–Y; the arc only tilts Z. A hub's
      // world position is the zone centre + its local (x, y) offset. Because the
      // separation pass keeps hubs ≥ foot+foot apart and each footprint bounds
      // the agent's whole issue sphere, no two agents' orbs can ever intersect.
      const hubCenter: Vec3 = {
        x: zoneCenter.x + m.local.x,
        y: zoneCenter.y + m.local.y,
        z: zoneCenter.z,
      };

      nodes.push({
        id: m.agent.id,
        kind: 'hub',
        position: hubCenter,
        radius: m.hubR,
        agentId: m.agent.id,
        harness: m.agent.harness,
        agent: m.agent,
        territoryRadius: m.foot,
      });

      m.issues.forEach((issue, index) => {
        nodes.push({
          id: issue.id,
          kind: 'issue',
          position: shellPosition(hubCenter, index, m.issues.length, m.shell),
          radius: radiusFromCount(issue.count),
          agentId: m.agent.id,
          harness: issue.harness,
          issue,
        });
      });
    }
  });

  return { nodes, links };
}
