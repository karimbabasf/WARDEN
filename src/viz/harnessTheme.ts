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
export const HARNESS = {
  claude_code: { label: 'Claude', color: '#3dffa0', glyph: '◆' },
  codex: { label: 'Codex', color: '#b98cff', glyph: '▲' },
} as const satisfies Record<string, HarnessTheme>;

// Schema drift / off-Fugu / unknown harnesses degrade to a neutral phosphor
// chip rather than borrowing another harness's identity.
export const NEUTRAL: HarnessTheme = { label: 'Unknown', color: '#76ff9d', glyph: '●' };

export type HarnessId = keyof typeof HARNESS;

/** Resolve a (possibly unknown) snake_case harness id to its theme. */
export function harnessTheme(h: string): HarnessTheme {
  return (HARNESS as Record<string, HarnessTheme>)[h] ?? NEUTRAL;
}
