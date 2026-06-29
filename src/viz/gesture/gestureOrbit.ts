// Gesture → orbit adapter. Ported from orbit-snap's `updateCameraOrbit`
// (src/lib/orbitCamera.ts), but instead of mutating a camera view it returns the
// per-frame angular DELTAS (radians) to add onto a drei <OrbitControls> each
// frame. Downstream damping (OrbitControls / MathUtils.damp) handles the settle,
// so on release we simply emit zeros.
//
// Pure TypeScript: no three.js / React imports.
import type { Pinch } from './types';

// ── Ported gains (orbit-snap orbitCamera.ts, values preserved exactly) ──
// orbit-snap fed `azimuth`/`elevation` (radians) into sin/cos to place the
// camera, so these gains produce radians directly and port over unchanged.
export const ORBIT_AZIMUTH_GAIN = 4.2; // base azimuth response per unit dx
export const ORBIT_ELEVATION_GAIN = 3.1; // base elevation response per unit dy
export const ORBIT_THROW_GAIN = 12; // azimuth "pointer throw": super-linear boost ∝ |dx|
export const ELEVATION_THROW_GAIN = 6; // elevation "pointer throw": super-linear boost ∝ |dy|

// orbit-snap's "regrab" guard: if the primary pinch jumps further than this in
// one frame (a tracking glitch / a different hand grabbing), don't translate it
// into motion — re-anchor the drag instead. Preserved from REGRAB_DISTANCE.
export const REGRAB_DISTANCE = 0.86;

// NOT in orbit-snap (it had no per-frame angular clamp, only a radius clamp).
// Added per the WARDEN spec so a single bad landmark frame can't fling the
// camera: each delta is hard-capped to ±this many radians/frame.
export const MAX_DELTA_PER_FRAME = 0.25;

// Two-hand ZOOM clamp (normalized hand-to-hand spread units / frame). A glitchy
// landmark frame can change the measured spread wildly; this caps how far the radius
// can move per frame so a blip can't snap the camera across the whole zoom range.
export const MAX_SPREAD_DELTA_PER_FRAME = 0.15;

// SIGN CONVENTION (be internally consistent; final direction reconciled at the
// integration layer):
//   dx = pinch.x - lastPoint.x  → moving the pinch toward +x (rightward in the
//        normalized image) yields a POSITIVE azimuthDelta.
//   dy = lastPoint.y - pinch.y  → orbit-snap INVERTS y (image y grows downward),
//        so moving the pinch UP on screen yields a POSITIVE elevationDelta.
// In short: positive azimuthDelta = pinch dragged right; positive
// elevationDelta = pinch dragged up.

export interface OrbitGestureOutput {
  azimuthDelta: number;
  elevationDelta: number;
  /** Two-hand dolly. Positive = the two hands spreading apart. 0 in single-pinch rotate mode. */
  radiusDelta: number;
  dragging: boolean;
}

// Internal session state. `lastPoint` is the primary pinch's position last frame
// (null = no active rotate drag); `primaryPinchId` is the pinch we're tracking (the
// lowest id, matching orbit-snap's id-sorted primary selection). `lastSpread` is the
// two-hand distance last frame (null = no active zoom gesture).
export interface OrbitGestureState {
  primaryPinchId: number | null;
  lastPoint: { x: number; y: number } | null;
  lastSpread: number | null;
}

export function createOrbitGestureState(): OrbitGestureState {
  return { primaryPinchId: null, lastPoint: null, lastSpread: null };
}

