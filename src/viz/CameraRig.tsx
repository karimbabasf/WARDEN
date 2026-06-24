// CameraRig.tsx — free orbit + cinematic focus + fly-to framing.
//
// The old camera could only snap between a fixed overview and a fixed per-node
// pose (no drag at all), which is exactly why the scene "felt locked". This rig
// gives drei OrbitControls with inertia (drag to spin the constellation like a
// 3D model, scroll to dolly) AND a damped focus move: when an orb is selected we
// glide the orbit target onto it and pull the camera in, KEEPING the user's
// current viewing angle (we only recenter + change distance). Crucially, we also
// REMEMBER the exact pose the user dove FROM, and restore it verbatim when they
// back out — so zooming out never leaves the camera tilted at a strange angle.
//
// This file also tames the close-zoom fisheye (FOV taper), keeps panning from
// losing the constellation (clamped target), and adds a cinematic fly-to that
// frames a bounded subtree (`focusBounds`) over ~700ms with an expo ease — again
// preserving the current viewing angle, and easing back to the overview pose
// when the bounds clear.

import { useEffect, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from './orbTypes';
import { frameDistance, type Bounds } from './cameraFraming';

// Pulled back from the old 9.4 so the (now more widely spaced) constellation
// opens with room to breathe instead of filling the frame.
const OVERVIEW_DIST = 12.6;
// Raised from 3 → 5: at 3 the camera sat so close that the wide-angle lens
// bent the scene (fisheye). 5 keeps you out of that distortion zone, and the
// FOV taper below mops up whatever remains on the closest approach.
const MIN_DIST = 5;
const MAX_DIST = 24;

// FOV taper. The Canvas mounts the perspective camera at 46° (see WarRoom).
// As the camera's distance to its target approaches MIN_DIST we ease the FOV
// down toward FOV_NEAR — a narrower lens flattens perspective and counteracts
// the wide-angle stretch you get up close. `FOV_TAPER_START` is the distance at
// which the taper begins; beyond it the lens stays at its natural 46°.
const FOV_FAR = 46;
const FOV_NEAR = 38;
const FOV_TAPER_START = 9;
// Below this projection-matrix delta we skip updateProjectionMatrix() — no point
// reuploading the matrix for a sub-hundredth-of-a-degree change every frame.
const FOV_EPS = 0.01;

// Constrained pan. OrbitControls pan moves the *target*; left unbounded you can
// drift the whole constellation out of frame and "lose" it. We clamp the target
// to a sphere around the scene origin so roaming always keeps the cluster
// reachable. Generous enough to inspect edges, tight enough not to get lost.
const PAN_BOUND_RADIUS = 14;
const panCenter = new THREE.Vector3(0, 0, 0);

// Fly-to framing timing — an explicit ~700ms expo ease-in-out (per spec), so the
// move reads as a deliberate cinematic push rather than the springy settle used
// for orb selection.
const FLY_MS = 700;

const dir = new THREE.Vector3();

// Expo ease-in-out on a normalized 0..1 clock. Slow lift-off, fast middle, soft
// landing — the classic "camera move" feel.
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
}: {
  selected: LayoutNode | null;
  // Fly-to target: when non-null we frame this bounded subtree; when it returns
  // to null we ease back to the overview/home pose. Defaults to null so callers
  // that don't drive it yet (and the type-checker) are happy.
  focusBounds?: Bounds | null;
}) {
  // OrbitControls instance — typed loosely to avoid importing three's controls type.
  const controls = useRef<any>(null);
  // The pose we're animating toward — both the orbit target AND the camera position,
  // lerped together so focus-in and back-out are one consistent motion.
  const targetGoal = useRef(new THREE.Vector3(0, 0, 0));
  const posGoal = useRef(new THREE.Vector3(0, 1, OVERVIEW_DIST));
  const animating = useRef(false);
  const wasSelected = useRef(false);
  // The exact pose (camera position + orbit target) the user was viewing from BEFORE
  // diving into an orb. Captured on the overview→focus edge and restored on back-out.
  const homeTarget = useRef(new THREE.Vector3(0, 0, 0));
  const homePos = useRef<THREE.Vector3 | null>(null);

  // Timed fly-to state. When `flyActive` is set we interpolate from a captured
  // start pose to the goal pose over FLY_MS using the expo ease, taking priority
  // over the damped-lerp path. The decay-lerp then settles any residual.
  const flyActive = useRef(false);
  const flyClock = useRef(0);
  const flyFromTarget = useRef(new THREE.Vector3());
  const flyFromPos = useRef(new THREE.Vector3());
  // Edge detector for focusBounds (compare by value — center + radius — so a
  // re-rendered-but-identical Bounds object doesn't retrigger the flight).
  const lastFocusKey = useRef<string | null>(null);

  // Kick off a timed fly-to toward the current goal poses (already set by the
  // caller below). Captures the live pose as the interpolation start.
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

  // --- Orb selection focus (unchanged behaviour: damped glide that preserves
  // angle, with verbatim home capture/restore on the focus edge). ---
  useEffect(() => {
    const c = controls.current;
    if (selected) {
      // Capture the dive-from pose ONCE, on the null→selected edge, so a back-out
      // returns exactly here (orb→orb jumps keep the original home).
      if (!wasSelected.current && c) {
        homeTarget.current.copy(c.target);
        homePos.current = (homePos.current ?? new THREE.Vector3()).copy(c.object.position);
      }
      wasSelected.current = true;

      targetGoal.current.set(selected.position.x, selected.position.y, selected.position.z);
      const dist = THREE.MathUtils.clamp(2.6 + Math.max(0.6, selected.radius) * 3.4, MIN_DIST, MAX_DIST);
      // Glide in along the CURRENT viewing direction (preserve the user's angle):
      // recenter on the orb, sit `dist` back along the existing view ray.
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
      // Restore the captured dive-from pose verbatim so backing out returns to the
      // exact angle + zoom the user left — never thrown off. Fallback to a canonical
      // overview only if nothing was ever captured (shouldn't happen in practice).
      if (homePos.current) {
        targetGoal.current.copy(homeTarget.current);
        posGoal.current.copy(homePos.current);
      } else {
        targetGoal.current.set(0, 0, 0);
        posGoal.current.set(0, 1, OVERVIEW_DIST);
      }
    }
    animating.current = true;
    // A selection move cancels any in-flight fly-to (they target the same poses).
    flyActive.current = false;
  }, [selected]);

  // --- Cinematic fly-to framing. On a focusBounds *change*, frame the bounded
  // subtree by easing camera + target over FLY_MS, PRESERVING the current view
  // angle (we recenter on the bounds centre and dolly along the existing ray to
  // the frameDistance). When focusBounds clears, ease back to the home/overview
  // pose the same way. ---
  useEffect(() => {
    const c = controls.current;

    if (focusBounds) {
      const key = `${focusBounds.center[0]},${focusBounds.center[1]},${focusBounds.center[2]}:${focusBounds.radius}`;
      if (key === lastFocusKey.current) return; // identical bounds — nothing to do.
      lastFocusKey.current = key;

      // Capture the dive-from pose once, on the overview→framed edge, so clearing
      // the bounds returns to exactly where the user was (mirrors orb focus).
      if (homePos.current == null && c) {
        homeTarget.current.copy(c.target);
        homePos.current = new THREE.Vector3().copy(c.object.position);
      }

      targetGoal.current.set(focusBounds.center[0], focusBounds.center[1], focusBounds.center[2]);
      const fov = c ? c.object.fov : FOV_FAR;
      const dist = THREE.MathUtils.clamp(frameDistance(focusBounds.radius, fov), MIN_DIST, MAX_DIST);
      // Preserve the current viewing direction (don't snap to a canned angle).
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
      // Cleared. If we were framed, ease back to the captured home pose (or the
      // canonical overview if none was captured), again over the timed expo.
      if (lastFocusKey.current !== null) {
        lastFocusKey.current = null;
        if (homePos.current) {
          targetGoal.current.copy(homeTarget.current);
          posGoal.current.copy(homePos.current);
        } else {
          targetGoal.current.set(0, 0, 0);
          posGoal.current.set(0, 1, OVERVIEW_DIST);
        }
        beginFly();
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusBounds]);

  useFrame((_, dtRaw) => {
    const c = controls.current;
    if (!c) return;
    const dt = Math.min(dtRaw, 0.05);

    if (animating.current) {
      if (flyActive.current) {
        // Timed expo ease-in-out over FLY_MS. Interpolate from the captured start
        // pose to the goal pose so the motion lands precisely on a known frame.
        flyClock.current += dt * 1000;
        const t = Math.min(1, flyClock.current / FLY_MS);
        const e = easeInOutExpo(t);
        c.target.copy(flyFromTarget.current).lerp(targetGoal.current, e);
        c.object.position.copy(flyFromPos.current).lerp(posGoal.current, e);
        if (t >= 1) {
          flyActive.current = false;
          // Snap exactly onto the goal, then let the settle check below stop us.
          c.target.copy(targetGoal.current);
          c.object.position.copy(posGoal.current);
        }
      } else {
        // Damped exponential glide (orb-selection focus / residual settle).
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

    // FOV taper — counteract close-zoom fisheye. Map the live camera→target
    // distance onto [FOV_NEAR, FOV_FAR]: at/under MIN_DIST use the narrow lens,
    // at/over FOV_TAPER_START use the natural lens, smoothstep between. Damp the
    // actual fov toward that target and only reupload the projection matrix on
    // frames where it meaningfully moved.
    const camDist = c.object.position.distanceTo(c.target);
    const taper = THREE.MathUtils.smoothstep(camDist, MIN_DIST, FOV_TAPER_START); // 0 near → 1 far
    const fovTarget = THREE.MathUtils.lerp(FOV_NEAR, FOV_FAR, taper);
    const fovK = 1 - Math.exp(-7 * dt);
    const nextFov = THREE.MathUtils.lerp(c.object.fov, fovTarget, fovK);
    if (Math.abs(nextFov - c.object.fov) > FOV_EPS) {
      c.object.fov = nextFov;
      c.object.updateProjectionMatrix();
    }

    // Constrained pan — keep the orbit target within reach of the constellation
    // so panning can never lose it. Clamp to a sphere around the scene centre.
    const offset = c.target.distanceTo(panCenter);
    if (offset > PAN_BOUND_RADIUS) {
      c.target.sub(panCenter).multiplyScalar(PAN_BOUND_RADIUS / offset).add(panCenter);
    }

    c.update();
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      enableDamping
      dampingFactor={0.085}
      rotateSpeed={0.85}
      zoomSpeed={0.95}
      // Dolly toward whatever the cursor is over (and back out from it) instead of
      // always toward the orbit centre — the scroll zooms into the region you're
      // pointing at. OrbitControls keeps that world point pinned under the mouse.
      zoomToCursor
      // Pan enabled but the target is clamped each frame (see useFrame) so the
      // constellation can never be roamed off-screen.
      enablePan
      panSpeed={0.7}
      // Screen-space pan keeps drag direction intuitive regardless of pitch.
      screenSpacePanning
      minDistance={MIN_DIST}
      maxDistance={MAX_DIST}
      // Keep the constellation upright-ish; allow looking from above/below but
      // never fully over the poles (avoids the disorienting flip).
      minPolarAngle={Math.PI * 0.16}
      maxPolarAngle={Math.PI * 0.84}
    />
  );
}

export default CameraRig;
