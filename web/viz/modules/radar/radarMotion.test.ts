import { describe, expect, it } from 'vitest';
import { globeSpinRate, SPIN_BASE_ROOT, SPIN_BASE_SUB } from './radarMotion';
import { dampValue } from '@/viz/shared/scene/useOrbCamera';

describe('globeSpinRate', () => {
  it('turns a resting globe at its base rate, roots slower than subagents', () => {
    expect(globeSpinRate(true, 0)).toBeCloseTo(SPIN_BASE_ROOT, 10);
    expect(globeSpinRate(false, 0)).toBeCloseTo(SPIN_BASE_SUB, 10);
    expect(globeSpinRate(true, 0)).toBeLessThan(globeSpinRate(false, 0));
  });

  it('spins a working globe faster than an idle one', () => {
    expect(globeSpinRate(true, 1)).toBeGreaterThan(globeSpinRate(true, 0));
    expect(globeSpinRate(false, 1)).toBeGreaterThan(globeSpinRate(false, 0));
  });

  it('keeps the lift subtle: a readout, not a toy', () => {
    // Under 2x. A working board should read as busier, not as a spinning novelty.
    expect(globeSpinRate(true, 1) / globeSpinRate(true, 0)).toBeLessThan(2);
    expect(globeSpinRate(true, 1) / globeSpinRate(true, 0)).toBeGreaterThan(1.4);
  });

  it('is monotonic in liveness, so the rate only ever climbs as a globe wakes', () => {
    let last = 0;
    for (const k of [0, 0.2, 0.4, 0.6, 0.8, 1]) {
      const rate = globeSpinRate(true, k);
      expect(rate).toBeGreaterThan(last);
      last = rate;
    }
  });

  it('EASES between the two rates on a status flip instead of snapping', () => {
    // The renderer damps its liveness factor, so feeding that in is what makes the
    // rate glide. Walk a real damped ramp and assert every step is a small change.
    const dt = 1 / 60;
    let liveK = 0;
    let prev = globeSpinRate(true, liveK);
    const span = globeSpinRate(true, 1) - globeSpinRate(true, 0);
    for (let i = 0; i < 180; i++) {
      liveK = dampValue(liveK, 1, 3.5, dt); // same lambda RadarGlobe uses for `live`
      const rate = globeSpinRate(true, liveK);
      expect(rate - prev).toBeGreaterThanOrEqual(0);
      expect(rate - prev).toBeLessThan(span * 0.1); // no single frame jumps the gap
      prev = rate;
    }
    expect(prev).toBeCloseTo(globeSpinRate(true, 1), 4);
  });

  it('reduced motion pins every globe to its resting rate', () => {
    expect(globeSpinRate(true, 1, true)).toBeCloseTo(SPIN_BASE_ROOT, 10);
    expect(globeSpinRate(false, 1, true)).toBeCloseTo(SPIN_BASE_SUB, 10);
    expect(globeSpinRate(false, 0.5, true)).toBe(globeSpinRate(false, 0, true));
  });

  it('clamps a corrupt liveness factor instead of flinging the globe', () => {
    for (const bad of [Number.NaN, -5, 9, Number.POSITIVE_INFINITY]) {
      const rate = globeSpinRate(true, bad);
      expect(Number.isFinite(rate)).toBe(true);
      expect(rate).toBeGreaterThanOrEqual(SPIN_BASE_ROOT);
      expect(rate).toBeLessThanOrEqual(globeSpinRate(true, 1));
    }
  });
});
