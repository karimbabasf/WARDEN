// planner.ts: turns the build logs of everyone working at one bench into a stream of
// build ops. Pure and stateful: it owns WHICH brick each placement takes, never how
// it looks. The renderer plays the ops; it never decides a brick on its own.
//
// Rules, all from the spec:
// * One `place` takes one brick: the lowest free brick in the builder's pool.
// * A `pull` frees exactly the brick its `place` took, if that set is still open.
// * A helper (subagent or teammate) claims a top-level submodel nobody has touched
//   and builds it at a side table. When it finishes, the submodel attaches.
// * The lead never takes a brick inside a submodel a working helper has claimed.
// * A full set is finished: it goes to the shelf and the next set opens.
//
// Entries are folded in ARRIVAL order (each ingest batch merged by timestamp), which
// is what a live bench sees. A replay ingests every log in one batch, so the same
// session always builds the same way.

import type { BuildStep } from './buildLog';

/** A brick range inside a set: one top-level submodel. `end` is exclusive. */
export type SubRange = { start: number; end: number };

/** What the planner needs to know about a set: its size and its submodels. */
export type SetShape = { bricks: number; submodels: SubRange[] };

export type OpKind =
  | 'place'
  | 'pull'
  | 'browse'
  | 'page'
  | 'spawn'
  | 'ask'
  | 'claim' // a helper walks in and takes a submodel
  | 'attach' // a helper's submodel joins the main build
  | 'complete'; // the set is finished and goes to the shelf

export type BuildOp = {
  /** Position in this bench's op stream. */
  n: number;
  kind: OpKind;
  actor: string;
  /** Which set of this bench (0 = the first) the op belongs to. */
  set: number;
  /** The brick placed or pulled, in build order within the set. */
  brick: number | null;
  /** The submodel a claim or attach is about. */
  sub: number | null;
  /** The log entry behind the op, null for structural ops (claim, attach, complete). */
  step: BuildStep | null;
};

type Placement = { set: number; brick: number };

export class BenchPlanner {
  private readonly shapeOf: (set: number) => SetShape;
  private readonly lead: string;
  private set = 0;
  private shape: SetShape;
  /** brick -> actor holding it, or null when free. */
  private owner: (string | null)[];
  private standing = 0;
  private readonly placements = new Map<string, Placement>();
  private readonly claims = new Map<string, number>();
  private readonly subTouched = new Set<number>();
  private readonly cursor = new Map<string, number>();
  private readonly finished = new Set<string>();
  private readonly ops: BuildOp[] = [];

  constructor(leadId: string, shapeOf: (set: number) => SetShape) {
    this.lead = leadId;
    this.shapeOf = shapeOf;
    this.shape = shapeOf(0);
    this.owner = new Array(this.shape.bricks).fill(null);
  }

  get opCount(): number {
    return this.ops.length;
  }
  opsFrom(n: number): BuildOp[] {
    return this.ops.slice(n);
  }
  get currentSet(): number {
    return this.set;
  }
  get standingBricks(): number {
    return this.standing;
  }
  /** Bricks currently standing in the open set, by actor. */
  ownerOf(brick: number): string | null {
    return this.owner[brick] ?? null;
  }
  claimOf(actor: string): number | null {
    return this.claims.get(actor) ?? null;
  }
  /** The highest seq ingested for an actor, so a poll can ask for what came after. */
  lastSeq(actor: string): number | null {
    const c = this.cursor.get(actor);
    return c === undefined ? null : c;
  }

  /**
   * Fold one poll's worth of new entries. Each actor's entries must be in seq order;
   * entries already ingested (seq at or below the cursor) are skipped, so a client can
   * re-send a whole log safely.
   */
  ingest(batch: { actor: string; steps: BuildStep[] }[]): BuildOp[] {
    const before = this.ops.length;
    const fresh: { actor: string; step: BuildStep }[] = [];
    for (const { actor, steps } of batch) {
      const cur = this.cursor.get(actor) ?? -1;
      for (const step of steps) if (step.seq > cur) fresh.push({ actor, step });
    }
    fresh.sort((a, b) => (a.step.ts < b.step.ts ? -1 : a.step.ts > b.step.ts ? 1 : a.step.seq - b.step.seq));
    for (const { actor, step } of fresh) {
      this.cursor.set(actor, Math.max(this.cursor.get(actor) ?? -1, step.seq));
      if (actor !== this.lead && !this.claims.has(actor) && !this.finished.has(actor)) this.claim(actor);
      this.apply(actor, step);
    }
    return this.ops.slice(before);
  }

