import type { RadarAgent } from '@/viz/shared/types/radarTypes';

export interface Bounds {
  center: [number, number, number];
  radius: number;
}

/**
 * Given a bounding sphere radius and a camera VERTICAL FOV in degrees, returns the
 * camera-to-center distance that fills `fill` fraction of the frame.
 *
 * Formula: r / (tan(fov_rad / 2) * fill)
 *
 * `aspect` (width / height) frames against the TIGHTER of the two frustums. three.js
 * `PerspectiveCamera.fov` is vertical only, so the horizontal frustum narrows with the
 * window: on a portrait or simply narrow window (aspect < 1) the vertical fit alone
 * leaves the constellation's wide rail layout clipped off the left and right edges.
 * Since tan(hFov/2) = tan(vFov/2) * aspect, clamping the effective half-angle by
 * min(1, aspect) yields whichever distance actually contains the sphere. Omitting
 * `aspect` keeps the original vertical-only behaviour.
 */
export function frameDistance(
  boundingRadius: number,
  fovDeg: number,
  fill = 0.6,
  aspect?: number,
): number {
  const halfV = Math.tan(((fovDeg * Math.PI) / 180) / 2);
  // A non-finite or non-positive aspect (a 0-height canvas mid-resize) must not divide
  // by zero or NaN the camera into oblivion, so fall back to vertical-only framing.
  const tighten = aspect !== undefined && Number.isFinite(aspect) && aspect > 0
    ? Math.min(1, aspect)
    : 1;
  return boundingRadius / (halfV * fill * tighten);
}

// ── framing inside the DOM's free channel ────────────────────────────────────
// The canvas fills the window, but the chrome does not: a left rail is always
// mounted and a right rail opens on selection, each `var(--rail-w)` wide. Framing
// against the full viewport therefore parks part of the constellation UNDER a panel.
// These three pure helpers move the fit into the free channel between the rails:
// `channelWidth` narrows the aspect fed to `frameDistance` (so the fit accounts for
// the width actually visible), and `channelShiftPx` + `pixelsToWorld` truck the
// camera sideways so the scene lands in the middle of that channel rather than the
// middle of the window. All inputs are CSS pixels, matching `useThree().size`.

/** Reserved chrome on each side of the canvas, in CSS pixels. */
export type RailInsets = { left: number; right: number };

/** Visible width between the reserved rails, floored at 1 so it never divides to 0. */
export function channelWidth(viewportWidth: number, insets: RailInsets): number {
  const w = Number.isFinite(viewportWidth) ? viewportWidth : 0;
  const left = Number.isFinite(insets.left) ? Math.max(0, insets.left) : 0;
  const right = Number.isFinite(insets.right) ? Math.max(0, insets.right) : 0;
  return Math.max(1, w - left - right);
}

/**
 * Signed CSS pixels to move the CAMERA along its own right vector so the scene
 * appears centred in the free channel instead of the window.
 *
 * The channel's midpoint sits `(left - right) / 2` px right of the window's midpoint,
 * and moving the camera right pushes the scene left, so the camera moves the opposite
 * way: `(right - left) / 2`. A left rail alone (right = 0) therefore returns a
 * negative shift, dollying the camera left so the constellation slides right, clear
 * of the panel. Equal rails cancel to 0, as does no chrome at all.
 */
export function channelShiftPx(insets: RailInsets): number {
  const left = Number.isFinite(insets.left) ? Math.max(0, insets.left) : 0;
  const right = Number.isFinite(insets.right) ? Math.max(0, insets.right) : 0;
  return (right - left) / 2;
}

/**
 * Convert a screen offset in CSS pixels to world units at `distance` from the camera.
 *
 * The frustum is `2 * distance * tan(fov/2)` world units tall and `viewportHeight`
 * pixels tall, so one pixel is their ratio. `fovDeg` is three.js's VERTICAL fov, which
 * is why height (never width) is the denominator: the vertical mapping is the only one
 * that holds regardless of aspect. Degenerate inputs return 0 rather than NaN, so a
 * mid-resize frame nudges the camera nowhere instead of into oblivion.
 */
export function pixelsToWorld(
  px: number,
  distance: number,
  fovDeg: number,
  viewportHeight: number,
): number {
  if (!Number.isFinite(px) || !Number.isFinite(distance) || !Number.isFinite(fovDeg)) return 0;
  if (!Number.isFinite(viewportHeight) || viewportHeight <= 0) return 0;
  const frustumHeight = 2 * distance * Math.tan(((fovDeg * Math.PI) / 180) / 2);
  return (px * frustumHeight) / viewportHeight;
}

/**
 * Enclosing sphere for an ENTIRE laid-out forest — every node, regardless of
 * hierarchy. Centre is the midpoint of the axis-aligned extent; radius is the
 * farthest node surface from that centre. Returns null for an empty set.
 *
 * The camera uses this to scale its zoom-out range and overview framing to
 * however large the constellation actually is, instead of a fixed cage.
 */
export function enclosingBounds(
  points: { pos: [number, number, number]; radius: number }[],
): Bounds | null {
  if (points.length === 0) return null;
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (const { pos, radius } of points) {
    minX = Math.min(minX, pos[0] - radius);
    minY = Math.min(minY, pos[1] - radius);
    minZ = Math.min(minZ, pos[2] - radius);
    maxX = Math.max(maxX, pos[0] + radius);
    maxY = Math.max(maxY, pos[1] + radius);
    maxZ = Math.max(maxZ, pos[2] + radius);
  }
  const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2, cz = (minZ + maxZ) / 2;
  let radius = 0;
  for (const { pos, radius: r } of points) {
    const d = Math.hypot(pos[0] - cx, pos[1] - cy, pos[2] - cz) + r;
    if (d > radius) radius = d;
  }
  return { center: [cx, cy, cz], radius };
}

/** Axis-aligned extent of a laid-out forest. Null for an empty set. */
export type Box = { min: [number, number, number]; max: [number, number, number] };

/**
 * Axis-aligned bounding box of a laid-out forest, node surfaces included.
 *
 * The enclosing SPHERE is what the camera frames with, but anything that has to be
 * drawn AROUND a constellation (a border, a backdrop) needs its real proportions: the
 * abacus board is far wider than it is tall, so a square sized off the sphere radius
 * would tower over it with dead space above and below.
 */
export function enclosingBox(
  points: { pos: [number, number, number]; radius: number }[],
): Box | null {
  if (points.length === 0) return null;
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (const { pos, radius } of points) {
    minX = Math.min(minX, pos[0] - radius);
    minY = Math.min(minY, pos[1] - radius);
    minZ = Math.min(minZ, pos[2] - radius);
    maxX = Math.max(maxX, pos[0] + radius);
    maxY = Math.max(maxY, pos[1] + radius);
    maxZ = Math.max(maxZ, pos[2] + radius);
  }
  return { min: [minX, minY, minZ], max: [maxX, maxY, maxZ] };
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
