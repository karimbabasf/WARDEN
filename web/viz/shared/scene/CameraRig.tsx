// CameraRig.tsx — orbit (scaled to the forest) + cinematic focus, with a tab-aware
// locked mode for the radar board.
//
// HISTORY: the first rig was a deliberately *caged* turntable — pan off, a fixed
// maxDistance of 24, the pivot pinned to origin, tilt clamped. That was right for
// a small cluster but with a large agent forest it traps you: you can't dolly out
// far enough to see everything, and you can't move the pivot to reach agents far
// from centre. This rig removes the cage and scales to the actual scene:
//
//   • ZOOM + FRAMING SCALE TO BOUNDS. Given the forest's bounding sphere
//     (`sceneBounds`), max dolly and the camera far-plane grow to contain it, and
//     "home"/overview frames the whole thing — so you can always pull back to see
//     every agent, however many there are.
//   • PAN + ZOOM-TO-CURSOR + FREE ROTATION. Right-drag pans the pivot across the
//     forest, the wheel dollies toward the cursor, and tilt is (almost) unclamped,
//     so distant agents are reachable and you can turn freely.
//   • LOCKED MODE (`locked`). The radar board passes this: rotate + pan are off and
//     the overview looks straight on (+Z), so the abacus rails stay horizontal. The
//     wheel still dollies toward the cursor. Habits leaves it false (uncaged).
//
// The cinematic moves are unchanged: selecting an orb glides the target onto it
// (preserving your viewing angle) and remembers the dive-from pose to restore on
// back-out; `focusBounds` flies to frame a subtree over ~700ms; `homeSignal`
// eases back to the (now bounds-framed) overview.

import { useEffect, useMemo, useRef } from 'react';
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

// Fallbacks used when no scene bounds are available yet (empty forest).
const OVERVIEW_DIST = 12.6;
const MIN_DIST = 5;
const MAX_DIST_BASE = 24; // floor — small scenes keep the original cosy range.
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

// Fly-to framing timing — explicit ~700ms expo ease (a deliberate cinematic push).
const FLY_MS = 700;

// How far past its lattice radius a globe actually reaches: a root wears the AgentCore
// gyro cradle plus a halo sprite, so the thing to keep inside the channel is wider than
// the layout radius the fit would otherwise use.
const CORE_REACH = 1.5;
// The most of the free channel a dived-in globe may occupy before the camera backs off.
// A CEILING, not a target: under it the dive keeps its own deliberately cosy distance.
const SELECT_MAX_FILL = 0.92;
// Fraction of the channel a framed SUBTREE fills. This one is a target, not a ceiling:
// framing a subtree is a fit, so it always frames. 0.6 is the value `frameDistance`
// defaulted to before the channel aspect was threaded through, so a roomy window
// (channel already wider than tall) frames exactly as it did.
const FOCUS_FILL = 0.6;

const dir = new THREE.Vector3();
// Scratch for the camera's right vector when trucking sideways into the free channel
// (module-level so the per-pose shift allocates nothing).
const rightAxis = new THREE.Vector3();
const WORLD_UP = new THREE.Vector3(0, 1, 0);
// A pleasant 3/4 overview angle the home/reset pose is framed along.
const OVERVIEW_DIR = new THREE.Vector3(0.35, 0.28, 1).normalize();
// Locked-board direction: dead-on the +Z axis so the abacus rails read horizontal
// with no perspective tilt between rails (up stays +Y).
const STRAIGHT_ON_DIR = new THREE.Vector3(0, 0, 1);

