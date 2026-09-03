// The HarnessTheme view of harness identity: the shape the legend, cage rims and
// the HUD consume. Colours, glyphs and labels themselves live in
// `harnessColors.ts`, which is the one source of truth; this file only reshapes
// them so every surface agrees. Always pair the colour with the glyph + label
// (color-blind a11y): colour alone is never a signal in WARDEN.

import { harnessColor, KNOWN_HARNESS_IDS, type HarnessId } from './harnessColors';

export type HarnessTheme = {
  /** Human label rendered in the legend, e.g. "Claude". */
  label: string;
  /** Secondary-accent hex (cage rim + legend swatch), NOT the verdict colour. */
  color: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
};

// Keys are snake_case harness ids exactly as the Rust side emits them
// ("claude_code", "codex", "grok"). Anything else falls through to `NEUTRAL`.
// Values are sourced from harnessColors, with no literals duplicated here.
//
// Built by MAPPING the canonical id list rather than by hand, so a harness added
// to `harnessColors.ts` cannot be forgotten here and silently render as Unknown.
// That is exactly what happened while the table held only Claude and Codex: the
// Rust `Harness` enum already carried Cursor and Hermes, and both would have
// drawn as neutral slate.
export const HARNESS = Object.fromEntries(
  KNOWN_HARNESS_IDS.map((id) => {
    const c = harnessColor(id);
    return [id, { label: c.label, color: c.hue, glyph: c.glyph }];
  }),
) as Record<string, HarnessTheme>;

// Schema drift / off-Fugu / unknown harnesses degrade to a quiet slate chip
// rather than borrowing another harness's identity.
const _un = harnessColor('unknown');
export const NEUTRAL: HarnessTheme = { label: _un.label, color: _un.hue, glyph: _un.glyph };

export type { HarnessId };

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
