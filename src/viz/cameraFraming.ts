import type { RadarAgent } from './radarTypes';

export interface Bounds {
  center: [number, number, number];
  radius: number;
}

/**
 * Given a bounding sphere radius and a camera vertical/horizontal FOV in degrees,
 * returns the camera-to-center distance that fills `fill` fraction of the frame.
 *
 * Formula: r / (tan(fov_rad / 2) * fill)
 */
export function frameDistance(
  boundingRadius: number,
  fovDeg: number,
  fill = 0.6,
): number {
  return boundingRadius / (Math.tan(((fovDeg * Math.PI) / 180) / 2) * fill);
}

/**
 * Computes the enclosing sphere for `rootId` and all its transitive descendants.
 *
 * - Builds a children map from `agents[].parentId`.
 * - BFS from `rootId` to collect the subtree member ids.
 * - Skips any id absent from `positions`.
 * - Center = mean of member centers.
 * - Radius = max over members of (distance(memberCenter, center) + memberRadius).
 */
export function subtreeBounds(
  positions: Map<string, { pos: [number, number, number]; radius: number }>,
  agents: RadarAgent[],
  rootId: string,
): Bounds {
  // Build children map
  const children = new Map<string, string[]>();
  for (const agent of agents) {
    if (!children.has(agent.id)) children.set(agent.id, []);
    if (agent.parentId !== null) {
      if (!children.has(agent.parentId)) children.set(agent.parentId, []);
      children.get(agent.parentId)!.push(agent.id);
    }
  }

  // BFS from rootId
  const memberIds: string[] = [];
  const queue: string[] = [rootId];
  const visited = new Set<string>();
  while (queue.length > 0) {
    const id = queue.shift()!;
    if (visited.has(id)) continue;
    visited.add(id);
    memberIds.push(id);
    const kids = children.get(id) ?? [];
    for (const child of kids) {
      if (!visited.has(child)) queue.push(child);
    }
  }

  // Filter to only members present in positions
  const members = memberIds
    .map((id) => ({ id, entry: positions.get(id) }))
    .filter((m): m is { id: string; entry: { pos: [number, number, number]; radius: number } } =>
      m.entry !== undefined,
    );

  // Leaf / single member: return its own bounds
  if (members.length === 1) {
    return { center: members[0].entry.pos, radius: members[0].entry.radius };
  }

  // Center = mean of member positions
  let cx = 0, cy = 0, cz = 0;
  for (const { entry } of members) {
    cx += entry.pos[0];
    cy += entry.pos[1];
    cz += entry.pos[2];
  }
  cx /= members.length;
  cy /= members.length;
  cz /= members.length;

  // Radius = max(dist(memberCenter, center) + memberRadius)
  let r = 0;
  for (const { entry } of members) {
    const dx = entry.pos[0] - cx;
    const dy = entry.pos[1] - cy;
    const dz = entry.pos[2] - cz;
    const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);
    r = Math.max(r, dist + entry.radius);
  }

  return { center: [cx, cy, cz], radius: r };
}
