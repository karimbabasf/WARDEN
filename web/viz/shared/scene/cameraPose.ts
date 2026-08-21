// cameraPose.ts: the camera's motion core, a pose and a spring that carries it.
//
// WHY THIS EXISTS. The rig used to interpolate with a fixed-duration `easeInOutExpo`
// clock, and it read as glitchy for one reason: an expo ease STARTS AND ENDS AT ZERO
// VELOCITY, so every re-target was a visible dead stop. A single click produced three
// of them, because three separate things re-aimed the camera in the space of a few
// frames:
//
//   1. the selection effect wrote a "cosy dive" pose and started a damped glide;
//   2. one commit later the focus stack resolved and `focusBounds` wrote a DIFFERENT
//      pose (a subtree fit, ~1.7 world units further out on a leaf) and restarted the
//      clock from t=0;
//   3. a frame after that the right rail finished measuring, both effects re-ran on
//      the changed insets, and the clock restarted from t=0 again.
//
// Then, for as long as you stayed dived in, every `radar_state` emit rebuilt the
// layout, `subtreeBounds` returned centres that differed in the twelfth decimal, the
// un-rounded focus key changed, and the camera re-flew a 700ms ease once a second.
//
// A spring fixes the whole class. It has no clock and no start pose: it has a POSITION
// and a VELOCITY, and re-aiming it mid-flight bends the path instead of restarting it.
// Point 3 stops being a stall and becomes a curve; point 2 stops mattering at all once
// the two poses are unified (see `resolvePose` in CameraRig); point 1 disappears
// because there is now one writer.
//
// Three decisions inside the pose are worth stating:
//
// * **The pose is (target, direction, distance), not (target, position).** Lerping two
//   world positions while the distance changes swings the camera along a chord, so the
//   scene slides sideways during a straight dolly. Splitting the dolly out keeps a
//   push-in a push-in.
// * **Distance springs in LOG space.** Overview to dive is 12.6 to 5.0, a 2.5x ratio.
//   Linear distance spends the back half of that crawling; log space makes the
//   apparent zoom rate uniform, which is what "smooth" means for a dolly.
// * **The direction is frozen at re-target time**, not re-derived per frame. Every
//   move in this rig preserves your viewing angle, and re-deriving it from a moving
//   camera is a feedback loop.
//
// Pure module: no React, no three.js, no DOM. Unit-tested in cameraPose.test.ts.

import { spring, springStep, type Spring } from '@/viz/shared/lib/spring';

/** Where the camera is aimed, in the form the spring integrates. */
export type Pose = {
  /** Orbit pivot in world space. */
  target: [number, number, number];
  /** Unit vector from the pivot TOWARD the camera. */
  dir: [number, number, number];
  /** Camera-to-pivot distance in world units. Always > 0. */
  distance: number;
};

/**
 * Spring state for one pose: three springs on the pivot, three on the direction, one
 * on log(distance). Seven scalars, each carrying its own velocity, so a re-target in
 * any one of them leaves the other six alone.
 */
export type PoseSpring = {
  tx: Spring; ty: Spring; tz: Spring;
  dx: Spring; dy: Spring; dz: Spring;
  /** Natural log of the distance (see the header). */
  logDist: Spring;
};

/**
 * How long a camera move takes to arrive, in seconds, and how hard it lands.
 *
 * 0.52s is a hair quicker than the 700ms ease it replaces, and reads quicker still
 * because a spring is at its fastest immediately (an expo ease spends its first
 * ~150ms almost stationary). Damping is exactly 1: a camera that overshoots its
 * subject reads as a mistake, not as life, however much bounce suits a toast.
 */
export const POSE_RESPONSE = 0.52;
export const POSE_DAMPING = 1;

/**
 * A quicker response for a move the operator did not ask for: an agent terminating
 * under the camera, or the board re-fitting as globes arrive. An unrequested move
 * should be over before it becomes something to watch.
 */
export const POSE_RESPONSE_QUIET = 0.38;

/** World units: closer than this to the goal, with the velocity gone, is arrived. */
const SETTLE_POS = 0.002;
const SETTLE_VEL = 0.02;

function len3(v: [number, number, number]): number {
  return Math.hypot(v[0], v[1], v[2]);
}

/**
 * Unit-length copy of `v`, or `fallback` when `v` is degenerate.
 *
 * Degenerate is not hypothetical: the direction is read as (camera - pivot), and those
 * two coincide for one frame whenever a pose is written before the controls mount.
 */
