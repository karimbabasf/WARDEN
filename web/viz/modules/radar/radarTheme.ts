// radarTheme.ts — RADAR's OWN palette. SEPARATE from Habits'
// `harnessTheme.HARNESS` (which stays the anti-pattern map). Per Karim's locked
// direction: a globe's colour is its harness hue ALONE — Claude tangy-orange,
// Codex cyan-ice — and the TWO live signals ride orthogonal channels so each reads
// cleanly: SIZE = context occupancy (the layout radius), BRIGHTNESS = liveness
// (working blazes white-hot, idle dims). Colour never encodes load, so a quiet
// near-full agent and a busy near-empty one stay the same hue — only size differs.
// Colour is ALWAYS paired with a glyph + label (color-blind a11y).
//
// Pure module: no Three.js import, so it is trivially unit-testable and shared by
// the layout, the render and the detail panel without dragging in WebGL.
//
// Harness colour/glyph/label literals live in `harnessColors.ts` — no duplication.

import { harnessColor } from '@/viz/shared/theme/harnessColors';

export type RadarTheme = {
  /** Human label rendered in cards/legend, e.g. "Claude". */
  label: string;
  /** Base harness hue (the ember/heat ramp is anchored on this). */
  color: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
};

// Keys are snake_case harness ids exactly as the backend emits them.
// Values sourced from harnessColors — no literals duplicated here.
const _cl = harnessColor('claude_code');
const _cx = harnessColor('codex');
export const RADAR_PALETTE = {
  claude_code: { label: _cl.label, color: _cl.hue, glyph: _cl.glyph },
  codex:       { label: _cx.label, color: _cx.hue, glyph: _cx.glyph },
} as const satisfies Record<string, RadarTheme>;

// Unknown / schema-drift harness — a quiet slate globe, never borrowing another
// harness's identity (honest-viz). Distinct hue from both brand colours.
const _un = harnessColor('unknown');
export const RADAR_NEUTRAL: RadarTheme = { label: _un.label, color: _un.hue, glyph: _un.glyph };

export type RadarHarnessId = keyof typeof RADAR_PALETTE;

/** Resolve a (possibly unknown) snake_case harness id to its Radar theme. */
export function radarHarness(h: string): RadarTheme {
  return (RADAR_PALETTE as Record<string, RadarTheme>)[h] ?? RADAR_NEUTRAL;
}
