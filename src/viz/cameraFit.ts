// cameraFit.ts — PURE camera-auto-fit math (no Three.js, unit-tested in isolation).
//
// The radar's "won't view nicely" complaint was framing: a fixed overview distance
// under- or over-shoots depending on how many folders/subagents are live. This module
// computes, from the live node positions alone, WHERE to look (the centroid) and how
// far back to sit so the whole fleet's bounding sphere lands in frame — accounting for
// the NARROWER of the vertical vs horizontal field of view, so a portrait window never
// clips the constellation on the sides.
//
// The distance law: a sphere of radius r exactly fills a symmetric frustum of
// half-angle a at distance r / sin(a) (the sightline is tangent to the sphere, so
// sin(a) = r / dist). We frame against min(vfov, hfov)/2 and inflate r by `margin`
// (> 1) for breathing room. Kept a pure function of primitives so it's trivially
// testable and callable from the render path without dragging in WebGL.

/**
 * Fallback framing distance for a fleet that can't drive an auto-fit (empty, or a
 * single zero-radius point). Matches CameraRig's OVERVIEW_DIST so the two agree on
 * "the default overview pull-back" — one number, two consumers.
 */
export const FIT_OVERVIEW_DIST = 12.6;

// A comfortable default headroom so the fleet lands with air around it, never flush
// to the frame edge. Callers may override per-context (e.g. a tighter subtree frame).
const DEFAULT_MARGIN = 1.18;

export type FitResult = {
  /** The point to look at — the centroid of the live nodes. */
  target: [number, number, number];
  /** Smallest camera-to-centroid distance that contains the bounding sphere in frame. */
  distance: number;
};

function isFinitePositive(n: number): boolean {
  return Number.isFinite(n) && n > 0;
}

/**
 * Frame a set of world-space points.
 *
 * @param points     live node centres (globe positions)
 * @param fovRadians the camera's VERTICAL field of view, in radians
 * @param aspect     viewport width / height
 * @param margin     radius inflation for headroom (default {@link DEFAULT_MARGIN}); > 1 = more air
 * @returns          `{ target: centroid, distance }`; empty/degenerate → overview fallback
 */
export function fitDistanceForBounds(
  points: { x: number; y: number; z: number }[],
  fovRadians: number,
  aspect: number,
  margin: number = DEFAULT_MARGIN,
): FitResult {
  if (!points || points.length === 0) {
    return { target: [0, 0, 0], distance: FIT_OVERVIEW_DIST };
  }

  // Centroid — the look-at target.
  let cx = 0;
  let cy = 0;
  let cz = 0;
  for (const p of points) {
    cx += p.x;
    cy += p.y;
    cz += p.z;
  }
  cx /= points.length;
  cy /= points.length;
  cz /= points.length;
  const target: [number, number, number] = [cx, cy, cz];

  // Bounding-sphere radius = farthest point from the centroid.
  let radius = 0;
  for (const p of points) {
    const dx = p.x - cx;
    const dy = p.y - cy;
    const dz = p.z - cz;
    const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
    if (d > radius) radius = d;
  }

  // A single point (or all-coincident points) → zero radius → can't drive a distance.
  // Look at it from the overview pull-back rather than diving to distance 0.
  const safeMargin = isFinitePositive(margin) ? margin : DEFAULT_MARGIN;
  if (!isFinitePositive(radius)) {
    return { target, distance: FIT_OVERVIEW_DIST };
  }

  // Vertical + horizontal half-angles; frame against the narrower one so neither axis
  // clips. hfov = 2·atan(tan(vfov/2)·aspect) is the standard perspective aspect widen.
  const vfov = isFinitePositive(fovRadians) ? fovRadians : (46 * Math.PI) / 180;
  const safeAspect = isFinitePositive(aspect) ? aspect : 1;
  const vHalf = vfov / 2;
  const hHalf = Math.atan(Math.tan(vHalf) * safeAspect);
  const half = Math.min(vHalf, hHalf);

  const sinHalf = Math.sin(half);
  // Guard the pathological half-angle (≤ 0 or non-finite) so we never divide by ~0.
  const distance = isFinitePositive(sinHalf)
    ? (radius * safeMargin) / sinHalf
    : FIT_OVERVIEW_DIST;

  return { target, distance: isFinitePositive(distance) ? distance : FIT_OVERVIEW_DIST };
}
