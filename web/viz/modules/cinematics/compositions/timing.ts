// timing.ts — the PURE timing math for the Remotion compositions.
//
// Kept deliberately free of any React / Remotion / DOM import so it can be unit-
// tested in the node vitest env (and imported by `main.ts` to size the <Player>)
// WITHOUT pulling the lazy Remotion chunk onto the summon hot path. The
// compositions import `revealDuration` for their `durationInFrames`, so this is
// the single source of truth for how long the reveal actually plays.

/** Canonical timebase for every WARDEN composition (intro · reveal · recap). */
export const FPS = 30;

/**
 * Frames the branded intro / scaffold occupies before the holes start
 * cascading. ~1.6s of slam-in runway (title flash + scanline sweep) so the
 * reveal has weight before the first hole lands. Also the floor duration when
 * there are zero findings (an empty-but-honest "no holes" beat).
 */
export const INTRO_FRAMES = Math.round(FPS * 1.6); // 48 frames @30fps

/** Per-finding screen time: each ranked hole owns exactly one second. */
const FRAMES_PER_FINDING = FPS;

/** Hard ceiling so a malformed finding count can never produce a runaway clip. */
const MAX_FINDINGS = 64;

/**
 * Total reveal length in frames for `n` findings.
 *   revealDuration(n) === n * FPS + INTRO_FRAMES
 * `n` is clamped to [0, MAX_FINDINGS] and floored to an integer so a garbage
 * payload (negative / fractional / absurd) can never break <Player> playback.
 */
export function revealDuration(n: number): number {
  const safe = Number.isFinite(n) ? Math.max(0, Math.min(MAX_FINDINGS, Math.floor(n))) : 0;
  return safe * FRAMES_PER_FINDING + INTRO_FRAMES;
}
