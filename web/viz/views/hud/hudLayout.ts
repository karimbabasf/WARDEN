// hudLayout.ts: the menu-bar HUD's geometry, as pure arithmetic.
//
// The HUD is a Dynamic-Island-style panel that hangs off WARDEN's tray icon: it is
// only ever as big as the fleet it has to show. Every dimension the panel animates
// to comes from HERE, so the spring in `HudPanel` and the genie in `hudGenie` are
// both fed by one source of truth and can never disagree about where an edge is.
//
// Pure module: no React, no Three, no DOM. Unit-tested in hudLayout.test.ts.

/** One agent cell: the globe, a two-line identity, and the status word, in CSS px.
 *  Two lines and not one because the identity is the session's TASK (see hudSort), and
 *  a task truncated to fourteen characters names nothing. */
export const HUD_CELL_W = 88;
export const HUD_CELL_H = 100;
/** Panel padding on every side. */
export const HUD_PAD = 14;
/** The "N agents · M working" summary strip above the grid. */
export const HUD_HEADER_H = 24;
/** The "+N more" strip, present only when the fleet overflows the grid. */
export const HUD_OVERFLOW_H = 20;

export const HUD_MAX_COLS = 5;
export const HUD_MAX_ROWS = 3;
export const HUD_MAX_VISIBLE = HUD_MAX_COLS * HUD_MAX_ROWS;

/** Idle shape: no agents means the island stays a pill, exactly like a resting one. */
export const HUD_PILL_W = 196;
export const HUD_PILL_H = 38;

/** Never narrower than this, so the header line is not clipped on a 1-agent grid. */
export const HUD_MIN_W = 168;

/** How far ABOVE its cell's centre a globe sits. The cell's lower third carries the
 *  label and the status word, so a body drawn at the true centre would sit on its own
 *  caption. Negative is up, matching the DOM's y axis. */
export const HUD_GLOBE_OFFSET_Y = -18;

export type HudGrid = {
  /** Columns in the globe grid; 0 when the panel is a pill. */
  cols: number;
  rows: number;
  /** Agents actually drawn (capped at HUD_MAX_VISIBLE). */
  visible: number;
  /** Agents the cap dropped, surfaced as "+N more" rather than silently hidden. */
  overflow: number;
  width: number;
  height: number;
};

/**
 * Balance `n` agents into a grid that fills its last row as evenly as possible and
 * never exceeds HUD_MAX_COLS. Taking the row count first and dividing back into it
 * is what keeps 11 agents as a tidy 4x3 instead of a ragged 5x3 with a lone straggler.
 */
export function hudGridShape(n: number): { cols: number; rows: number } {
  if (n <= 0) return { cols: 0, rows: 0 };
  const rows = Math.min(HUD_MAX_ROWS, Math.ceil(n / HUD_MAX_COLS));
  const cols = Math.min(HUD_MAX_COLS, Math.ceil(n / rows));
  return { cols, rows };
}

/**
 * The panel's target size for a fleet of `agentCount` root agents. An empty fleet
 * collapses to the pill; anything else is header + grid + optional overflow strip.
 */
export function hudLayout(agentCount: number): HudGrid {
  const total = Math.max(0, Math.floor(agentCount));
  if (total === 0) {
    return { cols: 0, rows: 0, visible: 0, overflow: 0, width: HUD_PILL_W, height: HUD_PILL_H };
  }
  const visible = Math.min(total, HUD_MAX_VISIBLE);
  const overflow = total - visible;
  const { cols, rows } = hudGridShape(visible);
  const width = Math.max(HUD_MIN_W, HUD_PAD * 2 + cols * HUD_CELL_W);
  const height =
    HUD_PAD * 2 + HUD_HEADER_H + rows * HUD_CELL_H + (overflow > 0 ? HUD_OVERFLOW_H : 0);
  return { cols, rows, visible, overflow, width, height };
}

/** Centre of cell `i` inside the panel's content box, in panel-local CSS px. */
export function hudCellCentre(i: number, grid: HudGrid): { x: number; y: number } {
  if (grid.cols === 0) return { x: grid.width / 2, y: grid.height / 2 };
  const col = i % grid.cols;
  const row = Math.floor(i / grid.cols);
  // The grid is centred in the panel: a short last row sits under the middle of the
  // rows above it rather than jamming left, which is what stops a 7-agent board
  // reading as a mistake.
  const inThisRow = Math.min(grid.cols, grid.visible - row * grid.cols);
  const rowW = inThisRow * HUD_CELL_W;
  const left = (grid.width - rowW) / 2;
  const colInRow = col;
  return {
    x: left + colInRow * HUD_CELL_W + HUD_CELL_W / 2,
    y: HUD_PAD + HUD_HEADER_H + row * HUD_CELL_H + HUD_CELL_H / 2,
  };
}

/**
 * Where the panel sits inside its (fixed, oversized, transparent) window: centred
 * under the tray icon, but pushed back inside the window if that would hang it off
 * an edge. `iconCentreX` and the result are both window-local CSS px.
 */
export function hudPanelLeft(iconCentreX: number, panelW: number, windowW: number, margin = 8): number {
  const ideal = iconCentreX - panelW / 2;
  const max = windowW - panelW - margin;
  if (max < margin) return Math.max(0, (windowW - panelW) / 2);
  return Math.min(Math.max(ideal, margin), max);
}