export function normalize3(
  v: [number, number, number],
  fallback: [number, number, number] = [0, 0, 1],
): [number, number, number] {
  const l = len3(v);
  if (!Number.isFinite(l) || l < 1e-6) return [...fallback];
  return [v[0] / l, v[1] / l, v[2] / l];
}

/** A pose spring sitting exactly on `pose` with no velocity. */
export function poseSpring(pose: Pose): PoseSpring {
  const d = normalize3(pose.dir);
  return {
    tx: spring(pose.target[0]),
    ty: spring(pose.target[1]),
    tz: spring(pose.target[2]),
    dx: spring(d[0]),
    dy: spring(d[1]),
    dz: spring(d[2]),
    logDist: spring(Math.log(Math.max(1e-3, pose.distance))),
  };
}

/** The pose a spring currently holds, direction renormalized. */
export function poseOf(s: PoseSpring): Pose {
  return {
    target: [s.tx.value, s.ty.value, s.tz.value],
    dir: normalize3([s.dx.value, s.dy.value, s.dz.value]),
    distance: Math.exp(s.logDist.value),
  };
}

/**
 * Advance `s` toward `goal` by `dt` seconds.
 *
 * Every axis is stepped independently and NONE of them is reset, which is the entire
 * point: calling this with a different `goal` than last frame is not a new animation,
 * it is the same animation aimed somewhere else.
 *
 * The direction is stepped as three loose components and renormalized on read
 * (`poseOf`). A true slerp would be more correct for a half-turn, but every move this
 * rig makes preserves the viewing angle or changes it by a few degrees, and a
 * component spring keeps per-axis velocity where a slerp would need its own.
 */
export function poseSpringStep(
  s: PoseSpring,
  goal: Pose,
  dt: number,
  response: number = POSE_RESPONSE,
  damping: number = POSE_DAMPING,
): PoseSpring {
  const g = normalize3(goal.dir);
  // Take the direction goal on whichever side of the sphere the spring is already on.
  // Without this, a goal direction that is the near-antipode of the current one drags
  // every component through zero at once and the camera whips through the pivot.
  const dot = s.dx.value * g[0] + s.dy.value * g[1] + s.dz.value * g[2];
  const gd: [number, number, number] = dot < -0.999 ? [-g[0], -g[1], -g[2]] : g;
  const step = (sp: Spring, target: number) => springStep(sp, target, dt, response, damping);
  return {
    tx: step(s.tx, goal.target[0]),
    ty: step(s.ty, goal.target[1]),
    tz: step(s.tz, goal.target[2]),
    dx: step(s.dx, gd[0]),
    dy: step(s.dy, gd[1]),
    dz: step(s.dz, gd[2]),
    logDist: step(s.logDist, Math.log(Math.max(1e-3, goal.distance))),
  };
}

/**
 * True when every axis has arrived and stopped.
 *
 * Checked so the rig can hand the camera back to OrbitControls rather than writing it
 * every frame forever: a spring approaches asymptotically and would otherwise fight
 * the user's first drag with a residual nudge.
 */
export function poseSettled(s: PoseSpring, goal: Pose): boolean {
  const g = normalize3(goal.dir);
  const near = (sp: Spring, target: number, eps: number) =>
    Math.abs(sp.value - target) < eps && Math.abs(sp.velocity) < eps * 10;
  return (
    near(s.tx, goal.target[0], SETTLE_POS) &&
    near(s.ty, goal.target[1], SETTLE_POS) &&
    near(s.tz, goal.target[2], SETTLE_POS) &&
    near(s.dx, g[0], SETTLE_VEL) &&
    near(s.dy, g[1], SETTLE_VEL) &&
    near(s.dz, g[2], SETTLE_VEL) &&
    // log space: 0.002 is 0.2% of the distance, sub-pixel at any dolly this rig reaches.
    near(s.logDist, Math.log(Math.max(1e-3, goal.distance)), SETTLE_POS)
  );
}

/** Snap the spring onto `goal` with no motion: the `prefers-reduced-motion` path. */
export function poseSnap(goal: Pose): PoseSpring {
  return poseSpring(goal);
}

/** Camera world position implied by a pose. */
export function posePosition(p: Pose): [number, number, number] {
  const d = normalize3(p.dir);
  return [
    p.target[0] + d[0] * p.distance,
    p.target[1] + d[1] * p.distance,
    p.target[2] + d[2] * p.distance,
  ];
}