function round(value: number): number {
  const r = Number(value.toFixed(4));
  // Normalize -0 → 0: a tiny negative delta rounds to -0, and Object.is(-0, 0) is
  // false, so a -0 would slip past an exact `=== 0` / `toBe(0)` "no motion" check.
  return r === 0 ? 0 : r;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function distance(a: { x: number; y: number }, b: { x: number; y: number }): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

// orbit-snap's "pointer throw": response = sign(delta) * |delta| * (base + |delta|*throw).
// The |delta|*throw term makes a fast flick travel disproportionately further
// than a slow nudge (super-linear), giving the gesture a momentum-like feel.
function pointerThrow(delta: number, baseGain: number, throwGain: number): number {
  const magnitude = Math.abs(delta);
  return round(Math.sign(delta) * magnitude * (baseGain + magnitude * throwGain));
}

const IDLE: OrbitGestureOutput = { azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false };

export function updateOrbitGesture(
  state: OrbitGestureState,
  pinches: Pinch[],
): OrbitGestureOutput {
  // No pinch held → reset both sessions (rotate + zoom) and report idle.
  if (pinches.length === 0) {
    state.primaryPinchId = null;
    state.lastPoint = null;
    state.lastSpread = null;
    return IDLE;
  }

  const sorted = [...pinches].sort((a, b) => a.id - b.id);

  // ── Two (or more) pinches → ZOOM (dolly); rotation is suppressed. ────────────
  // The distance between the two lowest-id pinch points is the "spread"; its
  // frame-to-frame change drives the radius. Single-hand rotation never runs while
  // two hands are pinching, so the gestures can't fight. We also clear the rotate
  // anchor so dropping back to one pinch re-grabs cleanly instead of lurching.
  if (sorted.length >= 2) {
    state.primaryPinchId = null;
    state.lastPoint = null;

    const spread = distance(sorted[0].point, sorted[1].point);

    // First frame of a two-hand gesture (or just switched from rotating): seed the
    // spread, emit no motion.
    if (state.lastSpread === null) {
      state.lastSpread = spread;
      return { azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false };
    }

    const dSpread = spread - state.lastSpread;

    // Regrab guard: an implausibly large spread jump (a hand popping in/out of frame)
    // re-anchors rather than lurching the zoom.
    if (Math.abs(dSpread) > REGRAB_DISTANCE) {
      state.lastSpread = spread;
      return { azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false };
    }

    state.lastSpread = spread;
    // Positive radiusDelta = hands spreading apart. The integration layer maps it to
    // dolly direction (CameraRig's GESTURE_ZOOM_SIGN) and world units (GESTURE_ZOOM_GAIN).
    const radiusDelta = round(
      clamp(dSpread, -MAX_SPREAD_DELTA_PER_FRAME, MAX_SPREAD_DELTA_PER_FRAME),
    );
    return { azimuthDelta: 0, elevationDelta: 0, radiusDelta, dragging: false };
  }

  // ── One pinch → ROTATE (existing behaviour). ────────────────────────────────
  // Leaving zoom: clear the spread anchor so a later two-hand gesture re-seeds.
  state.lastSpread = null;

  // Primary = lowest-id pinch, but stick with the one we were already tracking
  // if it's still present (matches orbit-snap's primaryPinchId behaviour).
  const primary =
    state.primaryPinchId !== null
      ? (sorted.find((p) => p.id === state.primaryPinchId) ?? sorted[0])
      : sorted[0];

  // First frame of this drag (or the primary pinch changed identity, or we lost
  // the anchor): seed lastPoint, emit no motion, but report dragging=true.
  if (state.primaryPinchId !== primary.id || !state.lastPoint) {
    state.primaryPinchId = primary.id;
    state.lastPoint = { ...primary.point };
    return { azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true };
  }

  // Regrab guard: an implausibly large jump in one frame re-anchors instead of
  // flinging the camera. Still dragging, just no delta this frame.
  if (distance(state.lastPoint, primary.point) > REGRAB_DISTANCE) {
    state.lastPoint = { ...primary.point };
    return { azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true };
  }

  const dx = primary.point.x - state.lastPoint.x;
  const dy = state.lastPoint.y - primary.point.y; // inverted: screen-up = +elevation

  const azimuthDelta = clamp(
    pointerThrow(dx, ORBIT_AZIMUTH_GAIN, ORBIT_THROW_GAIN),
    -MAX_DELTA_PER_FRAME,
    MAX_DELTA_PER_FRAME,
  );
  const elevationDelta = clamp(
    pointerThrow(dy, ORBIT_ELEVATION_GAIN, ELEVATION_THROW_GAIN),
    -MAX_DELTA_PER_FRAME,
    MAX_DELTA_PER_FRAME,
  );

  state.lastPoint = { ...primary.point };

  return { azimuthDelta, elevationDelta, radiusDelta: 0, dragging: true };
}
