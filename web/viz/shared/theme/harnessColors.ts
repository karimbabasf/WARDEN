// harnessColors.ts: single source of truth for harness identity on the web side.
//
// Every harness-aware component (Habits constellation, Radar globes, legend, HUD)
// sources its colour, glyph, and label from HERE, with no literals scattered
// across theme files. Colour is ALWAYS paired with a glyph + label (color-blind
// a11y requirement: colour alone is never a signal in WARDEN).
//
// Pure module: no side effects, no imports from Three.js or React.

export type HarnessId =
  | 'claude_code'
  | 'openclaw'
  | 'codex'
  | 'cursor'
  | 'grok'
  | 'hermes'
  | 'unknown';

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
 * Every hue here is the IDLE / resting colour. A working globe blazes from this
 * value toward white-hot in the render, so brightness is the liveness signal and
 * the hue is only ever identity. That is what makes the constraint below the real
 * design problem: these have to stay apart from each other while DIM, on a
 * near-black void (`--bg` #020403).
 *
 * Two regions of the wheel are unavailable, and they are not preferences:
 *
 * * **Red, 340 to 10 degrees, is AWAITING.** `--alert` #ff2740 means an agent is
 *   stopped on the operator, and it is the one colour in the app that demands a
 *   response. A harness identity anywhere near it would teach the eye to discount
 *   the alarm, which is the single failure mode this palette must not have.
 * * **Green, 90 to 160 degrees, is out** by the design system's own rule.
 *
 * That leaves four usable arcs, and the six harnesses are spread across them:
 *
 *   claude_code  #ff8636   25deg  tangy tangerine, Claude's warm clay orange
 *   openclaw     #ffc247   40deg  amber
 *   codex        #4fc9ff  200deg  cyan-ice, Codex's cool bluish light
 *   cursor       #7c9cff  228deg  periwinkle
 *   grok         #b47cff  268deg  violet
 *   hermes       #ff6ec7  322deg  magenta
 *   unknown      #8fa0b8         slate, borrows no brand hue
 *
 * CLAUDE AND OPENCLAW SIT 15 DEGREES APART ON PURPOSE, and it is the one pair
 * here that is deliberately close. OpenClaw is a Claude Code fork: two sessions
 * of the two of them are more alike than either is to a Codex session, and the
 * palette should say so rather than assign an arbitrary far-away hue that implies
 * a difference the runtime does not have. They are still separated by glyph
 * (diamond against hexagon) and by label, which is what the a11y rule requires
 * and what carries the distinction when the hues are dim.
 */
export const HARNESS_COLORS: Record<HarnessId, HarnessColor> = {
  claude_code: { id: 'claude_code', hue: '#ff8636', glyph: '◆', label: 'Claude' },
  openclaw:    { id: 'openclaw',    hue: '#ffc247', glyph: '⬢', label: 'OpenClaw' },
  codex:       { id: 'codex',       hue: '#4fc9ff', glyph: '▣', label: 'Codex' },
  cursor:      { id: 'cursor',      hue: '#7c9cff', glyph: '▲', label: 'Cursor' },
  grok:        { id: 'grok',        hue: '#b47cff', glyph: '✦', label: 'Grok' },
  hermes:      { id: 'hermes',      hue: '#ff6ec7', glyph: '●', label: 'Hermes' },
  unknown:     { id: 'unknown',     hue: '#8fa0b8', glyph: '◇', label: 'Unknown' },
};

/**
 * Every harness with a real identity, in legend order. `unknown` is excluded: it
 * is the fallback, not a harness.
 */
export const KNOWN_HARNESS_IDS: readonly HarnessId[] = [
  'claude_code',
  'openclaw',
  'codex',
  'cursor',
  'grok',
  'hermes',
];

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
