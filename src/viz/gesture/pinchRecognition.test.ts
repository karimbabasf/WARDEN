import { describe, it, expect } from 'vitest';
import {
  createPinchTracker,
  LANDMARK_INDEX,
  ACTIVATE_DISTANCE,
  RELEASE_DISTANCE,
  ACTIVATE_RATIO,
  RELEASE_RATIO,
  MIN_INDEX_EXTENSION_RATIO,
  MIN_THUMB_EXTENSION_RATIO,
  CONFIRM_FRAMES,
  SMOOTHING,
} from './pinchRecognition';
import type { HandLandmark } from './types';

// Build a synthetic 21-landmark MediaPipe hand. The MCPs/wrist give a stable
// hand scale (~0.26); the thumb & index tips are placed symmetrically around a
// center, `tipGap` apart, both clearly EXTENDED from their knuckles so the pose
// gate passes. Geometry was solved against the real thresholds:
//   tipGap 0.02 -> tip/scale ratio ~0.08  (< ACTIVATE_RATIO 0.5)  => pinch
//   tipGap 0.145 -> ratio ~0.56  (> activate, < RELEASE_RATIO 0.78) => hysteresis band
//   tipGap 0.30 -> ratio ~1.15  (> release)                         => no pinch / release
function makeHand(tipGap: number, cx = 0.5, cy = 0.4): HandLandmark[] {
  const h: HandLandmark[] = Array.from({ length: 21 }, () => ({ x: 0.5, y: 0.5, z: 0 }));
  h[LANDMARK_INDEX.WRIST] = { x: 0.5, y: 0.9, z: 0 };
  h[LANDMARK_INDEX.THUMB_MCP] = { x: 0.42, y: 0.78, z: 0 };
  h[LANDMARK_INDEX.INDEX_MCP] = { x: 0.46, y: 0.66, z: 0 };
  h[LANDMARK_INDEX.MIDDLE_MCP] = { x: 0.52, y: 0.64, z: 0 };
  h[LANDMARK_INDEX.PINKY_MCP] = { x: 0.62, y: 0.7, z: 0 };
  h[LANDMARK_INDEX.INDEX_TIP] = { x: cx - tipGap / 2, y: cy, z: -0.02 };
  h[LANDMARK_INDEX.THUMB_TIP] = { x: cx + tipGap / 2, y: cy, z: -0.02 };
  return h;
}

const PINCH_GAP = 0.02; // tips together -> within activate
const MID_GAP = 0.145; // tips mid -> within release only (hysteresis)
const OPEN_GAP = 0.3; // tips apart -> outside both

describe('ported threshold constants (must equal orbit-snap exactly)', () => {
  it('locks the orbit-snap DEFAULT_OPTIONS values', () => {
    expect(ACTIVATE_DISTANCE).toBe(0.066);
    expect(RELEASE_DISTANCE).toBe(0.104);
    expect(ACTIVATE_RATIO).toBe(0.5);
    expect(RELEASE_RATIO).toBe(0.78);
    expect(MIN_INDEX_EXTENSION_RATIO).toBe(0.24);
    expect(MIN_THUMB_EXTENSION_RATIO).toBe(0.22);
    expect(CONFIRM_FRAMES).toBe(3);
    expect(SMOOTHING).toBe(0.34);
  });

  it('keeps release looser than activate (the hysteresis band exists)', () => {
    expect(RELEASE_DISTANCE).toBeGreaterThan(ACTIVATE_DISTANCE);
    expect(RELEASE_RATIO).toBeGreaterThan(ACTIVATE_RATIO);
  });

  it('exposes the confirmed MediaPipe landmark indices', () => {
    expect(LANDMARK_INDEX).toMatchObject({
      WRIST: 0,
      THUMB_MCP: 2,
      THUMB_TIP: 4,
      INDEX_MCP: 5,
      INDEX_TIP: 8,
      MIDDLE_MCP: 9,
      PINKY_MCP: 17,
    });
  });
});

