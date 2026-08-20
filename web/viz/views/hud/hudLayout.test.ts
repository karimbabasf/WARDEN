import { describe, expect, it } from 'vitest';
import {
  HUD_CELL_H,
  HUD_CELL_W,
  HUD_MAX_VISIBLE,
  HUD_PAD,
  HUD_PILL_H,
  HUD_PILL_W,
  hudCellCentre,
  hudGridShape,
  hudLayout,
  hudPanelLeft,
} from './hudLayout';

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
    const l = hudLayout(0);
    expect(l).toMatchObject({ cols: 0, rows: 0, visible: 0, overflow: 0 });
    expect(l.width).toBe(HUD_PILL_W);
    expect(l.height).toBe(HUD_PILL_H);
  });

  it('grows with the fleet', () => {
    const one = hudLayout(1);
    const four = hudLayout(4);
    const twelve = hudLayout(12);
    expect(four.width).toBeGreaterThan(one.width);
    expect(twelve.height).toBeGreaterThan(four.height);
  });

  it('sizes width from the columns it actually draws', () => {
    const l = hudLayout(4);
    expect(l.width).toBe(HUD_PAD * 2 + 4 * HUD_CELL_W);
  });

  it('caps the grid and reports the remainder instead of hiding it', () => {
    const l = hudLayout(23);
    expect(l.visible).toBe(HUD_MAX_VISIBLE);
    expect(l.overflow).toBe(8);
    // the overflow strip costs real height, so it is in the number the panel springs to
    expect(l.height).toBeGreaterThan(hudLayout(HUD_MAX_VISIBLE).height);
  });

  it('never returns a width below the header minimum', () => {
    expect(hudLayout(1).width).toBeGreaterThanOrEqual(168);
  });
});

describe('hudCellCentre', () => {
  it('centres a full row across the panel', () => {
    const grid = hudLayout(4);
    const first = hudCellCentre(0, grid);
    const last = hudCellCentre(3, grid);
    expect(first.x + last.x).toBeCloseTo(grid.width, 5);
    expect(first.y).toBe(last.y);
  });

  it('centres a SHORT last row under the rows above it', () => {
    const grid = hudLayout(7); // 4x2, last row holds 3
    const rowTwoFirst = hudCellCentre(4, grid);
    const rowTwoLast = hudCellCentre(6, grid);
    expect(rowTwoFirst.x + rowTwoLast.x).toBeCloseTo(grid.width, 5);
    expect(rowTwoFirst.x).toBeGreaterThan(hudCellCentre(0, grid).x);
  });

  it('steps down by exactly one cell height per row', () => {
    const grid = hudLayout(8);
    expect(hudCellCentre(4, grid).y - hudCellCentre(0, grid).y).toBe(HUD_CELL_H);
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
