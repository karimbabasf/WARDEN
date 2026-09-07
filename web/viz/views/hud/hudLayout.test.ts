import { describe, expect, it } from 'vitest';
import {
  HUD_CELL_H,
  HUD_CELL_W,
  HUD_MAX_VISIBLE,
  HUD_PAD,
  HUD_PILL_H,
  HUD_PILL_W,
  HUD_KID_LINE_H,
  HUD_KID_TAIL,
  HUD_MAX_H,
  HUD_MAX_KID_LINES,
  HUD_MAX_W,
  HUD_EMBED_KID_LINE_H,
  HUD_EMBED_MAX_PITCH,
  HUD_EMBED_PAD,
  HUD_EMBED_PAD_MIN,
  HUD_KIDS_PER_LINE,
  hudCellCentre,
  hudEmbedLayout,
  hudGridShape,
  hudKidCentre,
  hudKidLines,
  hudLayout,
  hudPanelLeft,
} from './hudLayout';

/** A fleet of `n` roots, none of them running subagents: the shape every case below
 *  was written against, before height started following the moons too. */
const bare = (n: number) => new Array(n).fill(0);

describe('hudGridShape', () => {
  it('keeps a small fleet on one row', () => {
    expect(hudGridShape(1)).toEqual({ cols: 1, rows: 1 });
    expect(hudGridShape(4)).toEqual({ cols: 4, rows: 1 });
    expect(hudGridShape(5)).toEqual({ cols: 5, rows: 1 });
  });

  it('balances rather than filling rows greedily', () => {
    // 6 agents is 3x2, not 5+1: a lone straggler under a full row reads as broken.
    expect(hudGridShape(6)).toEqual({ cols: 3, rows: 2 });
    expect(hudGridShape(7)).toEqual({ cols: 4, rows: 2 });
    expect(hudGridShape(11)).toEqual({ cols: 4, rows: 3 });
  });

  it('never exceeds the grid cap', () => {
    for (let n = 1; n <= HUD_MAX_VISIBLE; n++) {
      const g = hudGridShape(n);
      expect(g.cols).toBeLessThanOrEqual(5);
      expect(g.rows).toBeLessThanOrEqual(3);
      expect(g.cols * g.rows).toBeGreaterThanOrEqual(n);
    }
  });

  it('is empty for an empty fleet', () => {
    expect(hudGridShape(0)).toEqual({ cols: 0, rows: 0 });
    expect(hudGridShape(-4)).toEqual({ cols: 0, rows: 0 });
  });
});

describe('hudLayout', () => {
  it('stays a pill with nothing running', () => {
    const l = hudLayout(bare(0));
    expect(l).toMatchObject({ cols: 0, rows: 0, visible: 0, overflow: 0 });
    expect(l.width).toBe(HUD_PILL_W);
    expect(l.height).toBe(HUD_PILL_H);
  });

  it('grows with the fleet', () => {
    const one = hudLayout(bare(1));
    const four = hudLayout(bare(4));
    const twelve = hudLayout(bare(12));
    expect(four.width).toBeGreaterThan(one.width);
    expect(twelve.height).toBeGreaterThan(four.height);
  });

  it('sizes width from the columns it actually draws', () => {
    const l = hudLayout(bare(4));
    expect(l.width).toBe(HUD_PAD * 2 + 4 * HUD_CELL_W);
  });

  it('caps the grid and reports the remainder instead of hiding it', () => {
    const l = hudLayout(bare(23));
    expect(l.visible).toBe(HUD_MAX_VISIBLE);
    expect(l.overflow).toBe(8);
    // the overflow strip costs real height, so it is in the number the panel springs to
    expect(l.height).toBeGreaterThan(hudLayout(bare(HUD_MAX_VISIBLE)).height);
  });

  it('never returns a width below the header minimum', () => {
    expect(hudLayout(bare(1)).width).toBeGreaterThanOrEqual(168);
  });
});

describe('hudCellCentre', () => {
  it('centres a full row across the panel', () => {
    const grid = hudLayout(bare(4));
    const first = hudCellCentre(0, grid);
    const last = hudCellCentre(3, grid);
    expect(first.x + last.x).toBeCloseTo(grid.width, 5);
    expect(first.y).toBe(last.y);
  });

  it('centres a SHORT last row under the rows above it', () => {
    const grid = hudLayout(bare(7)); // 4x2, last row holds 3
    const rowTwoFirst = hudCellCentre(4, grid);
    const rowTwoLast = hudCellCentre(6, grid);
    expect(rowTwoFirst.x + rowTwoLast.x).toBeCloseTo(grid.width, 5);
    expect(rowTwoFirst.x).toBeGreaterThan(hudCellCentre(0, grid).x);
  });

  it('steps down by exactly one cell height per row', () => {
    const grid = hudLayout(bare(8));
    expect(hudCellCentre(4, grid).y - hudCellCentre(0, grid).y).toBe(HUD_CELL_H);
  });
});

