import { describe, it, expect } from 'vitest';
import {
  ALERT_FLOOR,
  ALERT_PERIOD,
  ALERT_STEADY,
  alertBlink,
  alertGlowMultiplier,
  alertWhiteMix,
} from './radarAlert';

/** Sample one full cycle finely enough to characterise the waveform. */
function cycle(step = 0.005): Array<{ t: number; v: number }> {
  const out: Array<{ t: number; v: number }> = [];
  for (let t = 0; t < ALERT_PERIOD; t += step) out.push({ t, v: alertBlink(t) });
  return out;
}

describe('alertBlink', () => {
  it('stays inside [floor, 1] and never goes dark', () => {
    for (const { v } of cycle()) {
      expect(v).toBeGreaterThanOrEqual(ALERT_FLOOR - 1e-9);
      expect(v).toBeLessThanOrEqual(1 + 1e-9);
    }
  });

  it('fires exactly two strobes per cycle, then rests', () => {
    const lit = cycle().map(({ v }) => v > ALERT_FLOOR + 0.25);
    let bursts = 0;
    for (let i = 0; i < lit.length; i++) {
      if (lit[i] && !lit[i - 1]) bursts++;
    }
    expect(bursts).toBe(2);
    // The rest is the majority of the cycle: that gap is what reads as a beacon rather
    // than a fast breath.
    const litFraction = lit.filter(Boolean).length / lit.length;
    expect(litFraction).toBeLessThan(0.35);
  });

  it('reaches full brightness at a strobe peak', () => {
    const peak = Math.max(...cycle(0.001).map(({ v }) => v));
    expect(peak).toBeGreaterThan(0.97);
  });

  it('is periodic, so a long-running scene never drifts out of cadence', () => {
    for (const t of [0.05, 0.3, 0.9, 1.4]) {
      expect(alertBlink(t + ALERT_PERIOD * 7)).toBeCloseTo(alertBlink(t), 6);
    }
  });

  it('is continuous: no step between adjacent frames at 30fps', () => {
    const dt = 1 / 30;
    let maxJump = 0;
    for (let t = 0; t < ALERT_PERIOD * 2; t += dt) {
      maxJump = Math.max(maxJump, Math.abs(alertBlink(t + dt) - alertBlink(t)));
    }
    // A hard on/off would jump the full range in one frame; a sharpened sine cannot.
    expect(maxJump).toBeLessThan(0.9);
  });

  it('holds a steady high burn under prefers-reduced-motion', () => {
    const samples = [0, 0.1, 0.4, 1.2, 9.7].map((t) => alertBlink(t, true));
    expect(new Set(samples).size).toBe(1);
    expect(samples[0]).toBe(ALERT_STEADY);
  });

  it('degrades to the floor on a non-finite clock rather than throwing', () => {
    expect(alertBlink(Number.NaN)).toBe(ALERT_FLOOR);
    expect(alertBlink(Number.POSITIVE_INFINITY)).toBe(ALERT_FLOOR);
  });
});

describe('alert brightness mapping', () => {
  it('sweeps from below a resting globe to above a working one', () => {
    expect(alertGlowMultiplier(0)).toBeLessThan(1);
    expect(alertGlowMultiplier(1)).toBeGreaterThan(2.5);
  });

  it('keeps the red red at the crest', () => {
    // Working globes lerp up to 0.5 toward white; the alert must stay well under that or
    // the peak of each flash reads as a white-hot working core.
    expect(alertWhiteMix(1)).toBeLessThan(0.3);
    expect(alertWhiteMix(0)).toBe(0);
  });

  it('clamps a bad input instead of propagating it', () => {
    expect(alertGlowMultiplier(Number.NaN)).toBe(alertGlowMultiplier(0));
    expect(alertWhiteMix(5)).toBe(alertWhiteMix(1));
  });
});
