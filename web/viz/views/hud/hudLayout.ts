// hudLayout.ts: the menu-bar HUD's geometry, as pure arithmetic.
//
// The HUD is a Dynamic-Island-style panel that hangs off WARDEN's tray icon: it is
// only ever as big as the fleet it has to show. Every dimension the panel animates
// to comes from HERE, so the spring in `HudPanel` and the genie in `hudGenie` are
// both fed by one source of truth and can never disagree about where an edge is.
//
// The panel grows in BOTH directions and for two different reasons. Width and row
// count follow how many sessions are running. Row HEIGHT follows how much work is
// running inside them: a row carrying a session with six subagents is taller than a
// row of bare sessions, because those moons need somewhere to sit. That is why a row
// height is a number per row here and not one constant.
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

/** ── subagents ─────────────────────────────────────────────────────────────
 *  A root's live subagents hang under its cell as a short strip of moons. They are
 *  drawn, never offered: a subagent has no window to raise, so it gets no cell, no
 *  hover and no click (see HudPanel). The strip is what makes the panel's height
 *  dynamic. */
/** Moons per line, and the line's height in px. Four and not five: at a 16px pitch
 *  five of them fill an 88px cell edge to edge, and two neighbouring strips ran
 *  together into one long row that read as a single session's work. */
export const HUD_KIDS_PER_LINE = 4;
export const HUD_KID_LINE_H = 18;
/** Hard ceiling on lines, so one busy session cannot make the panel a column. */
export const HUD_MAX_KID_LINES = 2;
/** Horizontal spacing between moons on a line. */
export const HUD_KID_PITCH = 16;
/** Air under the last line of moons. Without it a strip sits an equal distance from
 *  its own caption and from the NEXT row's globe, and stops reading as belonging to
 *  either. The gap is what makes the grouping obvious. */
export const HUD_KID_TAIL = 8;
/** A moon's body radius in CSS px. Deliberately about a third of a root's, the same
 *  ratio the war room's layout gives a subagent. */
export const HUD_KID_RADIUS = 5.5;

/** Idle shape: no agents means the island stays a pill, exactly like a resting one. */
export const HUD_PILL_W = 196;
export const HUD_PILL_H = 38;

/** Never narrower than this, so the header line is not clipped on a 1-agent grid. */
export const HUD_MIN_W = 168;

/** How far ABOVE its cell's centre a globe sits. The cell's lower third carries the
 *  label and the status word, so a body drawn at the true centre would sit on its own
 *  caption. Negative is up, matching the DOM's y axis. */
export const HUD_GLOBE_OFFSET_Y = -18;

/** Body radius in CSS px, read off context occupancy exactly as the radar does:
 *  context is the SIZE channel on both screens. */
export function hudGlobeRadius(fillPct: number): number {
  const f = Number.isFinite(fillPct) ? Math.max(0, Math.min(1, fillPct)) : 0;
  return 15 + f * 5;
}

/** Lines of moons one root needs. Zero kids means zero lines, so a fleet with no
 *  subagents lays out exactly as it did before subagents were drawn at all. */
export function hudKidLines(kidCount: number): number {
  const n = Math.max(0, Math.floor(kidCount));
  if (n === 0) return 0;
  return Math.min(HUD_MAX_KID_LINES, Math.ceil(n / HUD_KIDS_PER_LINE));
}

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
  /** Horizontal step between cell centres, and the width of a cell's own box. It is
   *  HUD_CELL_W for the island, whose width is DERIVED from the fleet, and wider in
   *  the embedded section, whose width is GIVEN by the host (see hudEmbedLayout). */
  pitch: number;
  /** Height of each grid row: the base cell, plus room for the tallest moon strip in
   *  that row. One entry per row, so a quiet row stays short next to a busy one. */
  rowHeights: number[];
  /** Top edge of each row, in panel-local px. Cumulative over `rowHeights`. */
  rowTops: number[];
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
 * The panel's target size for a fleet, given each root's live subagent count in
 * board order. An empty fleet collapses to the pill; anything else is header + grid
 * + optional overflow strip, where the grid's height is the sum of its rows.
 *
 * The argument is the KID COUNTS and not a total, because height cannot be derived
 * from a count: five bare sessions and five sessions running four subagents each are
 * the same number and two very different panels.
 */