describe('subagent strips drive the height', () => {
  it('costs nothing when nothing is running inside a session', () => {
    expect(hudKidLines(0)).toBe(0);
    expect(hudLayout([0, 0, 0]).height).toBe(hudLayout(bare(3)).height);
  });

  it('grows the row that carries the moons, and only that row', () => {
    // 8 roots is 4x2. Loading a subagent onto row TWO must not move row one's cells.
    const quiet = hudLayout(bare(8));
    const busy = hudLayout([0, 0, 0, 0, 6, 0, 0, 0]);
    expect(busy.height).toBeGreaterThan(quiet.height);
    expect(busy.rowHeights[0]).toBe(HUD_CELL_H);
    expect(busy.rowHeights[1]).toBe(HUD_CELL_H + 2 * HUD_KID_LINE_H + HUD_KID_TAIL);
    expect(hudCellCentre(0, busy)).toEqual(hudCellCentre(0, quiet));
  });

  it('is as tall as the busiest cell in the row, not the sum of them', () => {
    const one = hudLayout([6, 0, 0, 0]);
    const four = hudLayout([6, 6, 6, 6]);
    expect(four.height).toBe(one.height);
  });

  it('caps the lines so one crowded session cannot make the panel a column', () => {
    expect(hudKidLines(3)).toBe(1);
    expect(hudKidLines(6)).toBe(2);
    expect(hudKidLines(40)).toBe(HUD_MAX_KID_LINES);
  });

  it('never lets a panel exceed the canvas it is drawn on', () => {
    const worst = hudLayout(new Array(HUD_MAX_VISIBLE).fill(99));
    expect(worst.height).toBeLessThanOrEqual(HUD_MAX_H);
    expect(worst.width).toBeLessThanOrEqual(HUD_MAX_W);
  });
});

describe('hudKidCentre', () => {
  it('hangs the moons BELOW their session, never over its caption', () => {
    const grid = hudLayout([3]);
    const cell = hudCellCentre(0, grid);
    const moon = hudKidCentre(0, 0, 3, grid);
    expect(moon.y).toBeGreaterThan(cell.y + HUD_CELL_H / 2 - 1);
  });

  it('centres a short line under the cell rather than jamming it left', () => {
    const grid = hudLayout([3]);
    const cell = hudCellCentre(0, grid);
    const first = hudKidCentre(0, 0, 3, grid);
    const last = hudKidCentre(0, 2, 3, grid);
    expect((first.x + last.x) / 2).toBeCloseTo(cell.x, 5);
  });

  it('wraps onto a second line and drops it one line height', () => {
    const grid = hudLayout([7]);
    const sixth = hudKidCentre(0, 5, 7, grid);
    const first = hudKidCentre(0, 0, 7, grid);
    expect(sixth.y - first.y).toBe(HUD_KID_LINE_H);
  });
});

describe('hudPanelLeft', () => {
  it('centres the panel under the icon when there is room', () => {
    expect(hudPanelLeft(280, 400, 560)).toBe(80);
  });

  it('pushes back inside the window rather than hanging off an edge', () => {
    expect(hudPanelLeft(40, 400, 560)).toBe(8);
    expect(hudPanelLeft(540, 400, 560)).toBe(152);
  });

  it('centres when the panel is wider than the window', () => {
    expect(hudPanelLeft(100, 600, 560)).toBe(0);
  });
});

