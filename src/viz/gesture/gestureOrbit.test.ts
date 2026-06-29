import { describe, it, expect } from 'vitest';
import {
  createOrbitGestureState,
  updateOrbitGesture,
  ORBIT_AZIMUTH_GAIN,
  ORBIT_ELEVATION_GAIN,
  ORBIT_THROW_GAIN,
  ELEVATION_THROW_GAIN,
  REGRAB_DISTANCE,
  MAX_DELTA_PER_FRAME,
  MAX_SPREAD_DELTA_PER_FRAME,
} from './gestureOrbit';
import type { Pinch } from './types';

const pinch = (x: number, y: number, id = 0): Pinch => ({ id, point: { x, y }, depth: 0 });

describe('ported gains (must equal orbit-snap exactly)', () => {
  it('locks the orbit-snap orbitCamera.ts gain values', () => {
    expect(ORBIT_AZIMUTH_GAIN).toBe(4.2);
    expect(ORBIT_ELEVATION_GAIN).toBe(3.1);
    expect(ORBIT_THROW_GAIN).toBe(12);
    expect(ELEVATION_THROW_GAIN).toBe(6);
    expect(REGRAB_DISTANCE).toBe(0.86);
    expect(MAX_DELTA_PER_FRAME).toBe(0.25);
  });
});

describe('updateOrbitGesture — single-pinch rotate', () => {
  it('returns all-zero / not-dragging for an empty pinch list', () => {
    const s = createOrbitGestureState();
    expect(updateOrbitGesture(s, [])).toEqual({
      azimuthDelta: 0,
      elevationDelta: 0,
      radiusDelta: 0,
      dragging: false,
    });
  });

  it('first frame of a pinch: zero deltas but dragging=true (seeds the anchor)', () => {
    const s = createOrbitGestureState();
    const out = updateOrbitGesture(s, [pinch(0.5, 0.5)]);
    expect(out).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true });
  });

  it('held pinch dragged +x over two frames => positive azimuthDelta, dragging=true', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5)]); // seed
    const out = updateOrbitGesture(s, [pinch(0.53, 0.5)]); // dx = +0.03
    expect(out.dragging).toBe(true);
    expect(out.azimuthDelta).toBeGreaterThan(0); // +x -> +azimuth (sign convention)
    expect(out.elevationDelta).toBe(0); // no y movement
    expect(out.radiusDelta).toBe(0); // single pinch never zooms
  });

  it('dragging -x yields a negative azimuthDelta (sign preserved)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5)]);
    const out = updateOrbitGesture(s, [pinch(0.47, 0.5)]); // dx = -0.03
    expect(out.azimuthDelta).toBeLessThan(0);
  });

  it('dragging UP on screen yields a positive elevationDelta (y is inverted)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5)]);
    // point.y decreases = moving up the image => dy = lastY - y > 0 => +elevation
    const out = updateOrbitGesture(s, [pinch(0.5, 0.45)]);
    expect(out.elevationDelta).toBeGreaterThan(0);
    expect(out.azimuthDelta).toBe(0);
  });

  it('a larger drag produces a strictly larger delta (pointer-throw is super-linear)', () => {
    const small = createOrbitGestureState();
    updateOrbitGesture(small, [pinch(0.5, 0.5)]);
    const smallOut = updateOrbitGesture(small, [pinch(0.52, 0.5)]); // dx = 0.02

    const large = createOrbitGestureState();
    updateOrbitGesture(large, [pinch(0.5, 0.5)]);
    const largeOut = updateOrbitGesture(large, [pinch(0.54, 0.5)]); // dx = 0.04

    expect(largeOut.azimuthDelta).toBeGreaterThan(smallOut.azimuthDelta);
    // super-linear: doubling dx MORE than doubles the response.
    expect(largeOut.azimuthDelta).toBeGreaterThan(2 * smallOut.azimuthDelta);
  });

  it('clamps a glitchy (but sub-regrab) jump to MAX_DELTA_PER_FRAME so the camera cannot fling', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.1, 0.5)]);
    // dx = 0.5: within REGRAB_DISTANCE (0.86) so it's treated as motion, but the
    // raw pointer-throw (~5.1 rad) must be capped at the per-frame ceiling.
    const out = updateOrbitGesture(s, [pinch(0.6, 0.5)]);
    expect(out.azimuthDelta).toBe(MAX_DELTA_PER_FRAME);
    expect(Math.abs(out.azimuthDelta)).toBeLessThanOrEqual(MAX_DELTA_PER_FRAME);
  });

  it('an implausible jump beyond REGRAB_DISTANCE re-anchors instead of moving (delta 0, still dragging)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.05, 0.5)]);
    const out = updateOrbitGesture(s, [pinch(0.95, 0.5)]); // dx = 0.9 > 0.86
    expect(out).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true });
  });

  it('release after a drag => all zeros and dragging=false, and resets the session', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5)]);
    updateOrbitGesture(s, [pinch(0.55, 0.5)]); // a real drag frame
    const released = updateOrbitGesture(s, []);
    expect(released).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false });
    // session reset: the next pinch is a fresh anchor (zero delta), not a jump
    // measured against the stale pre-release point.
    const reGrab = updateOrbitGesture(s, [pinch(0.8, 0.5)]);
    expect(reGrab).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true });
  });
});

