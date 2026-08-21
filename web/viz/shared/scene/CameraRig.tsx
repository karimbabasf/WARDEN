// CameraRig.tsx: orbit (scaled to the forest) plus one spring-driven camera move,
// with a tab-aware locked mode for the radar board.
//
// HISTORY, PART ONE: the first rig was a deliberately *caged* turntable, pan off, a
// fixed maxDistance of 24, the pivot pinned to origin, tilt clamped. That was right for
// a small cluster but with a large agent forest it traps you. This rig removes the cage
// and scales to the actual scene:
//
//   • ZOOM + FRAMING SCALE TO BOUNDS. Given the forest's bounding sphere
//     (`sceneBounds`), max dolly and the camera far-plane grow to contain it, and
//     "home"/overview frames the whole thing.
//   • PAN + ZOOM-TO-CURSOR + FREE ROTATION. Right-drag pans the pivot across the
//     forest, the wheel dollies toward the cursor, tilt is (almost) unclamped.
//   • LOCKED MODE (`locked`). The radar board passes this: rotate + pan are off and
//     the overview looks straight on (+Z), so the abacus rails stay horizontal.
//
// HISTORY, PART TWO, and the reason this file was rebuilt: the moves themselves used
// to be a fixed-duration `easeInOutExpo` clock plus a second damped-lerp branch, aimed
// by FIVE separate effects that each re-wrote the goal and restarted the clock. An
// expo ease starts and ends at zero velocity, so every restart was a visible dead
// stop, and one click produced three of them (selection, then `focusBounds` a commit
// later, then the rail insets a frame after that). See `cameraPose.ts` for the full
// autopsy. Three things changed:
//
//   1. **ONE WRITER.** The effects no longer compute poses. They set an AIM (what the
//      camera should be looking at) and nothing else. `resolveAim` turns an aim into a
//      pose once per frame, so there is exactly one place a pose is ever produced.
//   2. **RESOLVED PER FRAME, NOT PER COMMIT.** The pose is recomputed every frame from
//      the LIVE fit, insets and channel aspect, so a rail opening mid-dive, a window
//      resize, or a globe arriving is absorbed as a bend in the path. None of them is
//      a dependency of anything any more, and the whole auto-refit effect deleted
//      itself: the overview aim already tracks the board it is aimed at.
//   3. **SPRUNG, NOT EASED.** `poseSpringStep` carries velocity through a re-target,
//      so re-aiming mid-flight curves rather than stalls.
//
// The one law for how close a dive sits (`resolveAim`) also replaces two laws that
// used to disagree by ~1.7 world units on a leaf, which is what "arrived, stopped,
// then backed out again" was.

import { useCallback, useEffect, useMemo, useRef } from 'react';
import { useFrame, useThree } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from '@/viz/shared/types/orbTypes';
import {
  frameDistance,
  channelWidth,
  channelShiftPx,
  pixelsToWorld,
  type Bounds,
  type RailInsets,
} from './cameraFraming';
import {
  poseOf,
  poseSettled,
  poseSpring,
  poseSpringStep,
  posePosition,
  normalize3,
  POSE_RESPONSE,
  POSE_RESPONSE_QUIET,
  type Pose,
  type PoseSpring,
} from './cameraPose';
import { useReducedMotion } from './reducedMotion';

// Fallbacks used when no scene bounds are available yet (empty forest).
const OVERVIEW_DIST = 12.6;
const MIN_DIST = 5;
const MAX_DIST_BASE = 24; // floor: small scenes keep the original cosy range.
const DEFAULT_FAR = 140;
const FOV_FALLBACK = 46; // matches the <Canvas camera> fov in WarRoom.

// Overview framing fill: how much of the frame the whole forest fills at rest. The
// locked radar board frames a touch tighter than Habits so the beads read larger.
const OVERVIEW_FILL = 0.5;
const LOCKED_OVERVIEW_FILL = 0.72;

// FOV taper (orbit only): ease the lens from FOV_FAR toward FOV_NEAR on close
// approach to counteract wide-angle fisheye.
const FOV_FAR = 46;
const FOV_NEAR = 38;
const FOV_TAPER_START = 9;
const FOV_EPS = 0.01;

