// Drives the render loop while the Canvas is in 'demand' mode.
//
// In 'demand' R3F renders exactly once per invalidate() and never on its own, so a
// blurred window would freeze without this. Pacing invalidate() on an interval is what
// turns "blurred" into a cheap low-rate mode instead of a stopped one: the constellation
// keeps turning and liveness keeps reading, at roughly an eighth of the frames.
//
// Mounted inside <Canvas> so it can reach the R3F store. Renders nothing.
import { useEffect } from 'react';
import { useThree } from '@react-three/fiber';
import { BACKGROUND_FPS } from './frameloop';

export function BackgroundFrameTick({
  paced,
  fps = BACKGROUND_FPS,
}: {
  // True only in 'demand' mode. In 'always' the loop is already running and an extra
  // invalidate() would be a no-op, but gating keeps the timer off entirely.
  paced: boolean;
  fps?: number;
}) {
  const invalidate = useThree((s) => s.invalidate);

  useEffect(() => {
    if (!paced) return;
    // One immediate frame so the switch into background mode is not a visible stall.
    invalidate();
    const id = window.setInterval(invalidate, Math.round(1000 / fps));
    return () => window.clearInterval(id);
  }, [paced, fps, invalidate]);

  return null;
}
