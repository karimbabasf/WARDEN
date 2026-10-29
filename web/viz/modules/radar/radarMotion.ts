// radarMotion.ts: pure motion math for the radar globes.
//
// The globe's idle turn is a constant, calm rotation. A WORKING agent turns
// measurably faster, because rotation is the one channel not already spoken for
// (colour is harness identity, size is context occupancy, brightness is liveness),
// so it can carry "this one is actually running" without muddying any of them.
//
// Two rules keep it a readout rather than a toy:
//   • the lift is modest (a working globe turns ~1.85x, not 5x) so a busy board
//     stays legible instead of whirring;
//   • it is driven by the CALLER'S ALREADY-EASED liveness factor, never by the raw
//     boolean, so a status flip glides between the two rates and never snaps.
//
// Pure (no Three.js, no React), so the rate curve is unit-tested directly.

/** Radians per second a resting globe turns. Roots are larger, so they turn slower.
 *
 *  Raised 2.5x on 2026-09-08. The old rate put a resting root at ~78 seconds a
 *  revolution, which is under the threshold where a person reads a thing as turning at
 *  all: at HUD and notch size the globes looked frozen, and the working lift below had
 *  nothing legible to lift. ~31s resting, ~17s working reads as alive without whirring,
 *  and the ratio between the two states is untouched, so the spin still carries the one
 *  thing it is for. */
export const SPIN_BASE_ROOT = 0.2;
export const SPIN_BASE_SUB = 0.34;

/** Extra fraction of the base rate a fully-working globe adds (0.85 = ~1.85x). */
const SPIN_WORKING_LIFT = 0.85;

/**
 * Angular speed (rad/s) for one globe.
 *
 * `liveK` is the eased liveness factor the renderer already damps toward 1 while
 * `status === 'working'` and toward 0 otherwise, so passing it here is what makes
 * the rate EASE between the two speeds instead of jumping on the status edge.
 *
 * `reduced` (prefers-reduced-motion) pins the globe to its resting rate: the spin
 * carries no information a reduced-motion user is owed, and a speed change is
 * exactly the kind of restless motion that setting asks us to drop.
 */
export function globeSpinRate(isRoot: boolean, liveK: number, reduced = false): number {
  const base = isRoot ? SPIN_BASE_ROOT : SPIN_BASE_SUB;
  if (reduced) return base;
  const k = Number.isFinite(liveK) ? Math.max(0, Math.min(1, liveK)) : 0;
  return base * (1 + k * SPIN_WORKING_LIFT);
}
