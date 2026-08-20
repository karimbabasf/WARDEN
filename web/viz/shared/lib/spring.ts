// spring.ts: one damped spring, integrated implicitly so it cannot explode.
//
// WARDEN animates with easing tokens everywhere EXCEPT the menu-bar HUD, and the
// difference is interruption. A tray icon can be clicked twice in a third of a
// second, and the fleet behind the panel can change size mid-open; a CSS transition
// re-targeted at that moment restarts from a stale value and the panel visibly jumps.
// A spring re-targets from wherever it actually is, carrying its velocity through, so
// the second click reverses the first instead of fighting it. That is the whole reason
// this exists, and it is why the HUD does not just use `--ease-out`.
//
// Parameters are Apple's pair, not the physics triplet: RESPONSE (how long the value
// takes to arrive, in seconds) and DAMPING RATIO (1 = no overshoot, < 1 bounces), which
// are the two a designer can actually reason about.
//
// Integration is implicit Euler, solved directly rather than stepped: it is
// unconditionally stable, so a frame that arrives 300ms late (a stalled tab, a
// backgrounded window) settles toward the target instead of oscillating apart.
//
// Pure module: no React, no DOM. Unit-tested in spring.test.ts.

export type Spring = {
  value: number;
  velocity: number;
};

export function spring(value: number, velocity = 0): Spring {
  return { value, velocity };
}

/** Apple's default: arrives in ~0.32s, never overshoots. */
export const HUD_RESPONSE = 0.32;
export const HUD_DAMPING = 1;

/**
 * Advance `s` toward `target` by `dt` seconds. Returns a NEW spring; the caller owns
 * the state so a component can keep several independent springs (a 2D move is two
 * springs, never one on the diagonal distance, that desyncs the moment x and y have
 * different velocities).
 */
export function springStep(
  s: Spring,
  target: number,
  dt: number,
  response: number = HUD_RESPONSE,
  damping: number = HUD_DAMPING,
): Spring {
  if (!(dt > 0)) return s;
  // A tab that was backgrounded hands back one enormous dt. Clamping it keeps the
  // settle visually sane; stability itself does not need the clamp.
  const h = Math.min(dt, 0.064);
  const w = (2 * Math.PI) / Math.max(0.016, response);
  const z = Math.max(0, damping);
  // v1 = [v0 - h·w²(x0 - target)] / (1 + 2zwh + h²w²)   (implicit Euler, solved)
  const denom = 1 + 2 * z * w * h + h * h * w * w;
  const velocity = (s.velocity - h * w * w * (s.value - target)) / denom;
  return { value: s.value + h * velocity, velocity };
}

/** Within a pixel and barely moving: close enough to stop the loop. */
export function springSettled(s: Spring, target: number, epsilon = 0.4): boolean {
  return Math.abs(s.value - target) < epsilon && Math.abs(s.velocity) < epsilon * 8;
}

/** Snap to the target with no motion: the `prefers-reduced-motion` path. */
export function springSnap(target: number): Spring {
  return { value: target, velocity: 0 };
}
