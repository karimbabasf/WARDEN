// Pinch recognizer — a faithful port of orbit-snap's `recognizePinches`
// (src/lib/pinchRecognition.ts), adapted to (a) accept raw MediaPipe 21-landmark
// hands instead of pre-extracted samples, and (b) hide the prev-frame state
// inside a stateful tracker object so the React layer just calls `recognize`
// each frame.
//
// Pure TypeScript: no MediaPipe/three/React imports. The only input contract is
// the HandLandmark shape (normalized [0,1] coords) and the MediaPipe Hands
// landmark ordering captured in LANDMARK_INDEX below.
import type { HandLandmark, Pinch } from './types';

// MediaPipe Hands landmark indices (confirmed against orbit-snap App.tsx).
// A "hand" passed to `recognize` is an array indexed by these.
export const LANDMARK_INDEX = {
  WRIST: 0,
  THUMB_MCP: 2,
  THUMB_TIP: 4,
  INDEX_MCP: 5,
  INDEX_TIP: 8,
  MIDDLE_MCP: 9,
  PINKY_MCP: 17,
} as const;

// ── Ported thresholds (orbit-snap DEFAULT_OPTIONS, values preserved exactly) ──
// Two gating regimes exist, mirroring orbit-snap:
//  • If a hand scale can be estimated (the live path for full 21-landmark hands),
//    the thumb/index tip distance is gated as a RATIO of that scale
//    (ACTIVATE_RATIO / RELEASE_RATIO).
//  • If scale can't be estimated (degenerate / missing MCPs), it falls back to
//    ABSOLUTE normalized distances (ACTIVATE_DISTANCE / RELEASE_DISTANCE).
// Either way, RELEASE > ACTIVATE provides the hysteresis band: once a pinch is
// active it stays active until the fingers separate past the (looser) release
// bound — preventing flicker when the tips hover right at the activate edge.
export const ACTIVATE_DISTANCE = 0.066; // absolute-fallback activate (tips together)
export const RELEASE_DISTANCE = 0.104; // absolute-fallback release (tips apart) — hysteresis
export const ACTIVATE_RATIO = 0.5; // tipDist/scale to engage when scale known
export const RELEASE_RATIO = 0.78; // tipDist/scale to drop when scale known — hysteresis
export const MIN_INDEX_EXTENSION_RATIO = 0.24; // index finger must be extended this far (×scale)
export const MIN_THUMB_EXTENSION_RATIO = 0.22; // thumb must be extended this far (×scale)
export const CONFIRM_FRAMES = 3; // consecutive candidate frames before a fresh pinch latches on
export const SMOOTHING = 0.34; // EMA factor for point/depth once active (0=frozen, 1=raw)

export interface PinchTracker {
  recognize(hands: HandLandmark[][]): Pinch[];
}

// Per-hand carry-over state (kept private to the tracker closure).
interface HandState {
  active: boolean;
  heldFrames: number;
  point: { x: number; y: number };
  depth: number;
}

function distance2d(a: HandLandmark, b: HandLandmark): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

function midpoint(a: HandLandmark, b: HandLandmark): { x: number; y: number } {
  return {
    x: Number(((a.x + b.x) / 2).toFixed(4)),
    y: Number(((a.y + b.y) / 2).toFixed(4)),
  };
}

function averageDepth(a: HandLandmark, b: HandLandmark): number {
  return ((a.z ?? 0) + (b.z ?? 0)) / 2;
}

function smooth(previous: number, next: number): number {
  return Number((previous + (next - previous) * SMOOTHING).toFixed(4));
}

// Estimate a per-hand size to normalize distances against, so the same gesture
// reads identically whether the hand is near or far from the camera. Ported
// from orbit-snap: the larger of (index-MCP↔pinky-MCP knuckle span) and
// (wrist↔middle-MCP palm length); null if neither is measurable.
function estimateHandScale(hand: HandLandmark[]): number | null {
  const indexMcp = hand[LANDMARK_INDEX.INDEX_MCP];
  const pinkyMcp = hand[LANDMARK_INDEX.PINKY_MCP];
  const wrist = hand[LANDMARK_INDEX.WRIST];
  const middleMcp = hand[LANDMARK_INDEX.MIDDLE_MCP];

  const spans = [
    indexMcp && pinkyMcp ? distance2d(indexMcp, pinkyMcp) : 0,
    wrist && middleMcp ? distance2d(wrist, middleMcp) : 0,
  ].filter((span) => span > 0.001);

  return spans.length ? Math.max(...spans) : null;
}

