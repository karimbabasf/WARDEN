import type { LayoutNode, OrbLayout, OrbSceneModel, Vec3 } from './orbTypes';

// Issue orb radius from persistence count, hub radius from total load. Both
// monotonic (more = bigger) and bounded so one outlier can't dominate.
function radiusFromCount(count: number): number {
  return 0.34 + Math.min(0.36, Math.sqrt(Math.max(1, count)) * 0.11);
}

function hubRadius(load: number): number {
  return 0.55 + Math.min(0.42, Math.sqrt(Math.max(0, load)) * 0.09);
}

// DYNAMIC spacing: the satellite ring grows with the issue count so orbs never
// crowd — there must be enough circumference for each orb plus a clear gap. A
// sensible floor keeps small clusters from collapsing onto the hub.
function ringRadius(count: number, maxIssueR: number): number {
  if (count <= 1) return 1.5;
  const circumferenceNeed = (count * (maxIssueR * 2 + 0.55)) / (2 * Math.PI);
  return Math.max(1.6, circumferenceNeed + maxIssueR * 0.5);
}

// Satellites sit on a tilted ring. Starting at 12 o'clock and going clockwise,
// combined with a severity-desc sort, puts the worst issues up top so the spread
// reads the same way every time.
function satellitePosition(center: Vec3, index: number, total: number, ring: number): Vec3 {
  const angle = -Math.PI / 2 + (index / Math.max(1, total)) * Math.PI * 2;
  return {
    x: center.x + Math.cos(angle) * ring,
    y: center.y + Math.sin(angle) * ring * 0.62,
    z: center.z + Math.sin(angle) * ring * 0.5,
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
    const ring = ringRadius(issues.length, maxIssueR);
    const extent = (issues.length ? ring : hubRadius(agent.totalLoad)) + maxIssueR + 0.4;
    return { agent, issues, ring, extent };
  });

  // Pass 2 — lay clusters out left→right, each separated by its OWN extent (plus
  // a gap), so two busy clusters can never overlap however many issues they hold.
  const GAP = 1.6;
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
        position: satellitePosition(center, index, c.issues.length, c.ring),
        radius: radiusFromCount(issue.count),
        agentId: c.agent.id,
        harness: issue.harness,
        issue,
      });
    });
  });

  return { nodes, links };
}