export function hudLayout(kidCounts: number[]): HudGrid {
  const total = kidCounts.length;
  if (total === 0) {
    return {
      cols: 0,
      rows: 0,
      visible: 0,
      overflow: 0,
      width: HUD_PILL_W,
      height: HUD_PILL_H,
      pitch: HUD_CELL_W,
      rowHeights: [],
      rowTops: [],
    };
  }
  const visible = Math.min(total, HUD_MAX_VISIBLE);
  const overflow = total - visible;
  const { cols, rows } = hudGridShape(visible);
  const width = Math.max(HUD_MIN_W, HUD_PAD * 2 + cols * HUD_CELL_W);

  // A row is as tall as its most crowded cell: moons hang below the caption, so one
  // busy session pushes its whole row down and leaves the rows above it alone.
  const rowHeights: number[] = [];
  const rowTops: number[] = [];
  let y = HUD_PAD + HUD_HEADER_H;
  for (let r = 0; r < rows; r++) {
    let lines = 0;
    for (let c = 0; c < cols; c++) {
      const i = r * cols + c;
      if (i >= visible) break;
      lines = Math.max(lines, hudKidLines(kidCounts[i]));
    }
    const h = HUD_CELL_H + (lines > 0 ? lines * HUD_KID_LINE_H + HUD_KID_TAIL : 0);
    rowTops.push(y);
    rowHeights.push(h);
    y += h;
  }
  const height = y + (overflow > 0 ? HUD_OVERFLOW_H : 0) + HUD_PAD;
  return { cols, rows, visible, overflow, width, height, pitch: HUD_CELL_W, rowHeights, rowTops };
}

/** ── the embedded section ──────────────────────────────────────────────────
 *  The same globes, inside somebody else's panel: the WARDEN tab in the boring.notch
 *  fork. Everything the island DERIVES, the host GIVES. Its width and height are the
 *  notch's, its material is the notch's, and the header is the notch's too, so this
 *  layout drops the summary strip and the outer padding the island needed to be a
 *  card and spends the room on the fleet instead.
 *
 *  The one number that is not simply inherited is the pitch. Four 88px cells in a
 *  568px notch would sit as a tight cluster with a third of the bar empty on either
 *  side, which reads as a web page dropped into a hole rather than a section of the
 *  notch. So the cells SPREAD to fill the width they were given, up to a ceiling:
 *  past that a two-agent fleet would fly to the corners and stop reading as one
 *  board. Globe size is untouched by all of this (it is context occupancy, on both
 *  screens); what moves is the space between them. */

/** Air at the edges. Small: the host has already padded its own panel. */
export const HUD_EMBED_PAD = 8;
/** How far apart cells may spread before the fleet stops reading as one group. */
export const HUD_EMBED_MAX_PITCH = 132;
/** A ceiling on columns for a very wide host, so a fleet never becomes a thin line. */
export const HUD_EMBED_MAX_COLS = 8;

/**
 * The grid for a fleet inside a host box of `width` x `height` CSS px.
 *
 * Rows are capped by what the box can actually SHOW rather than by HUD_MAX_ROWS: the
 * notch is about one cell tall, and a second row drawn past its bottom edge would be
 * clipped by the host with nothing to say it was there. Anything that does not fit is
 * named by the "+N more" strip, exactly as the island names it.
 */
