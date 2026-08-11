// peerFraming.ts: where somebody else's constellation sits in OUR world.
//
// Watching a peer's swarm is a slide, not a swap. Both constellations exist in the
// scene at the same time, side by side on X, and the camera trucks laterally between
// them (see PeerConstellation.tsx and CameraRig's `viewTarget`). That only works if
// the gap between them is derived from how big they actually are: a fixed offset that
// looks generous against two agents will have a forty-agent board growing straight
// through its neighbour, and the two would interleave with no visible seam.
//
// So the gap is a FRACTION OF THE COMBINED RADII. Both boards scale, the channel
// between them scales with them, and they can never touch at any size.
//
// Pure (layout math only, no Three.js), so the no-overlap guarantee is unit-tested.

import type { RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { enclosingBounds, enclosingBox, type Bounds, type Box } from '@/viz/shared/scene/cameraFraming';
import { layoutRadarScene, type RadarLayout } from './radarLayout';

/** Empty air between the two bounding spheres, as a fraction of their combined radii. */
const DEFAULT_PAD_FRACTION = 0.4;

/**
 * A peer board's layout, laid out once and shared.
 *
 * Three separate call sites here and in `PeerConstellation` need the peer's nodes
 * (the placement, the hazard frame's box, and the forest itself), and each used to
 * call `layoutRadarScene` for itself. That is the same pure function over the same
 * model three times per emit, all of it inside the frame budget of a live scene, so
 * the lead computes it once and threads it through.
 */
export function peerLayout(model: RadarSceneModel): RadarLayout {
  return layoutRadarScene(model);
}

/** Where a peer constellation goes: its world X translation, and its bounds once moved. */
export type PeerPlacement = {
  /** World-X translation to apply to the peer's group. 0 when there is no peer. */
  offsetX: number;
  /** The peer's bounding sphere IN WORLD SPACE (offset applied), for camera framing. */
  bounds: Bounds | null;
};

function layoutPoints(model: RadarSceneModel, layout?: RadarLayout) {
  return (layout ?? layoutRadarScene(model)).nodes.map((n) => ({
    pos: [n.position.x, n.position.y, n.position.z] as [number, number, number],
    radius: n.radius,
  }));
}

/** Bounding sphere of a peer model's own layout, before it is moved anywhere. */
export function peerLayoutBounds(model: RadarSceneModel, layout?: RadarLayout): Bounds | null {
  return enclosingBounds(layoutPoints(model, layout));
}

/**
 * Axis-aligned extent of a peer model's own layout, in the peer group's LOCAL space
 * (so the caller draws the hazard frame around it without undoing `offsetX`).
 */
export function peerLayoutBox(model: RadarSceneModel, layout?: RadarLayout): Box | null {
  return enclosingBox(layoutPoints(model, layout));
}

/**
 * World-X translation that parks `peer` to the RIGHT of `local` with clear air between
 * them. The peer's left edge lands one gap past the local board's right edge, where the
 * gap is `padFraction * (localRadius + peerRadius)`.
 *
 * A missing local board is treated as a zero-radius point at the origin, so the very
 * first peer opened against an empty local forest still lands beside it, not on it.
 */
export function peerOffsetX(
  local: Bounds | null,
  peer: Bounds | null,
  padFraction = DEFAULT_PAD_FRACTION,
): number {
  if (!peer) return 0;
  const localX = local ? local.center[0] : 0;
  const localR = local ? Math.max(0, local.radius) : 0;
  const peerR = Math.max(0, peer.radius);
  const gap = Math.max(0, padFraction) * (localR + peerR);
  return localX + localR + gap + peerR - peer.center[0];
}

/**
 * Full placement for a peer model beside the local board: lay the peer out, measure
 * it, offset it clear of `localBounds`, and report the moved bounds so the camera can
 * frame the peer without re-deriving any of it. One computation, one source of truth,
 * shared by the constellation (which uses `offsetX`) and the rig (which uses `bounds`).
 */
export function peerWorldPlacement(
  model: RadarSceneModel | null,
  localBounds: Bounds | null,
  padFraction = DEFAULT_PAD_FRACTION,
  layout?: RadarLayout,
): PeerPlacement {
  if (!model) return { offsetX: 0, bounds: null };
  const own = peerLayoutBounds(model, layout);
  if (!own) return { offsetX: 0, bounds: null };
  const offsetX = peerOffsetX(localBounds, own, padFraction);
  return {
    offsetX,
    bounds: { center: [own.center[0] + offsetX, own.center[1], own.center[2]], radius: own.radius },
  };
}

/**
 * Enclosing sphere of BOTH boards. The rig uses it for the far plane and the dolly
 * ceiling, so pulling all the way back still shows the local forest and the peer's at
 * once, whichever one is currently framed.
 */
export function unionBounds(a: Bounds | null, b: Bounds | null): Bounds | null {
  if (!a) return b;
  if (!b) return a;
  const dx = b.center[0] - a.center[0];
  const dy = b.center[1] - a.center[1];
  const dz = b.center[2] - a.center[2];
  const d = Math.hypot(dx, dy, dz);
  // One sphere already swallows the other: nothing to grow.
  if (d + b.radius <= a.radius) return a;
  if (d + a.radius <= b.radius) return b;
  const radius = (d + a.radius + b.radius) / 2;
  // Walk from a's centre toward b's by however far the new centre has to move.
  const k = d > 0 ? (radius - a.radius) / d : 0;
  return {
    center: [a.center[0] + dx * k, a.center[1] + dy * k, a.center[2] + dz * k],
    radius,
  };
}
