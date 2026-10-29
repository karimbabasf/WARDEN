import { describe, expect, it } from 'vitest';
import { CATCHUP_MS, IDLE_MS, MAX_SPEED, PLACE_MS, Pacer } from './pacer';
import type { BuildOp, OpKind } from './planner';

const op = (n: number, kind: OpKind = 'place'): BuildOp => ({
  n,
  kind,
  actor: 'lead',
  set: 0,
  brick: kind === 'place' ? n : null,
  sub: null,
  step: null,
});

const ops = (count: number, kind: OpKind = 'place') => Array.from({ length: count }, (_, i) => op(i, kind));

/** Run the pacer at 60fps until its queue drains; returns elapsed ms. */
function drain(p: Pacer, maxMs = 120_000): number {
  let t = 0;
  while ((p.pending > 0 || p.playing) && t < maxMs) {
    p.tick(16);
    t += 16;
  }
  return t;
}

describe('Pacer', () => {
  it('plays a lone placement at normal speed', () => {
    const p = new Pacer();
    p.push([op(0)]);
    expect(p.tick(PLACE_MS - 50)).toEqual([]);
    expect(p.progress()).toBeGreaterThan(0.9);
    expect(p.tick(60).map((o) => o.n)).toEqual([0]);
  });

  it('speeds up when behind, but never past the cap', () => {
    const p = new Pacer();
    p.push(ops(8)); // 4.8 s of work
    expect(p.speed()).toBeCloseTo((8 * PLACE_MS) / CATCHUP_MS);
    p.push(ops(12));
    expect(p.speed()).toBe(MAX_SPEED);
  });

  it('a burst of 60 placements drains with the scene never far behind', () => {
    const p = new Pacer();
    p.push(ops(60));
    const t = drain(p);
    // At normal speed this would take 36 s.
    expect(t).toBeLessThan(7000);
    expect(t).toBeGreaterThan(CATCHUP_MS);
  });

  it('every op finishes exactly once and in order', () => {
    const p = new Pacer();
    const all = [...ops(5), op(5, 'browse'), op(6, 'pull'), ...ops(30).map((o) => ({ ...o, n: o.n + 7 }))];
    p.push(all);
    const seen: number[] = [];
    let t = 0;
    while ((p.pending > 0 || p.playing) && t < 60_000) {
      seen.push(...p.tick(16).map((o) => o.n));
      t += 16;
    }
    expect(seen).toEqual(all.map((o) => o.n));
  });

  it('drops page flips when behind, never bricks', () => {
    const p = new Pacer();
    p.push([...ops(10), op(10, 'browse'), op(11, 'browse'), op(12)]);
    const finished: BuildOp[] = [];
    let t = 0;
    while ((p.pending > 0 || p.playing) && t < 30_000) {
      finished.push(...p.tick(16));
      t += 16;
    }
    expect(finished.filter((o) => o.kind === 'place')).toHaveLength(11);
  });

  it('goes idle after a quiet spell', () => {
    const p = new Pacer();
    p.tick(IDLE_MS - 16);
    expect(p.idle).toBe(false);
    p.tick(32);
    expect(p.idle).toBe(true);
    p.push([op(0)]);
    expect(p.idle).toBe(false);
  });

  it('reduced motion commits everything at once', () => {
    const p = new Pacer(true);
    p.push(ops(25));
    expect(p.tick(16)).toHaveLength(25);
  });
});
