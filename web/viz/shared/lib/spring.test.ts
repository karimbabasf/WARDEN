import { describe, expect, it } from 'vitest';
import { spring, springSettled, springSnap, springStep } from './spring';

const FRAME = 1 / 120;

function settle(from: number, target: number, frames = 400, response = 0.32, damping = 1) {
  let s = spring(from);
  const peak = { above: from };
  for (let i = 0; i < frames; i++) {
    s = springStep(s, target, FRAME, response, damping);
    if (s.value > peak.above) peak.above = s.value;
  }
  return { s, peak: peak.above };
}

describe('springStep', () => {
  it('arrives at the target', () => {
    const { s } = settle(0, 100);
    expect(s.value).toBeCloseTo(100, 1);
    expect(Math.abs(s.velocity)).toBeLessThan(1);
  });

  it('does not overshoot at damping 1', () => {
    const { peak } = settle(0, 100);
    expect(peak).toBeLessThanOrEqual(100.001);
  });

  it('overshoots below damping 1', () => {
    const { peak } = settle(0, 100, 400, 0.32, 0.55);
    expect(peak).toBeGreaterThan(100);
  });

  it('reaches most of the way inside its response time', () => {
    // response 0.32s: after 0.32s the value should be clearly past halfway.
    let s = spring(0);
    for (let i = 0; i < Math.round(0.32 / FRAME); i++) s = springStep(s, 100, FRAME);
    expect(s.value).toBeGreaterThan(60);
  });

  it('carries velocity through a re-target instead of restarting', () => {
    let s = spring(0);
    for (let i = 0; i < 12; i++) s = springStep(s, 400, FRAME);
    const moving = s.velocity;
    expect(moving).toBeGreaterThan(0);
    // Re-target backwards mid-flight: the spring must still be carrying the old
    // velocity on the very next step, not snap to a fresh standstill.
    const next = springStep(s, 0, FRAME);
    expect(next.velocity).toBeLessThan(moving);
    expect(next.value).toBeGreaterThan(0);
  });

  it('stays finite when a frame arrives absurdly late', () => {
    let s = spring(0);
    for (let i = 0; i < 40; i++) s = springStep(s, 100, 1.5);
    expect(Number.isFinite(s.value)).toBe(true);
    expect(s.value).toBeCloseTo(100, 1);
  });

  it('is a no-op for a zero or negative dt', () => {
    const s = spring(3, 9);
    expect(springStep(s, 100, 0)).toBe(s);
    expect(springStep(s, 100, -1)).toBe(s);
  });
});

describe('springSettled / springSnap', () => {
  it('reports settled only once still AND arrived', () => {
    expect(springSettled(spring(100, 0), 100)).toBe(true);
    expect(springSettled(spring(100, 40), 100)).toBe(false);
    expect(springSettled(spring(60, 0), 100)).toBe(false);
  });

  it('snaps with no velocity', () => {
    expect(springSnap(42)).toEqual({ value: 42, velocity: 0 });
  });
});