// How far past its lattice radius a globe actually reaches: a root wears the AgentCore
// gyro cradle plus a halo sprite, so the thing to keep inside the channel is wider than
// the layout radius the fit would otherwise use.
const CORE_REACH = 1.5;
// The most of the free channel a dived-in globe may occupy before the camera backs off.
// A CEILING, not a target: under it the dive keeps its own deliberately cosy distance.
const SELECT_MAX_FILL = 0.92;
// Fraction of the channel a framed SUBTREE fills. A target, not a ceiling: framing a
// subtree is a fit, so it always frames.
const FOCUS_FILL = 0.6;

// How far the resolved goal may drift from a SETTLED camera before the rig re-engages
// on its own, as a fraction of the current dolly distance. This is what replaces the
// old auto-refit effect: 4% is under a globe's width at any dolly the board reaches,
// so a steady scene never twitches, but a globe arriving and widening the board eases
// the overview back out. Never fires once the operator has touched the controls.
const REENGAGE_DRIFT = 0.04;

const WORLD_UP = new THREE.Vector3(0, 1, 0);
// A pleasant 3/4 overview angle the home/reset pose is framed along.
const OVERVIEW_DIR: [number, number, number] = (() => {
  const v = new THREE.Vector3(0.35, 0.28, 1).normalize();
  return [v.x, v.y, v.z];
})();
// Locked-board direction: dead-on the +Z axis so the abacus rails read horizontal
// with no perspective tilt between rails (up stays +Y).
const STRAIGHT_ON_DIR: [number, number, number] = [0, 0, 1];

// Scratch, module level so the per-frame resolve allocates nothing.
const scratchDir = new THREE.Vector3();
const scratchRight = new THREE.Vector3();

/**
 * What the camera is currently aimed at. NOT a pose: an aim survives the board moving
 * under it, which is the whole reason the rig stopped storing poses.
 *
 * `dir` is the viewing angle frozen at the moment the aim was set. Every move this rig
 * makes preserves the angle you were already looking from; re-deriving it per frame
 * from a camera that is itself moving is a feedback loop.
 */
type Aim =
  | {
      kind: 'overview';
      /** Null asks for the canonical hero/straight-on angle rather than a kept one. */
      dir: [number, number, number] | null;
    }
  | {
      kind: 'frame';
      center: [number, number, number];
      /** Bounding radius of what is being framed. */
      radius: number;
      /**
       * Radius of the single globe that was SELECTED, when one was. Drives the cosy
       * floor below, so a leaf dive keeps its deliberate distance instead of being
       * fitted tight to a sphere the size of one bead. Null for a folder pick.
       */
      nodeRadius: number | null;
      dir: [number, number, number];
    };

/**
 * Value identity of a selection: two different objects describing the same globe in
 * the same place must produce the same string.
 *
 * The rail insets used to be part of this key, because the pose was computed once per
 * commit and the right rail opens one commit after the selection that opened it. They
 * are gone: the pose is now resolved every frame against whatever the insets currently
 * are, so a rail arriving bends the move instead of restarting it. Exported so the
 * property the rig depends on (a re-emitted board is not a new selection) is tested
 * rather than assumed.
 */
export function selectKey(node: Pick<LayoutNode, 'id' | 'position' | 'radius'> | null): string {
  if (!node) return 'none';
  const p = node.position;
  return `${node.id}:${p.x.toFixed(3)},${p.y.toFixed(3)},${p.z.toFixed(3)}:${node.radius.toFixed(3)}`;
}

/**
 * Value identity of a framed subtree, ROUNDED.
 *
 * The old key interpolated the raw floats. `subtreeBounds` is recomputed from a layout
 * the lead rebuilds on every `radar_state` emit, so its centre came back differing in
 * the twelfth decimal and the camera re-flew a 700ms ease once a second for as long as
 * you stayed dived in. Two decimals is well under a globe's radius: a real move still
 * changes the key, float noise does not.
 */
export function focusKey(b: Bounds | null): string {
  if (!b) return 'none';
  return `${b.center[0].toFixed(2)},${b.center[1].toFixed(2)},${b.center[2].toFixed(2)}:${b.radius.toFixed(2)}`;
}