describe('hudEmbedLayout', () => {
  // boring.notch's real body box: openNotchSize 640x190 less the header and the
  // cornerRadiusInsets it pads the content with. Every number below is judged against
  // this one, because it is the only host this layout has.
  const NOTCH = { width: 578, height: 124 };

  it('takes the box it is given rather than deriving one', () => {
    // The whole difference from the island. Its width IS the notch's, so the section
    // cannot be a card floating inside a wider panel, which is what it looked like.
    const l = hudEmbedLayout(bare(4), NOTCH);
    expect(l.width).toBe(NOTCH.width);
    expect(l.height).toBe(NOTCH.height);
  });

  it('spreads the fleet across that width, up to a ceiling', () => {
    const l = hudEmbedLayout(bare(4), NOTCH);
    expect(l.cols).toBe(4);
    expect(l.rows).toBe(1);
    // 562 inner / 4 would be 140, which is past the point where a board stops reading
    // as one group, so the pitch stops at the ceiling and the row centres itself.
    expect(l.pitch).toBe(HUD_EMBED_MAX_PITCH);
    expect(l.pitch).toBeGreaterThan(HUD_CELL_W);
  });

  it('never packs tighter than a cell', () => {
    // Six across 562px is 93 a cell; eight would be 70, which would overlap the
    // captions, so the column count is capped by what a full-width cell needs.
    const l = hudEmbedLayout(bare(12), NOTCH);
    expect(l.pitch).toBeGreaterThanOrEqual(HUD_CELL_W);
    expect(l.cols * HUD_CELL_W).toBeLessThanOrEqual(NOTCH.width - HUD_EMBED_PAD * 2);
  });

  it('draws only the rows the host can actually show, and names the rest', () => {
    // The notch is one cell tall. A second row would be drawn past its bottom edge,
    // where the host clips it with nothing on screen to say it was there.
    const l = hudEmbedLayout(bare(12), NOTCH);
    expect(l.rows).toBe(1);
    expect(l.visible + l.overflow).toBe(12);
    expect(l.overflow).toBeGreaterThan(0);
  });

  it('uses a second row when the host is tall enough for one', () => {
    const l = hudEmbedLayout(bare(8), { width: 578, height: 260 });
    expect(l.rows).toBe(2);
    expect(l.overflow).toBe(0);
  });

  it('centres the fleet in the box vertically', () => {
    const l = hudEmbedLayout(bare(3), NOTCH);
    const top = l.rowTops[0];
    const bottom = NOTCH.height - (top + l.rowHeights[0]);
    expect(Math.abs(top - bottom)).toBeLessThanOrEqual(1);
  });

  it('spills downward rather than losing its first row off the top', () => {
    // A fleet taller than the box (a row of moons in a short notch) has to keep its
    // globes; the host clips the bottom, which is recoverable, not the top.
    const l = hudEmbedLayout([8, 8, 8], { width: 578, height: 90 });
    expect(l.rowTops[0]).toBeGreaterThanOrEqual(HUD_EMBED_PAD_MIN);
  });

  it('keeps a line of moons in a notch that is barely a cell tall', () => {
    // The case this exists for, and it lives on a knife edge: 124 is the shortest box
    // the notch plausibly hands over, and a strip at the island's own line height and
    // tail does not fit in it. Losing the subagents to keep a margin is the wrong
    // trade, so the margin goes first.
    const l = hudEmbedLayout([6, 0, 0], { width: 578, height: 124 });
    expect(l.kidCap).toBe(HUD_KIDS_PER_LINE);
    expect(l.kidLineH).toBe(HUD_EMBED_KID_LINE_H);
    expect(l.rowHeights[0]).toBeGreaterThan(HUD_CELL_H);
    // Drawn inside the box, not past its bottom edge where the host would clip it.
    expect(l.rowTops[0] + l.rowHeights[0]).toBeLessThanOrEqual(124);
  });

  it('gives a taller host its second line of moons', () => {
    const l = hudEmbedLayout([8, 0], { width: 578, height: 170 });
    expect(l.kidCap).toBe(HUD_KIDS_PER_LINE * 2);
    expect(l.rowTops[0] + l.rowHeights[0]).toBeLessThanOrEqual(170);
  });

  it('drops the strip rather than clipping it when the box has no room', () => {
    // A fleet that overflows spends its spare height on the "+N more" strip, which is
    // the stronger claim: a board that hides agents and says nothing is lying.
    const l = hudEmbedLayout(new Array(12).fill(6), { width: 578, height: 124 });
    expect(l.overflow).toBeGreaterThan(0);
    expect(l.kidCap).toBe(0);
    expect(l.rowHeights[0]).toBe(HUD_CELL_H);
  });

  it('sizes rows with the same line height it places moons at', () => {
    // One number, on the grid, because two would drift and the drift is a strip
    // hanging off the bottom of the notch.
    const l = hudEmbedLayout([4, 0], { width: 578, height: 132 });
    const cell = hudCellCentre(0, l);
    const lastMoon = hudKidCentre(0, 3, 4, l);
    expect(lastMoon.y).toBeLessThanOrEqual(cell.y - HUD_CELL_H / 2 + l.rowHeights[0]);
  });

  it('centres a lone globe instead of parking it at the left edge', () => {
    const l = hudEmbedLayout(bare(1), NOTCH);
    expect(hudCellCentre(0, l).x).toBeCloseTo(NOTCH.width / 2, 0);
  });

  it('draws nothing for an empty fleet, and still fills the box', () => {
    const l = hudEmbedLayout(bare(0), NOTCH);
    expect(l).toMatchObject({ cols: 0, rows: 0, visible: 0, overflow: 0 });
    expect(l.width).toBe(NOTCH.width);
  });
});