function easeInOutExpo(t: number): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  return t < 0.5
    ? Math.pow(2, 20 * t - 10) / 2
    : (2 - Math.pow(2, -20 * t + 10)) / 2;
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
  // Frame against the FREE CHANNEL between the rails, not the whole canvas. The canvas
  // spans the window and the chrome floats on top of it, so fitting to `size.width`
  // parks the ends of the board under a panel. Zero insets reproduce the old aspect
  // exactly, so nothing changes for a caller that does not reserve chrome.
  const channelPx = channelWidth(size.width, { left: framingInsetLeft, right: framingInsetRight });
  const channelAspect = size.height > 0 ? channelPx / size.height : 1;
  const controls = useRef<any>(null);
  const targetGoal = useRef(new THREE.Vector3(0, 0, 0));
  const posGoal = useRef(new THREE.Vector3(0, 1, OVERVIEW_DIST));
  const animating = useRef(false);
  const wasSelected = useRef(false);
  const homeTarget = useRef(new THREE.Vector3(0, 0, 0));
  const homePos = useRef<THREE.Vector3 | null>(null);

  const flyActive = useRef(false);
  const flyClock = useRef(0);
  const flyFromTarget = useRef(new THREE.Vector3());
  const flyFromPos = useRef(new THREE.Vector3());
  const lastFocusKey = useRef<string | null>(null);
  const lastHomeSignal = useRef(homeSignal);

  // Derive the scaled limits from the forest bounds. overviewDist frames the whole
  // forest at ~50% fill (breathing room); maxDist gives headroom beyond that; far
  // grows to contain the farthest dolly. Clamped so a pathological layout can't
  // produce an absurd projection.
  // Which constellation is framed right now: the local board, or the peer's (already
  // parked beside it on world X). Everything downstream reads `framed`, so the peer
  // gets the identical framing law rather than a second code path.
  const framed = viewTarget === 'peer' && peerBounds ? peerBounds : sceneBounds;
  const fit = useMemo(() => {
    if (!framed || framed.radius <= 0) {
      return {
        center: new THREE.Vector3(0, 0, 0),
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
      center: new THREE.Vector3(framed.center[0], framed.center[1], framed.center[2]),
      radius: r,
      overviewDist,
      maxDist,
      far,
    };
  }, [framed, sceneBounds, peerBounds, locked, channelAspect]);
  // Latest-value ref so the [selected]/[focusBounds]/[homeSignal] effects and the
  // frame loop read current limits WITHOUT taking sceneBounds as a dependency
  // (which would re-fire the cinematic moves on every layout tick).
  const fitRef = useRef(fit);
  fitRef.current = fit;
  // Same latest-value trick for the rail insets, so the pose writers always truck by
  // the CURRENT chrome (the right rail opens on selection, mid-move) without the
  // cinematic effects taking the insets as a dependency and re-firing on every toggle.
  const insetsRef = useRef<RailInsets>({ left: framingInsetLeft, right: framingInsetRight });
  insetsRef.current = { left: framingInsetLeft, right: framingInsetRight };

  // Grow the camera far-plane to contain the scaled dolly range.
  useEffect(() => {
    const cam = camera as THREE.PerspectiveCamera;
    if (cam.far !== fit.far) {
      cam.far = fit.far;
      cam.updateProjectionMatrix();
    }
  }, [camera, fit.far]);

  // Distance at which a sphere of `radius` still fits inside the FREE CHANNEL. Same
  // law as the overview fit (frameDistance clamps the half-angle by min(1, aspect)
  // because three.js fov is vertical), just fed the channel's aspect rather than the
  // window's, so a rail narrowing the visible band pushes the camera back.
  function channelFit(radius: number, fovDeg: number, fill: number): number {
    return frameDistance(radius, fovDeg, fill, channelAspect);
  }

  // Slide a finished pose sideways so the scene lands in the middle of the FREE
  // CHANNEL rather than the middle of the window. Both the pivot and the camera move
  // by the same world vector: that is a TRUCK, so the framing shifts with no rotation
  // and no perspective skew. `viewDir` points from the pivot toward the camera (the
  // same convention `dir` uses below), so the camera's right is -(viewDir x up).
  function applyChannelShift(
    target: THREE.Vector3,
    pos: THREE.Vector3,
    viewDir: THREE.Vector3,
    distance: number,
  ) {
    const px = channelShiftPx(insetsRef.current);
    if (px === 0) return;
    const world = pixelsToWorld(px, distance, FOV_FALLBACK, size.height);
    if (world === 0) return;
    rightAxis.copy(viewDir).cross(WORLD_UP);
    if (rightAxis.lengthSq() < 1e-8) return; // looking straight up/down: no usable right
    rightAxis.normalize().negate();
    target.addScaledVector(rightAxis, world);
    pos.addScaledVector(rightAxis, world);
  }

  // Bounds-framed overview/home pose (replaces the old static one). When locked (the
  // radar board) the camera sits straight on the +Z axis looking at board centre (up
  // +Y), so the rails read horizontal with no perspective tilt; otherwise it frames
  // along the pleasant 3/4 hero angle. The whole pose is then trucked into the free
  // channel between the DOM rails.
  function writeOverviewPose(target: THREE.Vector3, pos: THREE.Vector3) {
    const f = fitRef.current;
    const overviewDir = locked ? STRAIGHT_ON_DIR : OVERVIEW_DIR;
    target.copy(f.center);
    pos.copy(f.center).addScaledVector(overviewDir, f.overviewDist);
    applyChannelShift(target, pos, overviewDir, f.overviewDist);
  }

  function beginFly() {
    const c = controls.current;
    if (c) {
      flyFromTarget.current.copy(c.target);
      flyFromPos.current.copy(c.object.position);
    } else {
      flyFromTarget.current.copy(targetGoal.current);
      flyFromPos.current.copy(posGoal.current);
    }
    flyClock.current = 0;
    flyActive.current = true;
    animating.current = true;
  }

  // --- Orb selection focus: damped glide that preserves angle, with verbatim home
  // capture/restore on the focus edge. ---
  useEffect(() => {
    const c = controls.current;
    if (selected) {
      if (!wasSelected.current && c) {
        homeTarget.current.copy(c.target);
        homePos.current = (homePos.current ?? new THREE.Vector3()).copy(c.object.position);
      }
      wasSelected.current = true;

      targetGoal.current.set(selected.position.x, selected.position.y, selected.position.z);
      const dist = THREE.MathUtils.clamp(
        Math.max(
          2.6 + Math.max(0.6, selected.radius) * 3.4,
          // Overflow guard, not a re-frame: the dive keeps its own cosy distance
          // unless that would push the globe wider than the free channel, in which
          // case back off just far enough to contain it. At a roomy window this term
          // never wins, so the feel of the dive is unchanged.
          channelFit(Math.max(0.6, selected.radius) * CORE_REACH, FOV_NEAR, SELECT_MAX_FILL),
        ),
        MIN_DIST,
        fitRef.current.maxDist,
      );
      if (c) {
        dir.copy(c.object.position).sub(c.target);
        if (dir.lengthSq() < 1e-6) dir.set(0, 0, 1);
        dir.normalize();
      } else {
        dir.set(0, 0, 1);
      }
      posGoal.current.copy(targetGoal.current).addScaledVector(dir, dist);
      // Selecting is exactly when the right rail opens, so the dive must land the
      // globe in the narrowed channel, not behind the panel that just appeared.
      applyChannelShift(targetGoal.current, posGoal.current, dir, dist);
    } else {
      wasSelected.current = false;
      if (homePos.current) {
        // Restore the ANGLE you dived in from, but re-derive centre and distance from
        // the LIVE fit. Replaying the captured position verbatim re-frames the forest as
        // it was at dive time, so any agent that appeared while you were inspecting
        // would land outside the frame on back-out (the locked auto-refit effect is
        // skipped while something is selected, so nothing else corrects it).
        const f = fitRef.current;
        dir.copy(homePos.current).sub(homeTarget.current);
        if (dir.lengthSq() < 1e-6) dir.copy(locked ? STRAIGHT_ON_DIR : OVERVIEW_DIR);
        dir.normalize();
        targetGoal.current.copy(f.center);
        posGoal.current.copy(f.center).addScaledVector(dir, f.overviewDist);
        applyChannelShift(targetGoal.current, posGoal.current, dir, f.overviewDist);
      } else {
        writeOverviewPose(targetGoal.current, posGoal.current);
      }
    }
    animating.current = true;
    flyActive.current = false;
    // The insets are dependencies, not just values read at click time: the right rail
    // OPENS BECAUSE something was selected, so its width lands one commit after this
    // effect first runs. Without re-running, the dive keeps a pose computed against
    // chrome that was not on screen yet.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, framingInsetLeft, framingInsetRight]);

  // --- Cinematic fly-to framing on focusBounds change (preserve view angle). ---
  useEffect(() => {
    const c = controls.current;

    if (focusBounds) {
      // The rail insets are part of the key. Selecting an agent OPENS the right
      // rail, and the insets are measured from the DOM a frame or two later, so the
      // first run of this effect still sees right=0. Without the insets in the key
      // that stale pose would stick and the globe would sit half under a panel.
      const key =
        `${focusBounds.center[0]},${focusBounds.center[1]},${focusBounds.center[2]}:${focusBounds.radius}` +
        `@${framingInsetLeft},${framingInsetRight}`;
      if (key === lastFocusKey.current) return;
      lastFocusKey.current = key;

      if (homePos.current == null && c) {
        homeTarget.current.copy(c.target);
        homePos.current = new THREE.Vector3().copy(c.object.position);
      }

      targetGoal.current.set(focusBounds.center[0], focusBounds.center[1], focusBounds.center[2]);
      const fov = c ? c.object.fov : FOV_FAR;
      // Frame against the FREE CHANNEL, exactly as the overview fit does. Omitting
      // the aspect here framed a dived-in globe against the whole viewport, so at a
      // wide window with both rails open it came out far too large and spilled
      // underneath the fleet rack. `fov` is vertical only, so the channel aspect is
      // what converts it into a horizontal constraint.
      const dist = THREE.MathUtils.clamp(
        // `fill` keeps its default; the only thing added here is the channel aspect,
        // so this composes with the FOCUS_MAX_FILL ceiling applied downstream rather
        // than overriding it with a fixed target.
        frameDistance(focusBounds.radius, fov, undefined, channelAspect),
        MIN_DIST,
        fitRef.current.maxDist,
      );
      if (c) {
        dir.copy(c.object.position).sub(c.target);
        if (dir.lengthSq() < 1e-6) dir.set(0, 0, 1);
        dir.normalize();
      } else {
        dir.set(0, 0, 1);
      }
      posGoal.current.copy(targetGoal.current).addScaledVector(dir, dist);
      applyChannelShift(targetGoal.current, posGoal.current, dir, dist);
      beginFly();
    } else {
      if (lastFocusKey.current !== null) {
        lastFocusKey.current = null;
        if (homePos.current) {
          targetGoal.current.copy(homeTarget.current);
          posGoal.current.copy(homePos.current);
        } else {
          writeOverviewPose(targetGoal.current, posGoal.current);
        }
        beginFly();
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusBounds, framingInsetLeft, framingInsetRight]);

  useEffect(() => {
    if (homeSignal === lastHomeSignal.current) return;
    lastHomeSignal.current = homeSignal;

    writeOverviewPose(targetGoal.current, posGoal.current);
    homeTarget.current.copy(targetGoal.current);
    homePos.current = new THREE.Vector3().copy(posGoal.current);
    wasSelected.current = false;
    lastFocusKey.current = null;
    beginFly();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [homeSignal]);

  // --- Peer slide-over: truck laterally between the two constellations. ---
  //
  // Watching somebody else's swarm is a MOVE, not a swap: both boards stay mounted in
  // world space (theirs parked to the right of yours by peerFraming) and the camera
  // slides across to the other one. Nothing implodes, nothing cross-fades, nothing
  // remounts, so the eye keeps its place, and the boards' relative sizes and distance
  // stay readable the whole way over. It reuses the SAME expo fly the focus dive and
  // the home reset use, so the slide speaks the rig's existing motion language.
  //
  // Declared after the [selected] effect so that when the lead clears the selection in
  // the same commit as the truck, this pose is the one that wins.
  const lastViewTarget = useRef(viewTarget);
  useEffect(() => {
    if (viewTarget === lastViewTarget.current) return;
    lastViewTarget.current = viewTarget;

    writeOverviewPose(targetGoal.current, posGoal.current);
    homeTarget.current.copy(targetGoal.current);
    homePos.current = (homePos.current ?? new THREE.Vector3()).copy(posGoal.current);
    wasSelected.current = false;
    lastFocusKey.current = null;
    beginFly();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewTarget]);

  // --- Auto-fit (locked radar board only): keep the whole board framed as agents
  // arrive and leave. Fires ONLY when the framed bounds change MATERIALLY (a rounded
  // signature of centre + radius), so a steady scene never fights the user's wheel
  // dolly, but a new agent that grows or shrinks the board eases the overview back to
  // fit. Skipped while a bead is selected or a subtree is focused so it never yanks
  // the view mid-inspection; on back-out the deselect path reframes to the fresh board.
  const lastFitSig = useRef<string | null>(null);
  useEffect(() => {
    if (!locked || selected || focusBounds) return;
    const f = fitRef.current;
    // The signature carries the FRAMING too, not just the bounds: a rail opening or a
    // window resize changes the free channel without moving a single agent, and the
    // board has to ease back into the new channel when it does.
    const sig = `${f.center.x.toFixed(1)},${f.center.y.toFixed(1)},${f.center.z.toFixed(1)}:${f.radius.toFixed(1)}@${f.overviewDist.toFixed(1)}+${channelShiftPx(insetsRef.current).toFixed(0)}`;
    if (sig === lastFitSig.current) return;
    lastFitSig.current = sig;
    writeOverviewPose(targetGoal.current, posGoal.current);
    homeTarget.current.copy(targetGoal.current);
    homePos.current = new THREE.Vector3().copy(posGoal.current);
    beginFly();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fit, locked, selected, focusBounds, framingInsetLeft, framingInsetRight]);

  useFrame((_, dtRaw) => {
    const c = controls.current;
    if (!c) return; // controls not mounted yet on the very first frame.
    const dt = Math.min(dtRaw, 0.05);

    if (animating.current) {
      if (flyActive.current) {
        flyClock.current += dt * 1000;
        const t = Math.min(1, flyClock.current / FLY_MS);
        const e = easeInOutExpo(t);
        c.target.copy(flyFromTarget.current).lerp(targetGoal.current, e);
        c.object.position.copy(flyFromPos.current).lerp(posGoal.current, e);
        if (t >= 1) {
          flyActive.current = false;
          c.target.copy(targetGoal.current);
          c.object.position.copy(posGoal.current);
        }
      } else {
        const k = 1 - Math.exp(-7 * dt);
        c.target.lerp(targetGoal.current, k);
        c.object.position.lerp(posGoal.current, k);
      }

      if (
        !flyActive.current &&
        c.target.distanceToSquared(targetGoal.current) < 0.0004 &&
        c.object.position.distanceToSquared(posGoal.current) < 0.0009
      ) {
        animating.current = false;
      }
    }

    // FOV taper — counteract close-zoom fisheye.
    const camDist = c.object.position.distanceTo(c.target);
    const taper = THREE.MathUtils.smoothstep(camDist, MIN_DIST, FOV_TAPER_START);
    const fovTarget = THREE.MathUtils.lerp(FOV_NEAR, FOV_FAR, taper);
    const fovK = 1 - Math.exp(-7 * dt);
    const nextFov = THREE.MathUtils.lerp(c.object.fov, fovTarget, fovK);
    if (Math.abs(nextFov - c.object.fov) > FOV_EPS) {
      c.object.fov = nextFov;
      c.object.updateProjectionMatrix();
    }

    c.update();
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
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