export function hudEmbedLayout(kidCounts: number[], box: { width: number; height: number }): HudGrid {
  const width = Math.max(HUD_MIN_W, Math.round(box.width));
  const height = Math.max(HUD_CELL_H, Math.round(box.height));
  const total = kidCounts.length;
  if (total === 0) {
    return {
      cols: 0,
      rows: 0,
      visible: 0,
      overflow: 0,
      width,
      height,
      pitch: HUD_CELL_W,
      rowHeights: [],
      rowTops: [],
    };
  }

  const inner = Math.max(HUD_CELL_W, width - HUD_EMBED_PAD * 2);
  const maxCols = Math.max(1, Math.min(HUD_EMBED_MAX_COLS, Math.floor(inner / HUD_CELL_W)));
  const fitRows = (reserve: number) =>
    Math.max(1, Math.min(HUD_MAX_ROWS, Math.floor((height - HUD_EMBED_PAD * 2 - reserve) / HUD_CELL_H)));

  // Two passes, because naming the fleet we dropped costs a strip and that strip can
  // be what pushes the last row out of the box.
  let rows = fitRows(0);
  let visible = Math.min(total, maxCols * rows);
  if (total > visible) {
    rows = fitRows(HUD_OVERFLOW_H);
    visible = Math.min(total, maxCols * rows);
  }
  const overflow = total - visible;

  // Balanced the way the island balances, but against the width we were handed.
  rows = Math.max(1, Math.min(rows, Math.ceil(visible / maxCols)));
  const cols = Math.max(1, Math.min(maxCols, Math.ceil(visible / rows)));
  const pitch = Math.min(HUD_EMBED_MAX_PITCH, Math.max(HUD_CELL_W, Math.floor(inner / cols)));

  const rowHeights: number[] = [];
  for (let r = 0; r < rows; r++) {
    let lines = 0;
    for (let c = 0; c < cols; c++) {
      const i = r * cols + c;
      if (i >= visible) break;
      lines = Math.max(lines, hudKidLines(kidCounts[i]));
    }
    rowHeights.push(HUD_CELL_H + (lines > 0 ? lines * HUD_KID_LINE_H + HUD_KID_TAIL : 0));
  }
  const contentH = rowHeights.reduce((a, b) => a + b, 0) + (overflow > 0 ? HUD_OVERFLOW_H : 0);
  // Centred in the box, but never above its top edge: a fleet taller than the host
  // spills DOWNWARD, where the host clips it, rather than losing its first row.
  let y = Math.max(HUD_EMBED_PAD, Math.round((height - contentH) / 2));
  const rowTops: number[] = [];
  for (const h of rowHeights) {
    rowTops.push(y);
    y += h;
  }

  return { cols, rows, visible, overflow, width, height, pitch, rowHeights, rowTops };
}

/** The tallest and widest panel `hudLayout` can ever ask for. The HUD's canvas is cut
 *  once at this size (resizing a WebGL drawing buffer every frame of a spring would
 *  reallocate it every frame), and the native window has to be bigger than it. */
export const HUD_MAX_W = HUD_PAD * 2 + HUD_MAX_COLS * HUD_CELL_W;
export const HUD_MAX_H =
  HUD_PAD * 2 +
  HUD_HEADER_H +
  HUD_MAX_ROWS * (HUD_CELL_H + HUD_MAX_KID_LINES * HUD_KID_LINE_H + HUD_KID_TAIL) +
  HUD_OVERFLOW_H;

/** Which row cell `i` is on. */
function rowOf(i: number, grid: HudGrid): number {
  return grid.cols === 0 ? 0 : Math.floor(i / grid.cols);
}

/** Centre of cell `i` inside the panel's content box, in panel-local CSS px. The cell
 *  is its BASE height only: a taller row hangs its extra space below, which is where
 *  the moons go, so a busy session's globe and caption do not drift off the grid. */
export function hudCellCentre(i: number, grid: HudGrid): { x: number; y: number } {
  if (grid.cols === 0) return { x: grid.width / 2, y: grid.height / 2 };
  const row = rowOf(i, grid);
  const col = i % grid.cols;
  // The grid is centred in the panel: a short last row sits under the middle of the
  // rows above it rather than jamming left, which is what stops a 7-agent board
  // reading as a mistake.
  const inThisRow = Math.min(grid.cols, grid.visible - row * grid.cols);
  const rowW = inThisRow * grid.pitch;
  const left = (grid.width - rowW) / 2;
  return {
    x: left + col * grid.pitch + grid.pitch / 2,
    y: (grid.rowTops[row] ?? HUD_PAD + HUD_HEADER_H) + HUD_CELL_H / 2,
  };
}

/**
 * Centre of one moon under cell `i`, in panel-local CSS px.
 *
 * Lines fill from the top and each line is centred on the cell, so a strip of three
 * sits under the middle of its session rather than hugging one edge. `kidCount` is
 * how many moons are DRAWN (the cap is applied before this), because the last line
 * has to know how wide it is to centre itself.
 */
export function hudKidCentre(
  i: number,
  kidIndex: number,
  kidCount: number,
  grid: HudGrid,
): { x: number; y: number } {
  const cell = hudCellCentre(i, grid);
  const perLine = HUD_KIDS_PER_LINE;
  const line = Math.floor(kidIndex / perLine);
  const col = kidIndex % perLine;
  const inThisLine = Math.min(perLine, kidCount - line * perLine);
  const lineW = inThisLine * HUD_KID_PITCH;
  const cellTop = cell.y - HUD_CELL_H / 2;
  return {
    x: cell.x - lineW / 2 + col * HUD_KID_PITCH + HUD_KID_PITCH / 2,
    y: cellTop + HUD_CELL_H + line * HUD_KID_LINE_H + HUD_KID_LINE_H / 2,
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