  /** A helper is done: its submodel joins the build and its claim is released. */
  finish(actor: string): BuildOp[] {
    if (actor === this.lead || this.finished.has(actor)) return [];
    this.finished.add(actor);
    const sub = this.claims.get(actor);
    if (sub === undefined) return [];
    this.claims.delete(actor);
    const before = this.ops.length;
    this.emit('attach', actor, null, sub, null);
    return this.ops.slice(before);
  }

  private emit(kind: OpKind, actor: string, brick: number | null, sub: number | null, step: BuildStep | null) {
    this.ops.push({ n: this.ops.length, kind, actor, set: this.set, brick, sub, step });
  }

  private claim(actor: string) {
    const claimed = new Set(this.claims.values());
    const sub = this.shape.submodels.findIndex((_, i) => !claimed.has(i) && !this.subTouched.has(i));
    if (sub === -1) return; // nothing left to claim: the helper shares the main build
    this.claims.set(actor, sub);
    this.emit('claim', actor, null, sub, null);
  }

  private subOf(brick: number): number {
    return this.shape.submodels.findIndex((r) => brick >= r.start && brick < r.end);
  }

  private nextBrick(actor: string): number | null {
    const mine = this.claims.get(actor);
    if (mine !== undefined) {
      const r = this.shape.submodels[mine];
      for (let b = r.start; b < r.end; b++) if (this.owner[b] === null) return b;
    }
    const blocked = new Set<number>();
    for (const [who, sub] of this.claims) if (who !== actor) blocked.add(sub);
    for (let b = 0; b < this.owner.length; b++) {
      if (this.owner[b] !== null) continue;
      const sub = this.subOf(b);
      if (sub !== -1 && blocked.has(sub)) continue;
      return b;
    }
    return null;
  }

  private apply(actor: string, step: BuildStep) {
    switch (step.action) {
      case 'place': {
        let brick = this.nextBrick(actor);
        if (brick === null) {
          // Every brick this builder may take is down. If the set is not actually
          // full (the rest sit in a helper's claim), the brick still counts: it goes
          // to the next free brick anywhere, because a placement is never dropped.
          brick = this.owner.findIndex((o) => o === null);
          if (brick === -1) {
            this.completeSet();
            brick = this.nextBrick(actor) ?? 0;
          }
        }
        this.owner[brick] = actor;
        this.standing++;
        const sub = this.subOf(brick);
        if (sub !== -1) this.subTouched.add(sub);
        this.placements.set(`${actor}:${step.seq}`, { set: this.set, brick });
        this.emit('place', actor, brick, null, step);
        if (this.standing === this.shape.bricks) this.completeSet();
        return;
      }
      case 'pull': {
        const placed = step.undoes === null ? undefined : this.placements.get(`${actor}:${step.undoes}`);
        // A pull for a set already on the shelf changes nothing on the bench.
        if (!placed || placed.set !== this.set || this.owner[placed.brick] !== actor) return;
        this.owner[placed.brick] = null;
        this.standing--;
        this.emit('pull', actor, placed.brick, null, step);
        return;
      }
      default:
        this.emit(step.action, actor, null, null, step);
    }
  }

  private completeSet() {
    this.emit('complete', this.lead, null, null, null);
    this.set++;
    this.shape = this.shapeOf(this.set);
    this.owner = new Array(this.shape.bricks).fill(null);
    this.standing = 0;
    this.subTouched.clear();
    // Claims are per set: a working helper takes a fresh submodel in the new set.
    const helpers = [...this.claims.keys()];
    this.claims.clear();
    for (const h of helpers) this.claim(h);
  }
}
