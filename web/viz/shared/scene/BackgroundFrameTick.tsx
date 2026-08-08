// Drives the render loop while the Canvas is in 'demand' mode.
//
// In 'demand' R3F renders exactly once per invalidate() and never on its own, so a
// blurred window would freeze without this. Pacing invalidate() is what turns
// "blurred" into a cheap low-rate mode instead of a stopped one: the constellation
// keeps turning and liveness keeps reading, at a fraction of the frames.
//
// The pacer rides requestAnimationFrame rather than setInterval, for two reasons that
// both showed up as real defects:
//   • SPACING. setInterval fires on its own clock, which beats against the display's.
//     A 33ms interval against a 16.7ms refresh lands 33/33/50/33 and the drift reads
//     as judder, so the scene looked worse than its nominal rate. rAF can only fire
//     ON a refresh, so every frame is spaced a whole number of refreshes apart.
//   • COST. setInterval keeps firing when the window is fully occluded or off-screen,
//     paying for renders literally nobody can see. The browser stops delivering rAF in
//     exactly those cases, so the pacer now costs nothing there without a single extra
//     visibility check.
//
// Mounted inside <Canvas> so it can reach the R3F store. Renders nothing.
import { useEffect } from 'react';
import { useThree } from '@react-three/fiber';
import { backgroundTickDue, BACKGROUND_FPS } from './frameloop';

export function BackgroundFrameTick({
  paced,
  fps = BACKGROUND_FPS,
}: {
  // True only in 'demand' mode. In 'always' the loop is already running and an extra
  // invalidate() would be a no-op, but gating keeps the pacer off entirely.
  paced: boolean;
  fps?: number;
}) {
  const invalidate = useThree((s) => s.invalidate);

  useEffect(() => {
    if (!paced) return;
    // One immediate frame so the switch into background mode is not a visible stall.
    invalidate();

    let raf = 0;
    let last = Number.NaN; // NaN => the first rAF tick is always due
    const step = (now: number) => {
      raf = window.requestAnimationFrame(step);
      if (!backgroundTickDue(now - last, fps)) return;
      last = now;
      invalidate();
    };
    raf = window.requestAnimationFrame(step);
    return () => window.cancelAnimationFrame(raf);
  }, [paced, fps, invalidate]);

  return null;
}