describe('updateOrbitGesture — two-pinch zoom (dolly)', () => {
  it('exposes a per-frame spread clamp constant', () => {
    expect(MAX_SPREAD_DELTA_PER_FRAME).toBeGreaterThan(0);
  });

  it('first frame with two pinches: seeds the spread, no motion, dragging=false', () => {
    const s = createOrbitGestureState();
    const out = updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // spread 0.2
    expect(out).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false });
  });

  it('spreading the two pinches apart yields a positive radiusDelta (and never rotates)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // spread 0.2 (seed)
    const out = updateOrbitGesture(s, [pinch(0.35, 0.5, 0), pinch(0.65, 0.5, 1)]); // spread 0.3
    expect(out.radiusDelta).toBeGreaterThan(0);
    expect(out.azimuthDelta).toBe(0);
    expect(out.elevationDelta).toBe(0);
    expect(out.dragging).toBe(false);
  });

  it('pinching the two pinches together yields a negative radiusDelta', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.3, 0.5, 0), pinch(0.7, 0.5, 1)]); // spread 0.4 (seed)
    const out = updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // spread 0.2
    expect(out.radiusDelta).toBeLessThan(0);
  });

  it('two pinches translating together (constant spread) neither zooms NOR rotates', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.3, 0.5, 0), pinch(0.5, 0.5, 1)]); // spread 0.2 (seed)
    const out = updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // both +0.1x, spread still 0.2
    expect(out.radiusDelta).toBe(0);
    expect(out.azimuthDelta).toBe(0); // two-hand mode suppresses rotation entirely
    expect(out.elevationDelta).toBe(0);
  });

  it('clamps a glitchy (sub-regrab) spread jump to MAX_SPREAD_DELTA_PER_FRAME', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.45, 0.5, 0), pinch(0.55, 0.5, 1)]); // spread 0.1 (seed)
    const out = updateOrbitGesture(s, [pinch(0.3, 0.5, 0), pinch(0.7, 0.5, 1)]); // spread 0.4, dSpread 0.3
    expect(out.radiusDelta).toBe(MAX_SPREAD_DELTA_PER_FRAME);
  });

  it('an implausible spread jump beyond REGRAB_DISTANCE re-anchors (radiusDelta 0)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5, 0), pinch(0.5, 0.5, 1)]); // spread 0 (seed)
    const out = updateOrbitGesture(s, [pinch(0.05, 0.5, 0), pinch(0.95, 0.5, 1)]); // spread 0.9 > 0.86
    expect(out.radiusDelta).toBe(0);
  });

  it('dropping from two pinches back to one re-anchors rotation cleanly (no jump)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // zoom seed
    updateOrbitGesture(s, [pinch(0.35, 0.5, 0), pinch(0.65, 0.5, 1)]); // zoom frame
    const back = updateOrbitGesture(s, [pinch(0.35, 0.5, 0)]); // now one pinch
    expect(back).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: true });
  });

  it('switching from one pinch to two seeds zoom without a rotation spike', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.5, 0.5, 0)]); // rotate seed
    updateOrbitGesture(s, [pinch(0.55, 0.5, 0)]); // rotate frame
    const zoom = updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // two pinches now
    expect(zoom).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false });
  });

  it('releasing from zoom resets the spread session (next two-pinch is a fresh seed)', () => {
    const s = createOrbitGestureState();
    updateOrbitGesture(s, [pinch(0.4, 0.5, 0), pinch(0.6, 0.5, 1)]); // spread 0.2
    updateOrbitGesture(s, [pinch(0.3, 0.5, 0), pinch(0.7, 0.5, 1)]); // spread 0.4
    updateOrbitGesture(s, []); // release
    const re = updateOrbitGesture(s, [pinch(0.45, 0.5, 0), pinch(0.55, 0.5, 1)]); // spread 0.1 fresh
    expect(re).toEqual({ azimuthDelta: 0, elevationDelta: 0, radiusDelta: 0, dragging: false });
  });
});