// Gate out non-pinch poses (e.g. a closed fist) by requiring the thumb and
// index to be sufficiently EXTENDED from their knuckles. Only enforced when a
// scale is known; otherwise we trust the tip-distance gate alone (orbit-snap
// returns true here when scale is null).
function hasPinchFingerPose(hand: HandLandmark[], scale: number | null): boolean {
  if (scale === null) return true;

  const indexTip = hand[LANDMARK_INDEX.INDEX_TIP];
  const indexMcp = hand[LANDMARK_INDEX.INDEX_MCP];
  const thumbTip = hand[LANDMARK_INDEX.THUMB_TIP];
  const thumbMcp = hand[LANDMARK_INDEX.THUMB_MCP];

  const indexExtension = indexMcp
    ? distance2d(indexTip, indexMcp) / scale
    : Number.POSITIVE_INFINITY;
  const thumbExtension = thumbMcp
    ? distance2d(thumbTip, thumbMcp) / scale
    : Number.POSITIVE_INFINITY;

  return (
    indexExtension >= MIN_INDEX_EXTENSION_RATIO &&
    thumbExtension >= MIN_THUMB_EXTENSION_RATIO
  );
}

// Are the thumb & index tips close enough for the given phase? Uses the ratio
// gate when scale is known, else the absolute-distance fallback. `activate` is
// the tight bound (to engage); `release` is the loose bound (to stay engaged).
function isWithinPinchDistance(
  hand: HandLandmark[],
  scale: number | null,
  phase: 'activate' | 'release',
): boolean {
  const thumbTip = hand[LANDMARK_INDEX.THUMB_TIP];
  const indexTip = hand[LANDMARK_INDEX.INDEX_TIP];
  const tipDistance = distance2d(thumbTip, indexTip);

  if (scale === null) {
    return tipDistance <= (phase === 'activate' ? ACTIVATE_DISTANCE : RELEASE_DISTANCE);
  }

  const ratio = tipDistance / scale;
  return ratio <= (phase === 'activate' ? ACTIVATE_RATIO : RELEASE_RATIO);
}

export function createPinchTracker(): PinchTracker {
  // Closure-held prev-frame state, keyed by hand id (the hand's array index).
  let prev: Record<number, HandState> = {};

  return {
    recognize(hands: HandLandmark[][]): Pinch[] {
      const next: Record<number, HandState> = {};
      const pinches: Pinch[] = [];

      hands.forEach((hand, id) => {
        // A malformed hand (missing the two tips we always need) can't pinch.
        const thumbTip = hand?.[LANDMARK_INDEX.THUMB_TIP];
        const indexTip = hand?.[LANDMARK_INDEX.INDEX_TIP];
        if (!thumbTip || !indexTip) return;

        const previous = prev[id];
        const rawPoint = midpoint(thumbTip, indexTip);
        const rawDepth = averageDepth(thumbTip, indexTip);
        const scale = estimateHandScale(hand);

        const hasPose = hasPinchFingerPose(hand, scale);
        const withinActivate = isWithinPinchDistance(hand, scale, 'activate');
        const withinRelease = isWithinPinchDistance(hand, scale, 'release');

        // Hysteresis: if already active, use the loose release bound; if not,
        // use the tight activate bound.
        const candidate = hasPose && (previous?.active ? withinRelease : withinActivate);
        const heldFrames = candidate ? (previous?.heldFrames ?? 0) + 1 : 0;
        // Stay active while candidate holds; a fresh pinch must persist
        // CONFIRM_FRAMES consecutive frames before it latches on (debounce).
        const active = previous?.active ? candidate : heldFrames >= CONFIRM_FRAMES;

        if (!candidate && !active) return;

        // Once active, EMA-smooth the point/depth; otherwise pass raw.
        const point =
          previous && active
            ? {
                x: smooth(previous.point.x, rawPoint.x),
                y: smooth(previous.point.y, rawPoint.y),
              }
            : rawPoint;
        const depth = previous && active ? smooth(previous.depth, rawDepth) : rawDepth;

        next[id] = { active, heldFrames, point, depth };

        if (active) {
          pinches.push({ id, point, depth });
        }
      });

      prev = next;
      return pinches;
    },
  };
}
