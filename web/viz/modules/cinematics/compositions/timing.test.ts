import { describe, it, expect } from 'vitest';
import { FPS, INTRO_FRAMES, revealDuration } from './timing';

// The timing module is PURE (zero React/Remotion import) so it runs in the
// node vitest env without pulling the player bundle. The compositions consume
// `revealDuration` for their `durationInFrames`, so locking the math here locks
// the reveal length the user actually sees.
describe('revealDuration', () => {
  it('is n*FPS + intro frames (the locked formula)', () => {
    for (const n of [0, 1, 4, 7, 24]) {
      expect(revealDuration(n)).toBe(n * FPS + INTRO_FRAMES);
    }
  });

  it('grows by exactly one second per additional finding', () => {
    expect(revealDuration(5) - revealDuration(4)).toBe(FPS);
  });

  it('never returns a non-positive or fractional duration', () => {
    for (const n of [0, 1, 3, 12]) {
      const d = revealDuration(n);
      expect(d).toBeGreaterThan(0);
      expect(Number.isInteger(d)).toBe(true);
    }
  });

  it('clamps absurd / negative counts so a malformed payload cannot break playback', () => {
    expect(revealDuration(-3)).toBe(INTRO_FRAMES);
    expect(Number.isInteger(revealDuration(9999))).toBe(true);
  });

  it('uses the canonical 30fps timebase', () => {
    expect(FPS).toBe(30);
  });
});
