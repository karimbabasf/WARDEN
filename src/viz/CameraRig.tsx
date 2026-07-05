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
// This file also tames the close-zoom fisheye (FOV taper), locks the orbit pivot
// to the constellation centre (no pan; zoom dollies to centre, not the cursor) so
// the cluster can never be roamed off-screen, and adds a cinematic fly-to that
// frames a bounded subtree (`focusBounds`) over ~700ms with an expo ease — again
// preserving the current viewing angle, and easing back to the overview pose
// when the bounds clear.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame, useThree } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from './orbTypes';
import { frameDistance, type Bounds } from './cameraFraming';
import { cameraTargetForOrbitOverview, radarHeroPose } from './useOrbCamera';
import { fitDistanceForBounds } from './cameraFit';
import { gestureBus, drainGesture } from './gesture/gestureBus';

// Pulled back from the old 9.4 so the (now more widely spaced) constellation
// opens with room to breathe instead of filling the frame.
const OVERVIEW_DIST = 12.6;
// Raised from 3 → 5: at 3 the camera sat so close that the wide-angle lens
// bent the scene (fisheye). 5 keeps you out of that distortion zone, and the
// FOV taper below mops up whatever remains on the closest approach.
const MIN_DIST = 5;
// Raised 24 → 42 so a BUSY fleet (many folders, each a full subagent sphere) can be
// pulled all the way back into frame. The volumetric layout spreads families across a
// horizontal field; at 25–40 agents the outer families sit well past the old 24-unit
// wall, so that wall stranded them off-screen with no way to zoom out to them.
const MAX_DIST = 42;

// Polar (vertical) orbit clamp — keep the constellation upright-ish, never tipping over
// the poles. Shared by the OrbitControls props AND the hand-gesture clamp below so mouse
// and hand obey the exact same vertical limits.
const MIN_POLAR = Math.PI * 0.16;
const MAX_POLAR = Math.PI * 0.84;

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

// Fly-to framing timing — an explicit ~700ms expo ease-in-out (per spec), so the
// move reads as a deliberate cinematic push rather than the springy settle used
// for orb selection.
const FLY_MS = 700;

// ── idle auto-orbit (cinematic "life") ─────────────────────────────────────────
// When the rig is at rest and nothing is selected, the camera drifts VERY slowly in
// azimuth so the constellation reads as a living object rather than a frozen render.
// Deliberately gentle (~one full revolution every ~2.6 min) so it never induces
// motion sickness and never fights the user. ANY interaction (mouse, wheel, gesture)
// pauses it; it resumes only after the scene has been untouched for IDLE_RESUME_MS.
const IDLE_ORBIT_SPEED = 0.04; // rad/s — ~2.6 min per revolution
const IDLE_RESUME_MS = 2600; // quiet time after the last interaction before drift resumes

// Re-fit hysteresis. We recompute the auto-fit as the fleet grows/shrinks, but only
// re-frame when the target or distance moved MATERIALLY (spawning one subagent must
// not yank a settled camera). Compared against the last applied fit.
const REFIT_TARGET_EPS = 1.2; // world units the centroid must move to trigger a re-fit
const REFIT_DIST_EPS = 2.0; // world units the fit distance must change to trigger a re-fit
const REFIT_DEBOUNCE_MS = 450; // settle window so a burst of spawns coalesces into one re-frame

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    Boolean(window.matchMedia?.('(prefers-reduced-motion: reduce)').matches)
  );
}

const dir = new THREE.Vector3();
// Hoisted scratch for the idle-orbit spherical rotation (never allocate in useFrame).
const idleOffset = new THREE.Vector3();
const idleSpherical = new THREE.Spherical();

