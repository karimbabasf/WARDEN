// heatmap.ts — pure helpers for the activity heatmap.
//
// Kept separate from the React component so the bucketing logic is unit-testable
// in the node test env (no jsdom needed). The only job here is turning a day's
// raw token total into one of N discrete intensity levels, plus the green-token
// ramp that level maps to. Honest-viz: level 0 is genuinely "no activity", and a
// busier day is never shown dimmer than a quieter one (monotonic).

/** Number of fill levels: 0 (empty) .. 4 (hottest). GitHub-style 5-step ramp. */
export const HEATMAP_LEVELS = 5;

/**
 * Bucket a day's `total_tokens` into an intensity level 0..HEATMAP_LEVELS-1.
 *
 * Level 0 is reserved strictly for zero/negative/invalid totals (a true blank
 * day). Positive days are split into levels 1..4 by their rank against the
 * busiest day in the window (`maxTokens`), so the ramp self-scales to whatever
 * volume the operator actually runs — a light week and a heavy week both use the
 * full ramp instead of all washing out to level 1.
 *
 * Pure + total: any non-finite input collapses to 0; equal totals get equal
 * levels (monotonic non-decreasing in `tokens`).
 */
export function heatmapLevel(tokens: number, maxTokens: number): number {
  if (!Number.isFinite(tokens) || tokens <= 0) return 0;
  // A degenerate window (one active day, or max not yet known) still shows the
  // active day at full intensity rather than vanishing.
  if (!Number.isFinite(maxTokens) || maxTokens <= 0) return HEATMAP_LEVELS - 1;

  // Log scale: real-corpus token totals are heavy-tailed (p50 ~100k, max ~50M),
  // so a linear ramp against the busiest day washes every normal day out to
  // level 1 and lets one outlier day dominate. Log keeps the ramp meaningful and
  // clamps outlier days to the top instead of flattening everything else.
  // (Calibration: docs/superpowers/research/dossier-rubric-calibration.md, Finding 5.)
  const span = HEATMAP_LEVELS - 1; // 4
  const ratio = Math.min(1, Math.log10(tokens + 1) / Math.log10(maxTokens + 1));
  const level = Math.ceil(ratio * span);
  return Math.min(span, Math.max(1, level));
}

/**
 * The CSS fill colour for an intensity level, drawn from the phosphor-green
 * tokens. Level 0 is a near-empty hairline cell; higher levels approach the
 * bright `--green`. Returned as a ready-to-use CSS colour string.
 */
export function heatmapFill(level: number): string {
  switch (level) {
    case 0:
      return 'rgba(118, 255, 157, 0.05)'; // empty — faint --green wash
    case 1:
      return 'rgba(27, 111, 58, 0.55)'; // --dim, low
    case 2:
      return 'rgba(27, 111, 58, 0.95)'; // --dim, full
    case 3:
      return 'rgba(118, 255, 157, 0.70)'; // --green, mid
    case 4:
    default:
      return 'rgba(118, 255, 157, 1)'; // --green, hot
  }
}

/** Largest `total_tokens` across the cells (0 when none). For ramp scaling. */
export function maxTokens(cells: { total_tokens: number }[]): number {
  let m = 0;
  for (const c of cells) {
    const t = c.total_tokens;
    if (Number.isFinite(t) && t > m) m = t;
  }
  return m;
}