describe('createPinchTracker.recognize', () => {
  it('reports exactly one Pinch for a thumb-tip-near-index pinch with extended fingers', () => {
    const t = createPinchTracker();
    const hand = makeHand(PINCH_GAP);
    // Must hold CONFIRM_FRAMES consecutive frames before it latches.
    let pinches = t.recognize([hand]);
    for (let i = 1; i < CONFIRM_FRAMES; i++) pinches = t.recognize([hand]);
    expect(pinches).toHaveLength(1);
    expect(pinches[0].id).toBe(0);
    // smoothed midpoint of the two tips ~ center x (0.5)
    expect(pinches[0].point.x).toBeCloseTo(0.5, 2);
  });

  it('does not confirm a pinch before CONFIRM_FRAMES have elapsed', () => {
    const t = createPinchTracker();
    const hand = makeHand(PINCH_GAP);
    // Only CONFIRM_FRAMES-1 frames -> not yet active.
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES - 1; i++) pinches = t.recognize([hand]);
    expect(pinches).toHaveLength(0);
  });

  it('reports zero Pinches when the tips are far apart', () => {
    const t = createPinchTracker();
    const hand = makeHand(OPEN_GAP);
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES + 2; i++) pinches = t.recognize([hand]);
    expect(pinches).toHaveLength(0);
  });

  it('hysteresis: once active, a mid-distance frame STAYS active (between activate and release)', () => {
    const t = createPinchTracker();
    const pinchHand = makeHand(PINCH_GAP);
    const midHand = makeHand(MID_GAP);
    // Latch the pinch first.
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES; i++) pinches = t.recognize([pinchHand]);
    expect(pinches).toHaveLength(1);
    // Now feed a mid-distance frame: tips are past the ACTIVATE bound but still
    // within the looser RELEASE bound -> hysteresis keeps the pinch alive.
    pinches = t.recognize([midHand]);
    expect(pinches).toHaveLength(1);
  });

  it('hysteresis: a mid-distance frame from rest does NOT start a pinch', () => {
    const t = createPinchTracker();
    const midHand = makeHand(MID_GAP);
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES + 2; i++) pinches = t.recognize([midHand]);
    // Within release but never within activate -> never engages from rest.
    expect(pinches).toHaveLength(0);
  });

  it('releases once the tips separate past the release bound', () => {
    const t = createPinchTracker();
    const pinchHand = makeHand(PINCH_GAP);
    const openHand = makeHand(OPEN_GAP);
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES; i++) pinches = t.recognize([pinchHand]);
    expect(pinches).toHaveLength(1);
    pinches = t.recognize([openHand]);
    expect(pinches).toHaveLength(0);
  });

  it('tracks two hands independently (distinct ids)', () => {
    const t = createPinchTracker();
    const left = makeHand(PINCH_GAP, 0.3);
    const right = makeHand(PINCH_GAP, 0.7);
    let pinches: ReturnType<typeof t.recognize> = [];
    for (let i = 0; i < CONFIRM_FRAMES; i++) pinches = t.recognize([left, right]);
    expect(pinches.map((p) => p.id).sort()).toEqual([0, 1]);
  });

  it('returns an empty array for an empty frame and survives a malformed hand', () => {
    const t = createPinchTracker();
    expect(t.recognize([])).toEqual([]);
    // A hand missing the fingertips we need must not throw.
    expect(() => t.recognize([[]])).not.toThrow();
    expect(t.recognize([[]])).toEqual([]);
  });

  it('smooths the reported point across frames once active (EMA, not raw jump)', () => {
    const t = createPinchTracker();
    const a = makeHand(PINCH_GAP, 0.4);
    for (let i = 0; i < CONFIRM_FRAMES; i++) t.recognize([a]);
    // Jump the center hard; the smoothed point should land between old and new,
    // not snap to the raw new midpoint (0.6).
    const b = makeHand(PINCH_GAP, 0.6);
    const pinches = t.recognize([b]);
    expect(pinches).toHaveLength(1);
    expect(pinches[0].point.x).toBeGreaterThan(0.4);
    expect(pinches[0].point.x).toBeLessThan(0.6);
  });
});