// Hand-gesture steering (pinch-drag → orbit), fed frame-by-frame on the gestureBus.
// Deltas arrive in radians (already throw-shaped + clamped by gestureOrbit). We rotate
// the camera around the orbit target in spherical space, then the existing controls
// `update()` reconciles it — the supported "manual transform + update()" path, so the
// hand shares the mouse's exact damping + clamps. Flip a SIGN if a direction feels
// inverted on your camera; nudge GAIN for a livelier/calmer hand.
const GESTURE_AZIMUTH_SIGN = 1;
const GESTURE_POLAR_SIGN = -1;
const GESTURE_GAIN = 1;
// Two-hand zoom: spreading the hands apart dollies IN (negative radius change), like
// phone pinch-to-zoom. Flip GESTURE_ZOOM_SIGN to invert; GESTURE_ZOOM_GAIN converts the
// normalized hand-spread delta into world distance units.
const GESTURE_ZOOM_SIGN = -1;
const GESTURE_ZOOM_GAIN = 18;
// Hoisted scratch — never allocate inside useFrame (R3F perf rule).
const gestureOffset = new THREE.Vector3();
const gestureSpherical = new THREE.Spherical();

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
  homeSignal = 0,
  fitPoints,
  reframeSignal = 0,
  idleOrbit = true,
}: {
  selected: LayoutNode | null;
  // Fly-to target: when non-null we frame this bounded subtree; when it returns
  // to null we ease back to the overview/home pose. Defaults to null so callers
  // that don't drive it yet (and the type-checker) are happy.
  focusBounds?: Bounds | null;
  homeSignal?: number;
  // Live node centres (globe positions). When provided (the radar path) the rig
  // AUTO-FITS: the overview/home pose is computed to frame these points at the
  // composed hero angle, and a material change in their bounds re-frames (debounced).
  // Omitted (the standalone dev <Canvas>) → the rig keeps the fixed overview pose.
  fitPoints?: { x: number; y: number; z: number }[];
  // Monotonic "reframe / fit" signal — bump it to recall the auto-fit and ease back
  // to the hero pose from any orbit state (the radar dock's reframe control).
  reframeSignal?: number;
  // Gentle idle auto-orbit when at rest (paused on interaction, reduced-motion aware).
  idleOrbit?: boolean;
}) {
  // OrbitControls instance — typed loosely to avoid importing three's controls type.
  const controls = useRef<any>(null);
  const { size } = useThree();
  const aspect = size.width > 0 && size.height > 0 ? size.width / size.height : 16 / 9;

  // The current camera FOV (radians) for the auto-fit. The Canvas mounts a 46° lens
  // (the FOV taper below narrows it near MIN_DIST, but framing uses the resting lens).
  const fovRad = (FOV_FAR * Math.PI) / 180;

  // Auto-fit home pose (the composed hero shot framing the live fleet). Recomputed
  // whenever the fleet's node positions or the viewport aspect change; when no
  // `fitPoints` are supplied it falls back to the fixed OrbitControls overview.
  const heroHome = useMemo(() => {
    if (!fitPoints || fitPoints.length === 0) {
      const o = cameraTargetForOrbitOverview();
      return { position: { ...o.position }, lookAt: { ...o.lookAt } };
    }
    const fit = fitDistanceForBounds(fitPoints, fovRad, aspect);
    // Clamp the framing distance into the rig's dolly band so the hero shot always
    // sits somewhere the wheel can also reach (never outside MIN/MAX_DIST).
    fit.distance = THREE.MathUtils.clamp(fit.distance, MIN_DIST, MAX_DIST);
    return radarHeroPose(fit);
    // fovRad is a module-derived constant; aspect + fitPoints are the real inputs.
  }, [fitPoints, aspect, fovRad]);
  // The pose we're animating toward — both the orbit target AND the camera position,
  // lerped together so focus-in and back-out are one consistent motion. Seeded from
  // the composed hero home so the very first settle already frames the fleet.
  const targetGoal = useRef(new THREE.Vector3(heroHome.lookAt.x, heroHome.lookAt.y, heroHome.lookAt.z));
  const posGoal = useRef(new THREE.Vector3(heroHome.position.x, heroHome.position.y, heroHome.position.z));
  const animating = useRef(false);
  const wasSelected = useRef(false);
  // The exact pose (camera position + orbit target) the user was viewing from BEFORE
  // diving into an orb. Captured on the overview→focus edge and restored on back-out.
  const homeTarget = useRef(new THREE.Vector3(0, 0, 0));
  const homePos = useRef<THREE.Vector3 | null>(null);
  // Live mirror of the current hero home so the frame loop + effects can read the
  // latest auto-fit pose without stale closures. Updated whenever `heroHome` changes.
  const heroHomeRef = useRef(heroHome);
  heroHomeRef.current = heroHome;

  // Idle auto-orbit bookkeeping: the timestamp of the last interaction (mouse / wheel
  // / gesture / programmatic move) — drift only resumes IDLE_RESUME_MS after it, and
  // never while animating, focused, or reduced-motion. Seeded to "now" so the scene
  // holds still briefly on open before it begins to breathe.
  const lastInteractionMs = useRef(typeof performance !== 'undefined' ? performance.now() : 0);
  const reducedMotion = useRef(prefersReducedMotion());

  // Re-fit hysteresis state: the last fit we actually APPLIED (so a tiny bounds nudge
  // doesn't re-frame) + a debounce deadline so a burst of spawns coalesces.
  const appliedFit = useRef<{ target: THREE.Vector3; distance: number } | null>(null);
  const refitDeadline = useRef<number | null>(null);
  const lastReframeSignal = useRef(reframeSignal);

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
  const lastHomeSignal = useRef(homeSignal);

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

  // Stamp "the user just touched the camera" — pauses idle drift and restarts its
  // resume timer. Fired by OrbitControls onStart (mouse/wheel) and by every gesture
  // frame, so any input immediately quiets the drift.
  function markInteraction() {
    lastInteractionMs.current = typeof performance !== 'undefined' ? performance.now() : Date.now();
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

  // Ease to the current HOME pose — the fit-based composed hero shot on the radar
  // path (freshest auto-fit via `heroHomeRef`), the fixed overview otherwise. Shared
  // by the double-click-home signal and the reframe control so both land on the same
  // intentional frame. Records it as the new dive-from home + resets focus/idle state.
  function goHome() {
    const home = heroHomeRef.current;
    targetGoal.current.set(home.lookAt.x, home.lookAt.y, home.lookAt.z);
    posGoal.current.set(home.position.x, home.position.y, home.position.z);
    homeTarget.current.copy(targetGoal.current);
    homePos.current = new THREE.Vector3(home.position.x, home.position.y, home.position.z);
    appliedFit.current = {
      target: new THREE.Vector3(home.lookAt.x, home.lookAt.y, home.lookAt.z),
      distance: posGoal.current.distanceTo(targetGoal.current),
    };
    wasSelected.current = false;
    lastFocusKey.current = null;
    refitDeadline.current = null;
    markInteraction(); // a home/reframe is a deliberate move — hold idle drift after it
    beginFly();
  }

  useEffect(() => {
    if (homeSignal === lastHomeSignal.current) return;
    lastHomeSignal.current = homeSignal;
    goHome();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [homeSignal]);

  // Reframe / fit control — recall the auto-fit and ease back to the hero pose from
  // any orbit state. Same landing as double-click-home, driven by an explicit signal
  // the radar dock bumps.
  useEffect(() => {
    if (reframeSignal === lastReframeSignal.current) return;
    lastReframeSignal.current = reframeSignal;
    goHome();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reframeSignal]);

  // Auto-refit as the fleet's bounds change materially. We DON'T re-frame on every
  // spawn (that would yank a settled camera every time a subagent appears); instead,
  // when the freshly computed `heroHome` diverges from the last APPLIED fit by more
  // than the epsilons, we arm a short debounce and, once it elapses, ease to the new
  // frame — but only while the camera is genuinely at rest (nothing selected, not
  // mid-animation), so it never interrupts a dive or a user's manual orbit.
  useEffect(() => {
    if (!fitPoints || fitPoints.length === 0) return;
    const home = heroHome;
    const nextTarget = new THREE.Vector3(home.lookAt.x, home.lookAt.y, home.lookAt.z);
    const nextDist = new THREE.Vector3(home.position.x, home.position.y, home.position.z).distanceTo(nextTarget);
    const prev = appliedFit.current;
    // First fit (no baseline yet): adopt it silently as the resting frame — the goal
    // refs were already seeded from heroHome, so no fly is needed on the cold open.
    if (!prev) {
      appliedFit.current = { target: nextTarget, distance: nextDist };
      return;
    }
    const moved =
      prev.target.distanceTo(nextTarget) > REFIT_TARGET_EPS ||
      Math.abs(prev.distance - nextDist) > REFIT_DIST_EPS;
    if (moved) {
      refitDeadline.current = (typeof performance !== 'undefined' ? performance.now() : Date.now()) + REFIT_DEBOUNCE_MS;
    }
    // The frame loop consumes `refitDeadline` (below) so the actual re-frame waits for
    // the debounce AND for the camera to be at rest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [heroHome, fitPoints]);

  useFrame((_, dtRaw) => {
    const c = controls.current;
    if (!c) return;
    const dt = Math.min(dtRaw, 0.05);

    // --- Hand-gesture orbit ---------------------------------------------------
    // Drain the pinch-drag rotation the tracker accumulated since the last frame and
    // apply it around the current orbit target in spherical space. We only steer while
    // the rig is at REST (a fly-to/focus owns the camera), but we still drain during an
    // animation so queued deltas can't pile up and lurch when it ends. The c.update() at
    // the foot of this frame reconciles the manual move with OrbitControls' state.
    if (gestureBus.enabled) {
      const { theta, phi, radius } = drainGesture();
      if (!animating.current && (theta !== 0 || phi !== 0 || radius !== 0)) {
        markInteraction(); // hand steering counts as interaction → pause idle drift
        gestureOffset.copy(c.object.position).sub(c.target);
        gestureSpherical.setFromVector3(gestureOffset);
        gestureSpherical.theta += theta * GESTURE_AZIMUTH_SIGN * GESTURE_GAIN;
        gestureSpherical.phi = THREE.MathUtils.clamp(
          gestureSpherical.phi + phi * GESTURE_POLAR_SIGN * GESTURE_GAIN,
          MIN_POLAR,
          MAX_POLAR,
        );
        // Two-hand spread → dolly, clamped to the SAME distance band the mouse wheel
        // uses, so hand-zoom and scroll-zoom share one set of limits.
        gestureSpherical.radius = THREE.MathUtils.clamp(
          gestureSpherical.radius + radius * GESTURE_ZOOM_SIGN * GESTURE_ZOOM_GAIN,
          MIN_DIST,
          MAX_DIST,
        );
        gestureSpherical.makeSafe();
        gestureOffset.setFromSpherical(gestureSpherical);
        c.object.position.copy(c.target).add(gestureOffset);
      }
    }

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

    // --- At rest: debounced auto-refit + gentle idle auto-orbit ----------------
    // Only when the camera is genuinely idle (not mid-animation, no orb selected, no
    // fly-to bounds active) do we (a) consume a pending re-fit and (b) drift. This is
    // the single gate that keeps the auto-frame and the cinematic breath from ever
    // fighting a dive or the user's own orbit.
    const atRest = !animating.current && !selected && !focusBounds;
    if (atRest) {
      const nowMs = typeof performance !== 'undefined' ? performance.now() : Date.now();

      // Debounced re-frame: a materially-changed fleet armed `refitDeadline`; once it
      // elapses AND we're at rest, ease to the freshest hero home (a soft settle, not
      // the hard 700ms fly, so a spawn-driven reframe feels ambient rather than abrupt).
      if (refitDeadline.current !== null && nowMs >= refitDeadline.current) {
        refitDeadline.current = null;
        const home = heroHomeRef.current;
        targetGoal.current.set(home.lookAt.x, home.lookAt.y, home.lookAt.z);
        posGoal.current.set(home.position.x, home.position.y, home.position.z);
        homeTarget.current.copy(targetGoal.current);
        homePos.current = (homePos.current ?? new THREE.Vector3()).copy(posGoal.current);
        appliedFit.current = {
          target: new THREE.Vector3(home.lookAt.x, home.lookAt.y, home.lookAt.z),
          distance: posGoal.current.distanceTo(targetGoal.current),
        };
        animating.current = true; // damped settle (flyActive stays false)
      } else if (
        idleOrbit &&
        !reducedMotion.current &&
        refitDeadline.current === null &&
        nowMs - lastInteractionMs.current > IDLE_RESUME_MS
      ) {
        // Gentle azimuth drift around the orbit target — the constellation slowly
        // turns so the scene reads alive. Applied in spherical space then reconciled
        // by c.update() below, exactly like the gesture path, so it shares the same
        // pivot + clamps. Polar (elevation) is left untouched — only azimuth breathes.
        idleOffset.copy(c.object.position).sub(c.target);
        idleSpherical.setFromVector3(idleOffset);
        idleSpherical.theta += IDLE_ORBIT_SPEED * dt;
        idleSpherical.makeSafe();
        idleOffset.setFromSpherical(idleSpherical);
        c.object.position.copy(c.target).add(idleOffset);
        // Keep the goal refs in lockstep so a subsequent dive-out doesn't snap back to
        // the pre-drift azimuth (the drift IS the new resting pose).
        posGoal.current.copy(c.object.position);
        if (homePos.current) homePos.current.copy(c.object.position);
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

    c.update();
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      enableDamping
      dampingFactor={0.15}
      rotateSpeed={0.95}
      zoomSpeed={1.0}
      // A manual drag/wheel START pauses the idle auto-orbit and restarts its resume
      // timer, so the drift never fights the user — it only breathes when untouched.
      // (Only `onStart`, never `onChange`: `onChange` fires on our own per-frame
      // c.update() too, which would re-stamp every drift frame and suppress itself.)
      onStart={markInteraction}
      // Locked pivot: zoom dollies toward the orbit centre (NOT the cursor) and pan
      // is OFF, so the constellation stays pinned in frame. Browsing is then always
      // a clean turntable orbit around the cluster — you can't slide the pivot off
      // into empty space and lose your bearings.
      enablePan={false}
      minDistance={MIN_DIST}
      maxDistance={MAX_DIST}
      // Keep the constellation upright-ish; allow looking from above/below but
      // never fully over the poles (avoids the disorienting flip).
      minPolarAngle={MIN_POLAR}
      maxPolarAngle={MAX_POLAR}
    />
  );
}

export default CameraRig;
