// harnessColors.ts — single source of truth for harness identity on the web side.
//
// Every harness-aware component (Habits constellation, Radar globes, legend, HUD)
// sources claude/codex/unknown colour, glyph, and label from HERE — no literals
// scattered across theme files. Colour is ALWAYS paired with a glyph + label
// (color-blind a11y requirement: colour alone is never a signal in WARDEN).
//
// Pure module: no side effects, no imports from Three.js or React.

export type HarnessId = 'claude_code' | 'codex' | 'unknown';

export interface HarnessColor {
  id: HarnessId;
  /** Base hex hue for this harness (e.g. '#ff7a18'). */
  hue: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
  /** Human-readable label for legend, cards, and screen-reader text. */
  label: string;
}

/**
 * Canonical harness colour/glyph/label table.
 *
 * Hues are deliberately VIBRANT and maximally distinct so a globe's harness reads
 * at a glance on --bg #020403: Claude a vivid orange, Codex an electric blue.
 * (Always paired with a glyph + label — colour alone is never a signal.)
 *   claude_code → vivid orange   #ff7a18  ◆  Claude
 *   codex       → electric blue  #2e8bff  ▣  Codex
 *   unknown     → slate          #8fa0b8  ◇  Unknown
 */
export const HARNESS_COLORS: Record<HarnessId, HarnessColor> = {
  claude_code: { id: 'claude_code', hue: '#ff7a18', glyph: '◆', label: 'Claude' },
  codex:       { id: 'codex',       hue: '#2e8bff', glyph: '▣', label: 'Codex' },
  unknown:     { id: 'unknown',     hue: '#8fa0b8', glyph: '◇', label: 'Unknown' },
};

/**
 * Resolve a (possibly null/undefined/unrecognised) harness string to its
 * canonical colour entry. Lowercases + trims before lookup; falls back to
 * the `unknown` neutral so unknown harnesses never borrow a brand hue.
 */
export function harnessColor(harness: string | null | undefined): HarnessColor {
  if (!harness) return HARNESS_COLORS.unknown;
  const key = harness.toLowerCase().trim() as HarnessId;
  return HARNESS_COLORS[key] ?? HARNESS_COLORS.unknown;
}
