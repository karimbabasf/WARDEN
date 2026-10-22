// pacer.ts: plays a bench's op stream at a watchable speed without falling behind.
//
// Agents are bursty (30 calls in 10 s, then minutes of thinking), so a fixed clip
// length either lags or idles. The rules, from the spec:
// * A brick placement takes PLACE_MS at normal speed.
// * When more than CATCHUP_MS of work is queued, playback speeds up, to MAX_SPEED.
// * When more than FLURRY bricks are queued, the extra placements play as a fast
//   flurry, so a burst of 60 calls lands in about 6 s instead of 36.
// * Browsing is skippable: when behind, page flips are dropped, never bricks.
// * After IDLE_MS with nothing to play, the bench is idle (the renderer fills it).
// Pure: time comes in through `tick`, so tests drive it with no clock.

import type { BuildOp, OpKind } from './planner';

export const PLACE_MS = 600;
export const CATCHUP_MS = 3000;
export const MAX_SPEED = 4;
export const FLURRY = 8;
export const FLURRY_MS = 40;
export const IDLE_MS = 5000;

const BASE_MS: Record<OpKind, number> = {
  place: PLACE_MS,
  pull: 700,
  browse: 380,
  page: 520,
  spawn: 450,
  ask: 600,
  claim: 1400,
  attach: 1600,
  complete: 1500,
};

const SKIPPABLE: ReadonlySet<OpKind> = new Set(['browse', 'page', 'spawn']);

export type Playing = { op: BuildOp; elapsed: number; duration: number };

export class Pacer {
  private queue: BuildOp[] = [];
  private current: Playing | null = null;
  private quietMs = 0;
  readonly reducedMotion: boolean;

  constructor(reducedMotion = false) {
    this.reducedMotion = reducedMotion;
  }

  push(ops: BuildOp[]) {
    this.queue.push(...ops);
  }

  get playing(): Playing | null {
    return this.current;
  }
  get pending(): number {
    return this.queue.length;
  }
  /** True after IDLE_MS with nothing queued or playing. */
  get idle(): boolean {
    return this.current === null && this.queue.length === 0 && this.quietMs >= IDLE_MS;
  }

  /** The playing op and the next `n - 1` queued, in play order. */
  lookahead(n: number): BuildOp[] {
    const out = this.current ? [this.current.op] : [];
    return out.concat(this.queue.slice(0, Math.max(0, n - out.length)));
  }

  /** Queued work in ms at normal speed, including what is left of the current op. */
  backlogMs(): number {
    let ms = this.current ? Math.max(0, this.current.duration - this.current.elapsed) : 0;
    for (const op of this.queue) ms += BASE_MS[op.kind];
    return ms;
  }

  speed(): number {
    return Math.min(MAX_SPEED, Math.max(1, this.backlogMs() / CATCHUP_MS));
  }

  private queuedPlaces(): number {
    let n = 0;
    for (const op of this.queue) if (op.kind === 'place') n++;
    return n;
  }

  private durationFor(op: BuildOp): number {
    if (this.reducedMotion) return 0;
    if (op.kind === 'place' && this.queuedPlaces() >= FLURRY) return FLURRY_MS;
    return BASE_MS[op.kind] / this.speed();
  }

  /**
   * Advance by `dt` ms. Returns every op that FINISHED during this tick, in order, so
   * the renderer can commit their end state even when several fit in one frame.
   */
  tick(dt: number): BuildOp[] {
    const done: BuildOp[] = [];
    let budget = dt;
    if (!this.current && this.queue.length === 0) {
      this.quietMs += dt;
      return done;
    }
    this.quietMs = 0;
    while (budget > 0 || (this.current && this.current.duration === 0)) {
      if (!this.current) {
        while (this.queue.length > 0 && this.backlogMs() > CATCHUP_MS && SKIPPABLE.has(this.queue[0].kind)) {
          done.push(this.queue.shift() as BuildOp);
        }
        const next = this.queue.shift();
        if (!next) break;
        this.current = { op: next, elapsed: 0, duration: this.durationFor(next) };
      }
      const left = this.current.duration - this.current.elapsed;
      if (left <= budget) {
        budget -= left;
        done.push(this.current.op);
        this.current = null;
      } else {
        this.current.elapsed += budget;
        budget = 0;
      }
    }
    return done;
  }

  /** Progress of the playing op in [0, 1], or null when nothing is playing. */
  progress(): number | null {
    if (!this.current) return null;
    return this.current.duration === 0 ? 1 : Math.min(1, this.current.elapsed / this.current.duration);
  }
}
