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
import { frameDistance, type Bounds } from './cameraFraming';

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

const dir = new THREE.Vector3();
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
}: {
  selected: LayoutNode | null;
  focusBounds?: Bounds | null;
  homeSignal?: number;
  /** Bounding sphere of the whole active forest; scales zoom range + framing. */
  sceneBounds?: Bounds | null;
  /** Radar board: lock rotate + pan, keep zoom-to-cursor, look straight on. */
  locked?: boolean;
}) {
  const { camera } = useThree();
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
  const fit = useMemo(() => {
    if (!sceneBounds || sceneBounds.radius <= 0) {
      return {
        center: new THREE.Vector3(0, 0, 0),
        radius: 0,
        overviewDist: OVERVIEW_DIST,
        maxDist: MAX_DIST_BASE,
        far: DEFAULT_FAR,
      };
    }
    const r = sceneBounds.radius;
    const overviewDist = frameDistance(r, FOV_FALLBACK, locked ? LOCKED_OVERVIEW_FILL : OVERVIEW_FILL);
    const maxDist = Math.min(1400, Math.max(MAX_DIST_BASE, overviewDist * 1.35));
    const far = Math.min(4000, Math.max(DEFAULT_FAR, (maxDist + r) * 1.3));
    return {
      center: new THREE.Vector3(sceneBounds.center[0], sceneBounds.center[1], sceneBounds.center[2]),
      radius: r,
      overviewDist,
      maxDist,
      far,
    };
  }, [sceneBounds, locked]);
  // Latest-value ref so the [selected]/[focusBounds]/[homeSignal] effects and the
  // frame loop read current limits WITHOUT taking sceneBounds as a dependency
  // (which would re-fire the cinematic moves on every layout tick).
  const fitRef = useRef(fit);
  fitRef.current = fit;

  // Grow the camera far-plane to contain the scaled dolly range.
  useEffect(() => {
    const cam = camera as THREE.PerspectiveCamera;
    if (cam.far !== fit.far) {
      cam.far = fit.far;
      cam.updateProjectionMatrix();
    }
  }, [camera, fit.far]);

  // Bounds-framed overview/home pose (replaces the old static one). When locked (the
  // radar board) the camera sits straight on the +Z axis looking at board centre (up
  // +Y), so the rails read horizontal with no perspective tilt; otherwise it frames
  // along the pleasant 3/4 hero angle.
  function writeOverviewPose(target: THREE.Vector3, pos: THREE.Vector3) {
    const f = fitRef.current;
    const overviewDir = locked ? STRAIGHT_ON_DIR : OVERVIEW_DIR;
    target.copy(f.center);
    pos.copy(f.center).addScaledVector(overviewDir, f.overviewDist);
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
        2.6 + Math.max(0.6, selected.radius) * 3.4,
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
    } else {
      wasSelected.current = false;
      if (homePos.current) {
        targetGoal.current.copy(homeTarget.current);
        posGoal.current.copy(homePos.current);
      } else {
        writeOverviewPose(targetGoal.current, posGoal.current);
      }
    }
    animating.current = true;
    flyActive.current = false;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  // --- Cinematic fly-to framing on focusBounds change (preserve view angle). ---
  useEffect(() => {
    const c = controls.current;

    if (focusBounds) {
      const key = `${focusBounds.center[0]},${focusBounds.center[1]},${focusBounds.center[2]}:${focusBounds.radius}`;
      if (key === lastFocusKey.current) return;
      lastFocusKey.current = key;

      if (homePos.current == null && c) {
        homeTarget.current.copy(c.target);
        homePos.current = new THREE.Vector3().copy(c.object.position);
      }

      targetGoal.current.set(focusBounds.center[0], focusBounds.center[1], focusBounds.center[2]);
      const fov = c ? c.object.fov : FOV_FAR;
      const dist = THREE.MathUtils.clamp(frameDistance(focusBounds.radius, fov), MIN_DIST, fitRef.current.maxDist);
      if (c) {
        dir.copy(c.object.position).sub(c.target);
        if (dir.lengthSq() < 1e-6) dir.set(0, 0, 1);
        dir.normalize();
      } else {
        dir.set(0, 0, 1);
      }
      posGoal.current.copy(targetGoal.current).addScaledVector(dir, dist);
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
  }, [focusBounds]);

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
    const sig = `${f.center.x.toFixed(1)},${f.center.y.toFixed(1)},${f.center.z.toFixed(1)}:${f.radius.toFixed(1)}`;
    if (sig === lastFitSig.current) return;
    lastFitSig.current = sig;
    writeOverviewPose(targetGoal.current, posGoal.current);
    homeTarget.current.copy(targetGoal.current);
    homePos.current = new THREE.Vector3().copy(posGoal.current);
    beginFly();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fit, locked, selected, focusBounds]);

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
      // Wheel/trackpad dolly was too twitchy at the default 1.0; calm it so a scroll
      // nudges the zoom rather than lurching it.
      zoomSpeed={0.6}
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
