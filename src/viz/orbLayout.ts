import type { LayoutNode, OrbLayout, OrbSceneModel, Vec3 } from './orbTypes';

// Issue orb radius from persistence count, hub radius from total load. Both
// monotonic (more = bigger) and bounded so one outlier can't dominate.
function radiusFromCount(count: number): number {
  return 0.34 + Math.min(0.36, Math.sqrt(Math.max(1, count)) * 0.11);
}

function hubRadius(load: number): number {
  return 0.55 + Math.min(0.42, Math.sqrt(Math.max(0, load)) * 0.09);
}

// Golden angle — the azimuth step of a Fibonacci sphere, the most even way to
// scatter N points over a sphere without them lining up or clumping.
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

// Shell radius so N issues, Fibonacci-spread, keep a clear gap between orbs. The
// nearest-neighbour spacing on a Fibonacci sphere is ≈ R·3.09/√N, so we invert
// that for the smallest R that guarantees surface-to-surface clearance. It grows
// with √N — a busy agent simply gets a bigger sphere, never a crowded one — and
// is floored so issues always clear the hub globe at the centre.
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

export function layoutOrbScene(scene: OrbSceneModel): OrbLayout {
  const agents = [...scene.agents].sort((a, b) => a.id.localeCompare(b.id));
  const links = scene.links.filter((link) => link.kind === 'agent_issue');

  // Pass 1 — resolve each agent's issues (worst first) and the cluster's extent.
  const clusters = agents.map((agent) => {
    const issues = scene.issues
      .filter((i) => i.agentId === agent.id)
      .sort((a, b) => b.severity - a.severity || b.count - a.count || a.id.localeCompare(b.id));
    const maxIssueR = issues.reduce((m, i) => Math.max(m, radiusFromCount(i.count)), 0.4);
    const shell = shellRadius(issues.length, maxIssueR, hubRadius(agent.totalLoad));
    const extent = (issues.length ? shell : hubRadius(agent.totalLoad)) + maxIssueR + 0.6;
    return { agent, issues, shell, extent };
  });

  // Pass 2 — lay clusters out left→right, each separated by its OWN extent (plus
  // a gap), so two busy clusters can never overlap however many issues they hold.
  const GAP = 2.4;
  const totalWidth = clusters.reduce((sum, c) => sum + c.extent * 2, 0) + GAP * Math.max(0, clusters.length - 1);
  let cursor = -totalWidth / 2;

  const nodes: LayoutNode[] = [];
  clusters.forEach((c, ci) => {
    const cx = cursor + c.extent;
    cursor += c.extent * 2 + GAP;
    const center: Vec3 = { x: cx, y: 0, z: ci % 2 === 0 ? -0.25 : 0.25 };

    nodes.push({
      id: c.agent.id,
      kind: 'hub',
      position: center,
      radius: hubRadius(c.agent.totalLoad),
      agentId: c.agent.id,
      harness: c.agent.harness,
      agent: c.agent,
      territoryRadius: c.extent,
    });

    c.issues.forEach((issue, index) => {
      nodes.push({
        id: issue.id,
        kind: 'issue',
        position: shellPosition(center, index, c.issues.length, c.shell),
        radius: radiusFromCount(issue.count),
        agentId: c.agent.id,
        harness: issue.harness,
        issue,
      });
    });
  });

  return { nodes, links };
}
