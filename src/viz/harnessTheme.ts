// Web mirror of the Rust `harness_theme` (src-tauri/src/ingest/mod.rs). This is
// the SINGLE source of harness identity on the web side — colours, glyphs and
// labels live here and nowhere else so the legend, cage rims and any future HUD
// all agree. Always pair the colour with the glyph + label (color-blind a11y):
// colour alone is never a signal in WARDEN.

export type HarnessTheme = {
  /** Human label rendered in the legend, e.g. "Claude". */
  label: string;
  /** Secondary-accent hex (cage rim + legend swatch), NOT the verdict colour. */
  color: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
};

// Keys are snake_case harness ids exactly as the Rust side emits them
// ("claude_code", "codex"). Anything else falls through to `NEUTRAL`.
//
// Palette (V3, brand-aligned + vivid): luminous, electric colours that glow on
// black. The lattice + crystal form already reads sophisticated, so the colour
// can be saturated and exciting without going childish-flat.
//   • Claude — luminous coral-orange, Claude's own warm brand hue.
//   • Codex  — electric aqua-teal, the cool counter to Claude's warmth.
// This is the single source of truth for harness colour on the web; the orb
// hubs, links and legend all read it (the backend OrbAgent.color is no longer
// rendered). Always pair the colour with the glyph + label (color-blind a11y).
export const HARNESS = {
  claude_code: { label: 'Claude', color: '#ff7d50', glyph: '◆' },
  codex: { label: 'Codex', color: '#2de2c0', glyph: '▲' },
} as const satisfies Record<string, HarnessTheme>;

// Schema drift / off-Fugu / unknown harnesses degrade to a quiet slate chip
// rather than borrowing another harness's identity.
export const NEUTRAL: HarnessTheme = { label: 'Unknown', color: '#9aa7a2', glyph: '●' };

export type HarnessId = keyof typeof HARNESS;

/** Resolve a (possibly unknown) snake_case harness id to its theme. */
export function harnessTheme(h: string): HarnessTheme {
  return (HARNESS as Record<string, HarnessTheme>)[h] ?? NEUTRAL;
}

// Severity ramp — a vivid heat scale that glows: a calm, clear sky-blue at the
// low end climbing through electric amber → orange → crimson. Saturated on
// purpose so danger reads instantly and the scene stays alive, not dull.
export function severityColor(severity: number): string {
  const s = Number.isFinite(severity) ? Math.round(severity) : 0;
  if (s <= 2) return '#54c6ff'; // luminous sky blue — calm / clear
  if (s === 3) return '#ffd23e'; // electric amber
  if (s === 4) return '#ff9332'; // vivid orange
  return '#ff3d52'; // hot crimson
}