export function CameraRig({
  selected,
  focusBounds = null,
  homeSignal = 0,
  sceneBounds = null,
  locked = false,
  framingInsetLeft = 0,
  framingInsetRight = 0,
  peerBounds = null,
  viewTarget = 'local',
}: {
  selected: LayoutNode | null;
  focusBounds?: Bounds | null;
  homeSignal?: number;
  /** Bounding sphere of the whole active forest; scales zoom range + framing. */
  sceneBounds?: Bounds | null;
  /** Radar board: lock rotate + pan, keep zoom-to-cursor, look straight on. */
  locked?: boolean;
  /** CSS pixels of chrome reserved on the LEFT of the canvas (the always-on rail). */
  framingInsetLeft?: number;
  /** CSS pixels of chrome reserved on the RIGHT (the inspector rail, 0 when closed). */
  framingInsetRight?: number;
  /** A watched peer's constellation, ALREADY offset into world space (peerFraming). */
  peerBounds?: Bounds | null;
  /** Which constellation the camera frames. Flipping it trucks laterally between them. */
  viewTarget?: 'local' | 'peer';
}) {
  const { camera } = useThree();
  // Canvas pixel size, reactive on resize. Feeds the aspect-aware overview fit so a
  // narrow window frames the wide rail layout instead of clipping its ends.
  const size = useThree((s) => s.size);
  const reduced = useReducedMotion();
  const controls = useRef<any>(null);

  // Frame against the FREE CHANNEL between the rails, not the whole canvas. The canvas
  // spans the window and the chrome floats on top of it, so fitting to `size.width`
  // parks the ends of the board under a panel.
  const channelPx = channelWidth(size.width, { left: framingInsetLeft, right: framingInsetRight });
  const channelAspect = size.height > 0 ? channelPx / size.height : 1;

  // Which constellation is framed right now: the local board, or the peer's (already
  // parked beside it on world X). Everything downstream reads `framed`, so the peer
  // gets the identical framing law rather than a second code path.
  const framed = viewTarget === 'peer' && peerBounds ? peerBounds : sceneBounds;
  const fit = useMemo(() => {
    if (!framed || framed.radius <= 0) {
      return {
        center: [0, 0, 0] as [number, number, number],
        radius: 0,
        overviewDist: OVERVIEW_DIST,
        maxDist: MAX_DIST_BASE,
        far: DEFAULT_FAR,
      };
    }
    const r = framed.radius;
    const overviewDist = frameDistance(
      r,
      FOV_FALLBACK,
      locked ? LOCKED_OVERVIEW_FILL : OVERVIEW_FILL,
      channelAspect,
    );
    const maxDist = Math.min(1400, Math.max(MAX_DIST_BASE, overviewDist * 1.35));
    // Far plane must contain BOTH boards, not just the framed one: while watching a
    // peer, the local constellation is still in the scene off to the side, and pulling
    // back to see them both at once must not clip either away.
    const spread = peerBounds && sceneBounds
      ? Math.hypot(
          peerBounds.center[0] - sceneBounds.center[0],
          peerBounds.center[1] - sceneBounds.center[1],
          peerBounds.center[2] - sceneBounds.center[2],
        ) + Math.max(peerBounds.radius, sceneBounds.radius)
      : r;
    const far = Math.min(4000, Math.max(DEFAULT_FAR, (maxDist + Math.max(r, spread)) * 1.3));
    return {
      center: [framed.center[0], framed.center[1], framed.center[2]] as [number, number, number],
      radius: r,
      overviewDist,
      maxDist,
      far,
    };
  }, [framed, sceneBounds, peerBounds, locked, channelAspect]);

  // Latest-value refs so the per-frame resolve reads CURRENT framing without any of it
  // becoming a dependency of an effect. This is what lets a rail open mid-dive without
  // re-aiming anything.
  const fitRef = useRef(fit);
  fitRef.current = fit;
  const insetsRef = useRef<RailInsets>({ left: framingInsetLeft, right: framingInsetRight });
  insetsRef.current = { left: framingInsetLeft, right: framingInsetRight };
  const aspectRef = useRef(channelAspect);
  aspectRef.current = channelAspect;
  const heightRef = useRef(size.height);
  heightRef.current = size.height;
  const lockedRef = useRef(locked);
  lockedRef.current = locked;

  // Grow the camera far-plane to contain the scaled dolly range.
  useEffect(() => {
    const cam = camera as THREE.PerspectiveCamera;
    if (cam.far !== fit.far) {
      cam.far = fit.far;
      cam.updateProjectionMatrix();
    }
  }, [camera, fit.far]);

  // ── the aim, and the spring that chases it ──────────────────────────────────
  const aim = useRef<Aim>({ kind: 'overview', dir: null });
  const pose = useRef<PoseSpring | null>(null);
  // Writing the camera every frame forever would fight the operator's first drag with
  // a residual nudge, so the rig releases the camera once it has arrived and only
  // takes it back when something re-aims it (or the board drifts materially under a
  // camera the operator has not touched).
  const engaged = useRef(false);
  const response = useRef(POSE_RESPONSE);
  // Set the moment the operator grabs the controls, cleared by the next real aim.
  // While set, the board changing shape never steals the view back.
  const userAdjusted = useRef(false);
  // The angle the current dive was entered from, so backing out restores it.
  const homeDir = useRef<[number, number, number] | null>(null);

  /** Current viewing angle (pivot toward camera), or the canonical one if unusable. */
  const currentDir = useCallback((): [number, number, number] => {
    const c = controls.current;
    const fallback = lockedRef.current ? STRAIGHT_ON_DIR : OVERVIEW_DIR;
    if (!c) return [...fallback];
    scratchDir.copy(c.object.position).sub(c.target);
    if (scratchDir.lengthSq() < 1e-6) return [...fallback];
    scratchDir.normalize();
    return [scratchDir.x, scratchDir.y, scratchDir.z];
  }, []);

  const reaim = useCallback((next: Aim, quiet = false) => {
    aim.current = next;
    response.current = quiet ? POSE_RESPONSE_QUIET : POSE_RESPONSE;
    engaged.current = true;
    userAdjusted.current = false;
  }, []);

  /**
   * Turn the current aim into a pose, against the CURRENT fit, insets and aspect.
   *
   * Called once per frame while engaged. Two things are deliberate:
   *
   * * **The fov fed to the fit is FIXED**, never `camera.fov`. The lens tapers with
   *   distance (see the frame loop), so resolving the goal against the live fov is a
   *   loop: closer narrows the lens, a narrower lens asks for a greater distance. The
   *   old code got away with reading it because it resolved once per commit.
   * * **The channel shift moves the TARGET**, and the position follows from
   *   target + dir * distance. The old code shifted both and had to keep them in step.
   */
  const resolveAim = useCallback((a: Aim): Pose => {
    const f = fitRef.current;
    const aspect = aspectRef.current;
    let center: [number, number, number];
    let dir: [number, number, number];
    let distance: number;

    if (a.kind === 'overview') {
      dir = a.dir ?? (lockedRef.current ? STRAIGHT_ON_DIR : OVERVIEW_DIR);
      center = [...f.center];
      distance = f.overviewDist;
    } else {
      dir = a.dir;
      center = [...a.center];
      // ONE law for how close a dive sits, replacing two that disagreed. The greatest
      // of three terms wins:
      //   • the cosy dive distance, when a single globe was selected. This is the feel
      //     of a dive and it is why a leaf is not fitted tight to one bead;
      //   • an overflow guard, so a fat globe never spills wider than the free channel;
      //   • the subtree fit, so selecting a lead with eight children frames all nine.
      // At a leaf the first term wins (as the old select pose did); at a real subtree
      // the third does (as the old focus fly did). Neither can now contradict the
      // other mid-move, because there is only one of them.
      const nodeR = a.nodeRadius !== null ? Math.max(0.6, a.nodeRadius) : null;
      const cosy = nodeR !== null ? 2.6 + nodeR * 3.4 : 0;
      const overflow =
        nodeR !== null ? frameDistance(nodeR * CORE_REACH, FOV_NEAR, SELECT_MAX_FILL, aspect) : 0;
      const subtree = frameDistance(Math.max(0.05, a.radius), FOV_NEAR, FOCUS_FILL, aspect);
      distance = Math.max(cosy, overflow, subtree);
    }
    distance = THREE.MathUtils.clamp(distance, MIN_DIST, f.maxDist);

    // Slide the pivot sideways so the scene lands in the middle of the FREE CHANNEL
    // rather than the middle of the window. Moving the pivot along the camera's own
    // right vector is a TRUCK: the framing shifts with no rotation and no skew.
    const px = channelShiftPx(insetsRef.current);
    if (px !== 0) {
      const world = pixelsToWorld(px, distance, FOV_FALLBACK, heightRef.current);
      if (world !== 0) {
        scratchDir.set(dir[0], dir[1], dir[2]);
        scratchRight.copy(scratchDir).cross(WORLD_UP);
        // Looking straight up or down leaves no usable right vector; skip the truck
        // rather than normalizing a zero.
        if (scratchRight.lengthSq() >= 1e-8) {
          scratchRight.normalize().negate();
          center = [
            center[0] + scratchRight.x * world,
            center[1] + scratchRight.y * world,
            center[2] + scratchRight.z * world,
          ];
        }
      }
    }
    return { target: center, dir: normalize3(dir), distance };
  }, []);

  // ── what re-aims the camera ────────────────────────────────────────────────
  //
  // These effects do no pose maths and read no framing. They set an aim and stop. Note
  // what is NOT in any dependency list: the rail insets, the window size, the fit. All
  // three are read per frame instead, so none of them can restart a move.

  // Selection and subtree framing are ONE aim, because they always described the same
  // intent and only ever differed in the distance they asked for. `focusBounds` is the
  // honest extent of what was selected (the globe plus its children), `selected` is
  // what supplies the cosy floor.
  const lastAimKey = useRef<string | null>(null);
  useEffect(() => {
    const key = `${selectKey(selected)}|${focusKey(focusBounds)}`;
    if (key === lastAimKey.current) return;
    const first = lastAimKey.current === null;
    lastAimKey.current = key;

    if (selected || focusBounds) {
      // Remember the angle to come back to, captured on the way IN only.
      if (homeDir.current === null) homeDir.current = currentDir();
      const center: [number, number, number] = focusBounds
        ? [focusBounds.center[0], focusBounds.center[1], focusBounds.center[2]]
        : [selected!.position.x, selected!.position.y, selected!.position.z];
      const radius = focusBounds ? focusBounds.radius : selected!.radius;
      reaim({
        kind: 'frame',
        center,
        radius,
        nodeRadius: selected ? selected.radius : null,
        // Keep the angle we are already looking from. Re-read on every re-aim so a
        // dive that follows a manual orbit starts from where the operator left it.
        dir: currentDir(),
      });
    } else {
      // Backing out. Restore the ANGLE the dive was entered from, but let the
      // per-frame resolve supply the centre and distance from the LIVE fit, so an
      // agent that arrived while you were inspecting is inside the frame you land in.
      const dir = homeDir.current;
      homeDir.current = null;
      // The very first run is mount, not a deselect: do not steal the camera before
      // the operator has done anything.
      if (first) {
        aim.current = { kind: 'overview', dir };
        return;
      }
      reaim({ kind: 'overview', dir });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, focusBounds]);

  // Escape / empty double-click: back to the canonical framed overview.
  const lastHomeSignal = useRef(homeSignal);
  useEffect(() => {
    if (homeSignal === lastHomeSignal.current) return;
    lastHomeSignal.current = homeSignal;
    homeDir.current = null;
    reaim({ kind: 'overview', dir: null });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [homeSignal]);

  // Peer slide-over: watching somebody else's swarm is a MOVE, not a swap. Both boards
  // stay mounted in world space (theirs parked to the right of yours by peerFraming)
  // and the camera trucks across. `fit` already follows `viewTarget`, so re-aiming at
  // the overview is the entire implementation.
  const lastViewTarget = useRef(viewTarget);
  useEffect(() => {
    if (viewTarget === lastViewTarget.current) return;
    lastViewTarget.current = viewTarget;
    homeDir.current = null;
    reaim({ kind: 'overview', dir: null });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewTarget]);

  // The operator taking the controls wins over anything automatic until the next real
  // aim. Without this the board re-fitting under a settled camera would yank the view
  // back from wherever they had just dollied to.
  const onControlStart = useCallback(() => {
    engaged.current = false;
    userAdjusted.current = true;
  }, []);

  useFrame((_, dtRaw) => {
    const c = controls.current;
    if (!c) return; // controls not mounted yet on the very first frame.
    const dt = Math.min(dtRaw, 0.05);
    const goal = resolveAim(aim.current);
    const camPos = c.object.position;
    const camTarget = c.target;

    if (!engaged.current && !userAdjusted.current) {
      // Nothing re-aimed us, but the board may have changed shape underneath. This is
      // the old auto-refit effect, reduced to a distance check against the LIVE camera:
      // if the goal has walked away from where we are sitting, ease back to it.
      // Re-engaging is quiet, because the operator did not ask for it.
      const dolly = camPos.distanceTo(camTarget);
      const drift = Math.hypot(
        goal.target[0] - camTarget.x,
        goal.target[1] - camTarget.y,
        goal.target[2] - camTarget.z,
      );
      const tol = Math.max(0.05, dolly * REENGAGE_DRIFT);
      if (drift > tol || Math.abs(goal.distance - dolly) > tol) {
        engaged.current = true;
        response.current = POSE_RESPONSE_QUIET;
      }
    }

    if (engaged.current) {
      // Seed ONLY when idle. A re-aim that lands mid-move deliberately keeps the
      // existing spring: that is the velocity being carried through, and it is the
      // whole reason this rig stopped stalling on every re-target.
      if (!pose.current) {
        pose.current = poseSpring({
          target: [camTarget.x, camTarget.y, camTarget.z],
          dir: normalize3([
            camPos.x - camTarget.x,
            camPos.y - camTarget.y,
            camPos.z - camTarget.z,
          ]),
          distance: Math.max(1e-3, camPos.distanceTo(camTarget)),
        });
      }
      if (reduced) {
        // Reduced motion asks for no MOVEMENT, not for a camera that never arrives.
        pose.current = poseSpring(goal);
      } else {
        pose.current = poseSpringStep(pose.current, goal, dt, response.current);
      }
      const at = poseOf(pose.current);
      const [px, py, pz] = posePosition(at);
      camTarget.set(at.target[0], at.target[1], at.target[2]);
      camPos.set(px, py, pz);
      if (reduced || poseSettled(pose.current, goal)) {
        // Arrived: drop the spring and hand the camera back to OrbitControls, so the
        // operator's next drag is not fighting a residual nudge.
        pose.current = null;
        engaged.current = false;
      }
    }

    // FOV taper: counteract close-zoom fisheye. Deliberately NOT fed back into the
    // fit above (see `resolveAim`).
    const camDist = c.object.position.distanceTo(c.target);
    const taper = THREE.MathUtils.smoothstep(camDist, MIN_DIST, FOV_TAPER_START);
    const fovTarget = THREE.MathUtils.lerp(FOV_NEAR, FOV_FAR, taper);
    // Track the taper on the same response as the move, so the lens and the dolly
    // arrive together. They used to run on unrelated curves, and because fov changes
    // apparent size, the sum of the two was not monotone: the push-in visibly slowed,
    // then crept, at the end of every dive.
    const fovK = reduced ? 1 : 1 - Math.exp((-2 * Math.PI * dt) / response.current);
    const nextFov = THREE.MathUtils.lerp(c.object.fov, fovTarget, fovK);
    if (Math.abs(nextFov - c.object.fov) > FOV_EPS) {
      c.object.fov = nextFov;
      c.object.updateProjectionMatrix();
    }

    c.update();

    // Dev-only trace, so "is the dive smooth" is a measurement rather than an opinion.
    // The rig drives drei's OrbitControls, which is inside R3F's own store and reachable
    // from nothing on `window`, so the browser harnesses could look at the camera but
    // never sample it. `import.meta.env.DEV` is a literal at build time, so the whole
    // block leaves the production bundle. Read it as `__wardenCam` in a page probe:
    // a smooth move has a single-peaked speed profile, and every stall this rebuild
    // removed showed up as a zero in the middle of one.
    if (import.meta.env.DEV) {
      const w = window as unknown as { __wardenCam?: number[][] };
      if (!w.__wardenCam) w.__wardenCam = [];
      if (w.__wardenCam.length < 3000) {
        w.__wardenCam.push([
          performance.now(),
          c.object.position.x,
          c.object.position.y,
          c.object.position.z,
          camDist,
          engaged.current ? 1 : 0,
        ]);
      }
    }
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      onStart={onControlStart}
      enableDamping
      dampingFactor={0.15}
      rotateSpeed={0.95}
      // Wheel/trackpad dolly was too twitchy at the default 1.0. 0.6 was still enough to
      // overshoot the board in one flick on a Mac trackpad, where momentum scrolling
      // keeps delivering wheel events after the fingers lift, so calm it further: a
      // scroll should nudge the zoom, never lurch it.
      zoomSpeed={0.32}
      // Board is locked: no rotate, no pan; the wheel still dollies toward the
      // cursor. Habits keeps the uncaged rig (rotate + pan).
      enableRotate={!locked}
      enablePan={!locked}
      screenSpacePanning={!locked}
      zoomToCursor
      minDistance={MIN_DIST}
      maxDistance={fit.maxDist}
      minPolarAngle={locked ? Math.PI / 2 : 0.01}
      maxPolarAngle={locked ? Math.PI / 2 : Math.PI - 0.01}
      minAzimuthAngle={locked ? 0 : -Infinity}
      maxAzimuthAngle={locked ? 0 : Infinity}
    />
  );
}

export default CameraRig;
