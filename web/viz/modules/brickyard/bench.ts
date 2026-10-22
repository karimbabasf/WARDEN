// bench.ts: one agent's workbench, and the choreography that plays its build.
//
// The planner decides WHICH brick every op is about; the pacer decides WHEN it plays.
// This file only decides how it LOOKS, and it commits the end state of every op the
// pacer reports finished, so the model on the turntable is always exactly the
// planner's bricks once the queue drains, however the animation was cut short.
//
// Layout (bench-local, +z toward the default camera):
//   the lead stands at the left facing +x, the model sits on a turntable in front of
//   it, the parts tray is at its front-left, the book lies open at its front-right,
//   helpers work at side turntables on the right, finished sets go on the shelf behind.

import * as THREE from 'three';
import { BenchPlanner, type BuildOp } from './planner';
import { Pacer } from './pacer';
import type { BuildStep } from './buildLog';
import { benchSet, shapeFor, type BenchSet } from './setModel';
import { hashId, SET_CATALOG, setFor, type SetEntry } from './catalog';
import { makeMinifig, REST_POSE, type Minifig, type MinifigPose } from './minifig';
import { PAGE_H, PAGE_W, renderPage, requestPage } from './pageRenderer';
import { disposeMerged, mergeBricks, ownerOfFace } from './merge';

export type ActorRole = 'lead' | 'helper';
export type ActorStatus = 'working' | 'awaiting' | 'idle' | 'closed' | 'terminated';
export type BenchActor = { id: string; role: ActorRole; status: ActorStatus; name: string };

/** What a click on the bench resolves to. */
export type BrickPick = {
  benchId: string;
  set: SetEntry;
  brick: number;
  placed: BuildStep | null;
  pulled: BuildStep | null;
  actor: string | null;
  /** The set is finished and on the shelf: the panel inspects the whole thing. */
  shelved: boolean;
  setIndex: number;
};

export type ShelfInfo = {
  entry: SetEntry;
  file: string;
  bricks: number;
  /** Bricks placed by each builder, most first. */
  builders: { actor: string; bricks: number }[];
};

export type BenchStats = {
  set: SetEntry;
  setIndex: number;
  bricks: number;
  standing: number;
  placedTotal: number;
  pulledTotal: number;
  page: number;
  steps: number;
  shelf: SetEntry[];
  recent: { actor: string; step: BuildStep; kind: BuildOp['kind'] }[];
  lagMs: number;
};

const TABLE_TOP = 0;
const TT_CENTER = new THREE.Vector3(0.25, 0, -0.2);
const TT_TOP = 0.05;
const LEAD_POS = new THREE.Vector3(-1.05, 0, 0.15);
const TRAY_POS = new THREE.Vector3(-1.6, 0, 0.85);
const BOOK_POS = new THREE.Vector3(-0.35, 0, 1.15);
const REJECT_POS = new THREE.Vector3(-0.45, 0, 0.45);
const HELPER_SLOTS = [
  { stand: new THREE.Vector3(2.75, 0, 0.55), table: new THREE.Vector3(1.95, 0, 0.55) },
  { stand: new THREE.Vector3(2.75, 0, -0.95), table: new THREE.Vector3(1.95, 0, -0.95) },
  { stand: new THREE.Vector3(-1.9, 0, -1.0), table: new THREE.Vector3(-1.15, 0, -1.0) },
];
const SHELF_Z = -2.0;
const SHELF_SCALE = 0.52;
export const BENCH_W = 6.0;
export const BENCH_D = 4.0;
/** How far ahead playback checks that the sets it needs are loaded. */
const FLURRY_LOOKAHEAD = 64;

const smooth = (t: number) => t * t * (3 - 2 * t);
const clamp01 = (t: number) => Math.min(1, Math.max(0, t));
const seg = (p: number, a: number, b: number) => clamp01((p - a) / (b - a));
const mix = (a: number, b: number, t: number) => a + (b - a) * t;
function mixPose(a: MinifigPose, b: MinifigPose, t: number): MinifigPose {
  const out = { ...a };
  for (const k of Object.keys(a) as (keyof MinifigPose)[]) out[k] = mix(a[k], b[k], t);
  return out;
}
/** Shortest signed angle from a to b. */
function angleTo(a: number, b: number) {
  let d = (b - a) % (Math.PI * 2);
  if (d > Math.PI) d -= Math.PI * 2;
  if (d < -Math.PI) d += Math.PI * 2;
  return d;
}

// Poses, relative to the figure's base facing.
const P_TRAY: MinifigPose = { ...REST_POSE, turn: 0.95, lean: 0.25, armR: -0.55, headPitch: 0.35 };
const P_REACH: MinifigPose = { ...REST_POSE, lean: 0.12, armR: -1.25, headPitch: 0.15 };
const P_BOOK: MinifigPose = { ...REST_POSE, turn: -0.55, headYaw: -0.35, headPitch: 0.45, armL: -0.9 };
const P_STUDY: MinifigPose = { ...REST_POSE, turn: -0.35, headYaw: -0.4, headPitch: 0.5 };
const P_ASK: MinifigPose = { ...REST_POSE, turn: -1.1, armR: -2.9, headPitch: -0.1 };
const P_CHEER: MinifigPose = { ...REST_POSE, turn: -0.9, armL: -2.9, armR: -2.9, headPitch: -0.2 };

type Builder = {
  actor: BenchActor;
  fig: Minifig;
  base: THREE.Vector3;
  baseYaw: number;
  pose: MinifigPose;
  /** Helper slot index, -1 for the lead. */
  slot: number;
  /** World position the figure is walking toward (null when it stands). */
  walk: { from: THREE.Vector3; to: THREE.Vector3; t: number; leave: boolean } | null;
  side: SideBuild | null;
  cheerUntil: number;
  /** A helper's own parts tray; the lead uses the bench tray. */
  tray: THREE.Group | null;
};

type SideBuild = { sub: number; holder: THREE.Group; group: THREE.Object3D; parent: THREE.Object3D; index: number };

type LiveSet = {
  entry: SetEntry;
  bs: BenchSet;
  /** The place op that put each brick, for clicks. */
  placedBy: (BuildStep | null)[];
  placedActor: (string | null)[];
  /** Baked runs of finished bricks, one per CHUNK bricks of build order. */
  chunks: (THREE.Group | null)[];
  /** The whole set baked in one piece once it is on the shelf. */
  whole: THREE.Group | null;
  /** Its spot on the shelf, once finished. */
  shelfAt: THREE.Group | null;
};

/** Bricks per baked chunk: small enough to rebake in a frame, big enough to matter. */
const CHUNK = 24;
/** Helpers wear quiet shirts so the lead, in its harness colour, reads first. */
const HELPER_SHIRTS = [0x8a94a8, 0x6f8fa6, 0x9a8f7a, 0x7d8a72];
/** Fewer bricks than any catalog set has (the Landspeeder is the smallest). */
const MIN_SET_BRICKS = 50;

export class BenchRig {
  readonly id: string;
  readonly group = new THREE.Group();
  /** Created in boot, once every set shape is known. */
  private planner!: BenchPlanner;
  private readonly leadId: string;
  private readonly pacer: Pacer;
  private readonly hue: THREE.Color;
  private readonly turntable = new THREE.Group();
  private readonly shelf = new THREE.Group();
  private readonly rejects = new THREE.Group();
  private readonly tray = new THREE.Group();
  private readonly book: Book;
  private readonly alertRing: THREE.Mesh;
  private readonly sideDiscs: THREE.Mesh[] = [];
  private builders = new Map<string, Builder>();
  private sets = new Map<number, LiveSet>();
  private loading = new Map<number, Promise<LiveSet>>();
  private openSet = 0;
  private shapes = new Map<string, { bricks: number; submodels: { start: number; end: number }[] }>();
  private ttAngle = 0;
  private modelView = false;
  private holdBake = 0;
  private ttFrom = 0;
  private ttTo = 0;
  private playingN = -1;
  private placedTotal = 0;
  private pulledTotal = 0;
  private recent: BenchStats['recent'] = [];
  private shelfEntries: SetEntry[] = [];
  private rejectSteps = new Map<THREE.Object3D, { placed: BuildStep | null; pulled: BuildStep; set: number; brick: number }>();
  private page = 0;
  private flipAt = -1;
  private idleT = 0;
  private disposed = false;
  private readonly ready: Promise<void>;
  private onChange: (() => void) | null = null;

  /** Where this bench starts in the catalog, so no two benches build the same set. */
  private readonly offset: number;

  constructor(lead: BenchActor, hue: string, reducedMotion: boolean, offset = 0) {
    this.id = lead.id;
    this.offset = offset;
    this.hue = new THREE.Color(hue);
    this.pacer = new Pacer(reducedMotion);
    this.leadId = lead.id;
    this.book = new Book();
    this.alertRing = new THREE.Mesh(
      new THREE.RingGeometry(0.62, 0.7, 48),
      new THREE.MeshBasicMaterial({ color: 0xff2740, transparent: true, opacity: 0, depthWrite: false }),
    );
    this.buildFurniture();
    this.ready = this.boot(lead);
  }

  /** Called whenever counts or the page change, so the panel can re-read stats. */
  subscribe(cb: (() => void) | null) {
    this.onChange = cb;
  }

  get whenReady(): Promise<void> {
    return this.ready;
  }

  setFor(n: number): SetEntry {
    return setFor(this.id, n, SET_CATALOG, this.offset);
  }

  private shapeSync(n: number) {
    const s = this.shapes.get(this.setFor(n).file);
    if (!s) throw new Error(`brickyard: set ${n} shape requested before it was loaded`);
    return s;
  }

  /** Shapes for this bench's sets 0..upTo, so the planner can open any of them synchronously. */
  private async ensureShapes(upTo: number) {
    for (let n = 0; n <= upTo; n++) {
      const file = this.setFor(n).file;
      if (!this.shapes.has(file)) this.shapes.set(file, await shapeFor(file));
    }
  }

  private async boot(lead: BenchActor) {
    await this.ensureShapes(1);
    this.planner = new BenchPlanner(this.leadId, (n) => this.shapeSync(n));
    await this.addBuilder(lead);
    await this.loadSet(0);
    this.placeSet(0);
    this.refreshTray();
    await this.turnTo(0, true);
  }

  private buildFurniture() {
    const g = this.group;
    const wood = new THREE.MeshStandardMaterial({ color: 0x252c3a, roughness: 0.82, metalness: 0.05 });
    const edge = new THREE.MeshStandardMaterial({ color: 0x2a3242, roughness: 0.7 });
    const table = new THREE.Mesh(new THREE.BoxGeometry(BENCH_W, 0.14, BENCH_D), wood);
    table.position.set(0.45, TABLE_TOP - 0.07, -0.2);
    table.receiveShadow = true;
    g.add(table);

    // Turntable: a dark disc with a thin lit rim, so the model reads as "on display".
    const tt = new THREE.Mesh(new THREE.CylinderGeometry(1.0, 1.04, TT_TOP, 64), edge);
    tt.position.y = TT_TOP / 2;
    tt.receiveShadow = true;
    const rim = new THREE.Mesh(
      new THREE.TorusGeometry(1.02, 0.008, 6, 96),
      new THREE.MeshBasicMaterial({ color: this.hue }),
    );
    rim.rotation.x = Math.PI / 2;
    rim.position.y = TT_TOP;
    const base = new THREE.Group();
    base.position.copy(TT_CENTER);
    base.add(tt, rim);
    this.turntable.position.set(0, TT_TOP, 0);
    base.add(this.turntable);
    this.alertRing.rotation.x = -Math.PI / 2;
    this.alertRing.position.set(0, TT_TOP + 0.003, 0);
    this.alertRing.scale.setScalar(1.6);
    base.add(this.alertRing);
    g.add(base);

    // Parts tray.
    const trayBox = new THREE.Mesh(new THREE.BoxGeometry(0.8, 0.06, 0.55), edge);
    trayBox.position.set(0, 0.03, 0);
    trayBox.receiveShadow = true;
    this.tray.position.copy(TRAY_POS);
    this.tray.add(trayBox);
    g.add(this.tray);

    this.book.group.position.copy(BOOK_POS);
    // Turned toward the default camera so the pages read from where you look.
    this.book.group.rotation.y = 0.45;
    g.add(this.book.group);

    // Shelf.
    const shelf = new THREE.Mesh(new THREE.BoxGeometry(BENCH_W - 0.4, 0.05, 0.6), edge);
    shelf.position.set(0, 0.025, 0);
    shelf.receiveShadow = true;
    this.shelf.position.set(0.45, 0, SHELF_Z);
    this.shelf.add(shelf);
    g.add(this.shelf);

    this.rejects.position.copy(REJECT_POS);
    g.add(this.rejects);

    // A pool of light on the table: the bench reads as lit, the void stays dark.
    const pool = new THREE.Mesh(new THREE.CircleGeometry(2.6, 48), new THREE.MeshBasicMaterial({ map: poolTexture(), transparent: true, depthWrite: false }));
    pool.rotation.x = -Math.PI / 2;
    pool.position.set(TT_CENTER.x - 0.3, 0.002, TT_CENTER.z + 0.3);
    pool.renderOrder = -1;
    g.add(pool);

    // Side turntables for helpers, shown while someone works at them.
    for (const slot of HELPER_SLOTS) {
      const disc = new THREE.Mesh(new THREE.CylinderGeometry(0.55, 0.58, 0.04, 40), edge);
      disc.position.copy(slot.table).setY(0.02);
      disc.receiveShadow = true;
      disc.visible = false;
      this.sideDiscs.push(disc);
      g.add(disc);
    }

  }

  private async addBuilder(actor: BenchActor): Promise<Builder> {
    const existing = this.builders.get(actor.id);
    if (existing) return existing;
    const lead = actor.role === 'lead';
    const seed = hashId(actor.id);
    const fig = await makeMinifig({
      torso: lead ? this.hue : HELPER_SHIRTS[seed % HELPER_SHIRTS.length],
      legs: lead ? 0x2b3140 : 0x3d4556,
      helmet: lead,
      seed,
    });
    const used = new Set([...this.builders.values()].map((b) => b.slot));
    const slot = lead ? -1 : Math.max(0, HELPER_SLOTS.findIndex((_, i) => !used.has(i)));
    const base = lead ? LEAD_POS.clone() : HELPER_SLOTS[slot % HELPER_SLOTS.length].stand.clone();
    const facing = lead ? TT_CENTER : HELPER_SLOTS[slot % HELPER_SLOTS.length].table;
    const baseYaw = Math.atan2(facing.x - base.x, facing.z - base.z);
    const b: Builder = { actor, fig, base, baseYaw, pose: { ...REST_POSE }, slot, walk: null, side: null, cheerUntil: 0, tray: null };
    fig.root.position.copy(base);
    fig.root.rotation.y = baseYaw;
    if (!lead) {
      this.sideDiscs[slot % HELPER_SLOTS.length].visible = true;
      // The helper's tray sits at its front-left, where a turn and a reach find it.
      const tray = new THREE.Group();
      const box = new THREE.Mesh(new THREE.BoxGeometry(0.62, 0.05, 0.42), TRAY_MAT);
      box.position.y = 0.025;
      box.receiveShadow = true;
      tray.add(box);
      tray.position.copy(base).add(new THREE.Vector3(-0.3, 0, 0.66));
      this.group.add(tray);
      b.tray = tray;
      this.fillHelperTray(b);
    }
    if (!lead) {
      // Helpers walk in from the near edge of the bench.
      const from = base.clone().add(new THREE.Vector3(0.8, 0, 1.6));
      fig.root.position.copy(from);
      b.walk = { from, to: base.clone(), t: 0, leave: false };
    }
    this.group.add(fig.root);
    this.builders.set(actor.id, b);
    return b;
  }

  private async loadSet(n: number): Promise<LiveSet> {
    const have = this.sets.get(n);
    if (have) return have;
    let p = this.loading.get(n);
    if (!p) {
      p = (async () => {
        const entry = this.setFor(n);
        const bs = await benchSet(entry.file);
        for (const b of bs.bricks) {
          b.userData.home = b.position.clone();
          b.visible = false;
        }
        const live: LiveSet = {
          entry,
          bs,
          placedBy: new Array(bs.bricks.length).fill(null),
          placedActor: new Array(bs.bricks.length).fill(null),
          chunks: new Array(Math.ceil(bs.bricks.length / CHUNK)).fill(null),
          whole: null,
          shelfAt: null,
        };
        this.sets.set(n, live);
        return live;
      })();
      this.loading.set(n, p);
    }
    return p;
  }

  private placeSet(n: number) {
    const live = this.sets.get(n);
    if (!live) return;
    this.turntable.clear();
    this.turntable.add(live.bs.holder);
    this.openSet = n;
    void this.showPage(0);
  }

  /** Bring the actors at this bench up to date: helpers arrive, finish and leave. */
  async setActors(actors: BenchActor[]) {
    await this.ready;
    for (const a of actors) {
      const b = this.builders.get(a.id);
      if (b) b.actor = a;
      else if (a.role === 'helper' && a.status !== 'closed' && a.status !== 'terminated') await this.addBuilder(a);
      if (a.role === 'helper' && (a.status === 'closed' || a.status === 'terminated')) this.pacer.push(this.planner.finish(a.id));
    }
    const present = new Set(actors.map((a) => a.id));
    for (const [id, b] of this.builders) {
      if (b.actor.role === 'helper' && !present.has(id)) this.pacer.push(this.planner.finish(id));
    }
    const lead = actors.find((a) => a.role === 'lead');
    const lb = lead ? this.builders.get(lead.id) : null;
    if (lb && (lead?.status === 'closed' || lead?.status === 'terminated') && lb.cheerUntil === 0) {
      lb.cheerUntil = performance.now() + 2600;
    }
  }

  /** Fold new log entries in. Each actor's steps must be in seq order. */
  async ingest(batch: { actor: string; steps: BuildStep[] }[]) {
    await this.ready;
    // Enough shapes for the worst case: every placement in the batch, on the smallest set.
    let places = 0;
    for (const b of batch) for (const st of b.steps) if (st.action === 'place') places++;
    await this.ensureShapes(this.planner.currentSet + Math.ceil(places / MIN_SET_BRICKS) + 1);
    const ops = this.planner.ingest(batch);
    if (ops.length === 0) return;
    // Start loading every set these ops touch, plus the one after, before they play.
    const touched = new Set(ops.map((o) => o.set));
    for (const n of touched) {
      void this.loadSet(n);
      void this.loadSet(n + 1);
    }
    for (const op of ops) {
      if (op.step) {
        this.recent.push({ actor: op.actor, step: op.step, kind: op.kind });
        if (op.kind === 'place') this.placedTotal++;
        if (op.kind === 'pull') this.pulledTotal++;
      }
    }
    if (this.recent.length > 60) this.recent = this.recent.slice(-60);
    this.pacer.push(ops);
    this.onChange?.();
  }

  lastSeq(actor: string): number | null {
    return this.planner ? this.planner.lastSeq(actor) : null;
  }

  stats(): BenchStats | null {
    const live = this.sets.get(this.openSet);
    if (!live) return null;
    const standing = live.placedBy.filter((s) => s !== null).length;
    return {
      set: live.entry,
      setIndex: this.openSet,
      bricks: live.bs.bricks.length,
      standing,
      placedTotal: this.placedTotal,
      pulledTotal: this.pulledTotal,
      page: this.page,
      steps: live.bs.steps,
      shelf: this.shelfEntries.slice(),
      recent: this.recent.slice(-30).reverse(),
      lagMs: this.pacer.backlogMs(),
    };
  }

  /** The page canvas currently open on the right of the book. */
  pageCanvas(): HTMLCanvasElement | null {
    return this.book.rightCanvas;
  }

  // ── op playback ────────────────────────────────────────────────────────────

  private brickWorld(live: LiveSet, brick: number, out: THREE.Vector3): THREE.Vector3 {
    const b = live.bs.bricks[brick];
    const parent = b.parent as THREE.Object3D;
    parent.updateWorldMatrix(true, false);
    return parent.localToWorld(out.copy(b.userData.home as THREE.Vector3));
  }

  /** Turntable angle that brings a brick round to face the lead. */
  private faceAngle(live: LiveSet, brick: number): number {
    const b = live.bs.bricks[brick];
    const p = (b.userData.home as THREE.Vector3).clone();
    const parent = b.parent as THREE.Object3D;
    // In the turntable's unrotated frame.
    const m = new THREE.Matrix4();
    let o: THREE.Object3D | null = parent;
    const chain: THREE.Object3D[] = [];
    while (o && o !== this.turntable) {
      chain.unshift(o);
      o = o.parent;
    }
    // A brick on a helper's side table is not on the turntable: leave it be.
    if (!o) return this.ttAngle;
    for (const c of chain) c.updateMatrix();
    m.identity();
    for (const c of chain) m.multiply(c.matrix);
    p.applyMatrix4(m);
    const toLead = Math.atan2(LEAD_POS.x - TT_CENTER.x, LEAD_POS.z - TT_CENTER.z);
    return toLead - Math.atan2(p.x, p.z);
  }

  private async turnTo(angle: number, instant = false) {
    this.ttFrom = this.ttAngle;
    this.ttTo = this.ttAngle + angleTo(this.ttAngle, angle);
    if (instant) {
      this.ttAngle = this.ttTo;
      this.turntable.rotation.y = this.ttAngle;
    }
  }

  private startOp(op: BuildOp) {
    const live = this.sets.get(op.set);
    if (op.kind === 'place' && live && op.brick !== null) {
      const helper = this.builders.get(op.actor);
      if (!helper || helper.actor.role === 'lead' || !helper.side) void this.turnTo(this.faceAngle(live, op.brick));
      else void this.turnTo(this.ttAngle);
      const step = live.bs.stepOf[op.brick];
      if (step !== this.page) void this.showPage(step);
    } else if (op.kind === 'pull' && live && op.brick !== null) {
      this.unbake(live, Math.floor(op.brick / CHUNK));
      void this.turnTo(this.faceAngle(live, op.brick));
    } else if (op.kind === 'claim' && op.sub !== null) {
      this.claimSide(op);
    } else if (op.kind === 'page') {
      this.flipAt = 0;
    } else {
      void this.turnTo(this.ttAngle);
    }
  }

  private commitOp(op: BuildOp) {
    const live = this.sets.get(op.set);
    if (op.kind === 'place' && live && op.brick !== null) {
      const b = live.bs.bricks[op.brick];
      b.position.copy(b.userData.home as THREE.Vector3);
      b.scale.setScalar(1);
      b.visible = true;
      live.placedBy[op.brick] = op.step;
      live.placedActor[op.brick] = op.actor;
    } else if (op.kind === 'pull' && live && op.brick !== null) {
      this.unbake(live, Math.floor(op.brick / CHUNK));
      const b = live.bs.bricks[op.brick];
      const placed = live.placedBy[op.brick];
      b.position.copy(b.userData.home as THREE.Vector3);
      b.visible = false;
      live.placedBy[op.brick] = null;
      live.placedActor[op.brick] = null;
      this.dropReject(live, op, placed);
    } else if (op.kind === 'attach' && op.sub !== null) {
      this.attachSide(op);
    } else if (op.kind === 'complete') {
      this.completeSet(op.set);
    }
    this.onChange?.();
  }

  private dropReject(live: LiveSet, op: BuildOp, placed: BuildStep | null) {
    if (op.brick === null || !op.step) return;
    const src = live.bs.bricks[op.brick];
    const clone = src.clone(true);
    clone.visible = true;
    // Keep the world scale of the model.
    const s = new THREE.Vector3();
    src.parent?.getWorldScale(s);
    clone.scale.copy(s);
    clone.rotation.set(Math.PI + (Math.random() - 0.5) * 0.6, Math.random() * Math.PI * 2, (Math.random() - 0.5) * 0.4);
    const n = this.rejects.children.length;
    clone.position.set(((n % 5) - 2) * 0.09, 0.02 + Math.floor(n / 5) * 0.03, ((n * 7) % 3) * 0.08);
    this.rejects.add(clone);
    this.rejectSteps.set(clone, { placed, pulled: op.step, set: op.set, brick: op.brick });
    while (this.rejects.children.length > 30) {
      const old = this.rejects.children[0];
      this.rejects.remove(old);
      this.rejectSteps.delete(old);
    }
  }

  private claimSide(op: BuildOp) {
    const b = this.builders.get(op.actor);
    const live = this.sets.get(op.set);
    if (!b || !live || op.sub === null || b.slot < 0) return;
    const range = live.bs.shape.submodels[op.sub];
    const first = live.bs.bricks[range.start];
    // The submodel's own group: the ancestor of its bricks just below the model level.
    let group: THREE.Object3D = first;
    while (group.parent && group.parent !== live.bs.model) {
      const owned = new Set<THREE.Object3D>();
      group.parent.traverse((o) => owned.add(o));
      if (live.bs.bricks.slice(range.start, range.end).every((x) => owned.has(x))) {
        // Stop at the highest ancestor that holds this submodel and nothing else.
        const outside = live.bs.bricks.some((x, i) => (i < range.start || i >= range.end) && owned.has(x));
        if (outside) break;
      } else break;
      group = group.parent;
    }
    const parent = group.parent as THREE.Object3D;
    const index = parent.children.indexOf(group);
    const holder = new THREE.Group();
    this.group.add(holder);
    // Same world transform as the model, slid so the submodel lands on the side table.
    this.group.updateMatrixWorld(true);
    const centre = new THREE.Box3();
    for (let i = range.start; i < range.end; i++) {
      const w = this.brickWorld(live, i, new THREE.Vector3());
      centre.expandByPoint(w);
    }
    const c = centre.getCenter(new THREE.Vector3());
    const slot = HELPER_SLOTS[b.slot % HELPER_SLOTS.length].table;
    const target = this.group.localToWorld(slot.clone());
    const shift = new THREE.Matrix4().makeTranslation(target.x - c.x, target.y - centre.min.y + TT_TOP, target.z - c.z);
    const world = shift.multiply(parent.matrixWorld.clone());
    const local = new THREE.Matrix4().copy(this.group.matrixWorld).invert().multiply(world);
    local.decompose(holder.position, holder.quaternion, holder.scale);
    holder.add(group);
    b.side = { sub: op.sub, holder, group, parent, index };
  }

  private attachSide(op: BuildOp) {
    const b = this.builders.get(op.actor);
    if (!b?.side) return;
    const { group, parent, index, holder } = b.side;
    parent.add(group);
    // Back at its own place in the model's child order.
    parent.children.splice(parent.children.indexOf(group), 1);
    parent.children.splice(Math.min(index, parent.children.length), 0, group);
    this.group.remove(holder);
    b.side = null;
    b.walk = { from: b.fig.root.position.clone(), to: b.base.clone().add(new THREE.Vector3(1.2, 0, 2.2)), t: 0, leave: true };
  }

  private completeSet(n: number) {
    const live = this.sets.get(n);
    if (!live) return;
    // The finished model goes to the shelf, small, in a row behind the bench.
    // On the shelf the set never changes again: bake it whole.
    for (let c = 0; c < live.chunks.length; c++) this.unbake(live, c);
    const all = live.bs.bricks.map((obj, index) => ({ obj, index })).filter(({ index }) => live.placedBy[index] !== null);
    for (const { obj } of all) obj.position.copy(obj.userData.home as THREE.Vector3);
    live.whole = mergeBricks(all, live.bs.model);
    live.bs.model.add(live.whole);
    for (const { obj } of all) obj.visible = false;
    const holder = live.bs.holder;
    const slot = this.shelfEntries.length;
    this.shelfEntries.push(live.entry);
    holder.removeFromParent();
    const keep = new THREE.Group();
    keep.add(holder);
    keep.scale.setScalar(SHELF_SCALE);
    keep.position.set(-2.2 + (slot % 5) * 1.1, 0.05, 0);
    // Five fit on the shelf; the oldest makes room.
    if (this.shelf.children.length > 5) this.shelf.remove(this.shelf.children[1]);
    keep.rotation.y = -0.4;
    this.shelf.add(keep);
    live.shelfAt = keep;
    void this.loadSet(n + 1).then(() => {
      this.placeSet(n + 1);
      this.refreshTray();
    });
  }

  /** A handful of the open set's own small parts, baked into a couple of draw calls. */
  private makePile(live: LiveSet, seed: number, count: number): THREE.Group {
    const s = new THREE.Vector3();
    live.bs.holder.updateMatrixWorld(true);
    live.bs.model.getWorldScale(s);
    // Only small parts go in a tray: a baseplate in a parts pile reads as a bug.
    const small = live.bs.bricks.filter((b) => {
      const size = new THREE.Box3().setFromObject(b).getSize(new THREE.Vector3());
      return Math.max(size.x, size.y, size.z) < 0.16;
    });
    const pile = new THREE.Group();
    const picks = Math.min(count, small.length);
    for (let i = 0; i < picks; i++) {
      const src = small[Math.floor(((i + seed) * 7919) % small.length)];
      const c = src.clone(true);
      c.traverse((o) => {
        o.visible = true;
      });
      c.scale.set(s.x, s.y, s.z);
      c.position.set(((i % 5) - 2) * 0.13, 0.07, (Math.floor(i / 5) - 1) * 0.14);
      c.rotation.set(Math.PI + (((i + seed) * 37) % 7) * 0.1, ((i + seed) * 1.3) % (Math.PI * 2), 0);
      pile.add(c);
    }
    const baked = mergeBricks(pile.children.map((obj, index) => ({ obj, index })), pile);
    baked.traverse((o) => {
      o.raycast = () => {};
    });
    return baked;
  }

  /** The lead's tray, refilled from the open set. */
  private refreshTray() {
    const live = this.sets.get(this.openSet);
    if (!live) return;
    for (const c of this.tray.children.slice(1)) {
      this.tray.remove(c);
      disposeMerged(c);
    }
    this.tray.add(this.makePile(live, 0, 16));
    for (const b of this.builders.values()) if (b.tray) this.fillHelperTray(b);
  }

  /** Every helper works from its own tray, at its own side table. */
  private fillHelperTray(b: Builder) {
    const live = this.sets.get(this.openSet);
    if (!live || !b.tray) return;
    for (const c of b.tray.children.slice(1)) {
      b.tray.remove(c);
      disposeMerged(c);
    }
    b.tray.add(this.makePile(live, 1 + (hashId(b.actor.id) % 97), 10));
  }

  private async showPage(step: number) {
    const live = this.sets.get(this.openSet);
    if (!live) return;
    const steps = live.bs.steps;
    const target = Math.min(step, steps);
    const forward = target > this.page;
    this.page = target;
    const label = `${live.entry.number} ${live.entry.name}`;
    // Coalesced: a flurry that turns past this page drops it before it renders.
    const right = await requestPage(this.id, live.entry.file, target, steps, label);
    if (!right || this.disposed || this.page !== target) return;
    const left = target > 0 ? await renderPage(live.entry.file, target - 1, steps, label) : null;
    if (this.disposed || this.page !== target) return;
    this.book.turn(left, right, forward);
    this.onChange?.();
  }

  /** Bake at most one finished chunk per frame, so a long build never stalls a frame. */
  private bakeOne(playing: BuildOp | null) {
    if (this.modelView || performance.now() < this.holdBake) return;
    const live = this.sets.get(this.openSet);
    if (!live) return;
    const busy = playing && playing.set === this.openSet && playing.brick !== null ? Math.floor(playing.brick / CHUNK) : -1;
    for (let c = 0; c < live.chunks.length; c++) {
      if (live.chunks[c] || c === busy) continue;
      const lo = c * CHUNK;
      const hi = Math.min(lo + CHUNK, live.bs.bricks.length);
      let ready = true;
      for (let i = lo; i < hi && ready; i++) {
        // Placed, and on the main model (a helper's side build bakes once it attaches).
        ready = live.placedBy[i] !== null && isUnder(live.bs.bricks[i], live.bs.model);
      }
      if (!ready) continue;
      const run = [];
      for (let i = lo; i < hi; i++) {
        const obj = live.bs.bricks[i];
        obj.position.copy(obj.userData.home as THREE.Vector3);
        obj.scale.setScalar(1);
        run.push({ obj, index: i });
      }
      const g = mergeBricks(run, live.bs.model);
      live.bs.model.add(g);
      for (const { obj } of run) obj.visible = false;
      live.chunks[c] = g;
      return;
    }
  }

  /** Undo a baked chunk: its bricks stand on their own again. */
  private unbake(live: LiveSet, c: number) {
    const g = live.chunks[c];
    if (!g) return;
    disposeMerged(g);
    live.chunks[c] = null;
    const lo = c * CHUNK;
    const hi = Math.min(lo + CHUNK, live.bs.bricks.length);
    for (let i = lo; i < hi; i++) live.bs.bricks[i].visible = live.placedBy[i] !== null;
  }

  // ── per frame ───────────────────────────────────────────────────────────────

  update(dtMs: number, now: number) {
    if (this.disposed) return;
    // Hold the queue while a set the next ops need is still loading, so no op is
    // ever committed against a model that does not exist yet.
    for (const op of this.pacer.lookahead(FLURRY_LOOKAHEAD)) {
      if (!this.sets.has(op.set)) {
        void this.loadSet(op.set);
        return;
      }
      if (op.kind === 'complete' && !this.sets.has(op.set + 1)) {
        void this.loadSet(op.set + 1);
        return;
      }
    }
    const finished = this.pacer.tick(dtMs);
    for (const op of finished) {
      if (op.n > this.playingN) this.startOp(op);
      this.playingN = Math.max(this.playingN, op.n);
      this.commitOp(op);
    }
    const playing = this.pacer.playing;
    if (playing && playing.op.n > this.playingN) {
      this.playingN = playing.op.n;
      this.startOp(playing.op);
    }
    const p = this.pacer.progress() ?? 0;

    // Turntable eases to its target during the first part of an op.
    const ttT = playing ? smooth(seg(p, 0, 0.5)) : 1;
    this.ttAngle = mix(this.ttFrom, this.ttTo, ttT);
    this.turntable.rotation.y = this.ttAngle;

    const t = now / 1000;
    const awaiting = [...this.builders.values()].some((b) => b.actor.status === 'awaiting');
    (this.alertRing.material as THREE.MeshBasicMaterial).opacity = awaiting
      ? 0.25 + 0.45 * (0.5 + 0.5 * Math.sin((t * Math.PI * 2) / 1.55))
      : 0;

    for (const b of this.builders.values()) this.animateBuilder(b, playing?.op ?? null, p, dtMs, now);
    this.animateBrick(playing?.op ?? null, p);
    this.bakeOne(playing?.op ?? null);
    this.book.update(dtMs);
    this.idleT += dtMs;
    if (this.pacer.idle && this.idleT > 6500) {
      this.idleT = 0;
      this.book.riffle();
    }
    if (this.flipAt >= 0) {
      this.flipAt = -1;
      this.book.riffle(true);
    }
  }

  private animateBrick(op: BuildOp | null, p: number) {
    if (!op || op.brick === null) return;
    const live = this.sets.get(op.set);
    if (!live) return;
    const brick = live.bs.bricks[op.brick];
    const parent = brick.parent as THREE.Object3D;
    const b = this.builders.get(op.actor);
    if (!b) return;
    const home = brick.userData.home as THREE.Vector3;
    const hand = b.fig.handR.getWorldPosition(new THREE.Vector3());
    parent.updateWorldMatrix(true, false);
    const handLocal = parent.worldToLocal(hand.clone());
    if (op.kind === 'place') {
      if (p < 0.22) {
        brick.visible = false;
        return;
      }
      brick.visible = true;
      if (p < 0.58) {
        brick.position.copy(handLocal);
        brick.scale.setScalar(1);
        return;
      }
      const f = smooth(seg(p, 0.58, 0.86));
      brick.position.lerpVectors(handLocal, home, f);
      // Lift over the top on the way in, so a brick never cuts through the model.
      const lift = Math.sin(f * Math.PI) * 30;
      brick.position.y -= lift;
      const pop = p > 0.86 ? 1 + Math.sin(seg(p, 0.86, 1) * Math.PI) * 0.12 : 1;
      brick.scale.setScalar(pop);
    } else if (op.kind === 'pull') {
      brick.visible = true;
      if (p < 0.4) {
        brick.position.copy(home);
        return;
      }
      if (p < 0.7) {
        brick.position.lerpVectors(home, handLocal, smooth(seg(p, 0.4, 0.7)));
        return;
      }
      const reject = this.rejects.getWorldPosition(new THREE.Vector3());
      const rl = parent.worldToLocal(reject);
      const f = seg(p, 0.7, 1);
      brick.position.lerpVectors(handLocal, rl, f);
      brick.position.y -= Math.sin(f * Math.PI) * 40;
    }
  }

  private animateBuilder(b: Builder, op: BuildOp | null, p: number, dtMs: number, now: number) {
    const t = now / 1000;
    let target: MinifigPose = { ...REST_POSE };
    const mine = op && op.actor === b.actor.id;

    if (b.walk) {
      b.walk.t = Math.min(1, b.walk.t + dtMs / 1600);
      const f = smooth(b.walk.t);
      b.fig.root.position.lerpVectors(b.walk.from, b.walk.to, f);
      const dir = b.walk.to.clone().sub(b.walk.from);
      if (b.walk.t < 1) b.fig.root.rotation.y = Math.atan2(dir.x, dir.z);
      const phase = t * 9;
      target = { ...REST_POSE, legL: Math.sin(phase) * 0.6, legR: -Math.sin(phase) * 0.6, armL: -Math.sin(phase) * 0.45, armR: Math.sin(phase) * 0.45, bob: Math.abs(Math.cos(phase)) * 0.02 };
      if (b.walk.t >= 1) {
        const leaving = b.walk.leave;
        b.walk = null;
        b.fig.root.rotation.y = b.baseYaw;
        if (leaving) {
          if (b.slot >= 0) this.sideDiscs[b.slot % HELPER_SLOTS.length].visible = false;
          if (b.tray) {
            b.tray.traverse((o) => (o as THREE.Mesh).geometry?.dispose());
            this.group.remove(b.tray);
          }
          this.group.remove(b.fig.root);
          this.builders.delete(b.actor.id);
          return;
        }
      }
      b.fig.apply(target);
      return;
    }

    if (b.cheerUntil > now) {
      const j = Math.abs(Math.sin(t * 7));
      target = { ...P_CHEER, bob: j * 0.08, armL: P_CHEER.armL + Math.sin(t * 7) * 0.25, armR: P_CHEER.armR - Math.sin(t * 7) * 0.25 };
    } else if (mine && op) {
      switch (op.kind) {
        case 'place': {
          if (p < 0.25) target = mixPose(REST_POSE, P_TRAY, smooth(seg(p, 0, 0.18)));
          else if (p < 0.6) target = mixPose(P_TRAY, P_REACH, smooth(seg(p, 0.25, 0.55)));
          else target = mixPose(P_REACH, REST_POSE, smooth(seg(p, 0.82, 1)));
          break;
        }
        case 'pull': {
          if (p < 0.7) target = mixPose(REST_POSE, P_REACH, smooth(seg(p, 0, 0.4)));
          else target = { ...mixPose(P_REACH, REST_POSE, smooth(seg(p, 0.7, 1))), headYaw: Math.sin(seg(p, 0.7, 1) * Math.PI * 4) * 0.5 };
          break;
        }
        case 'browse': {
          const read = /read|fetch|notebook/i.test(op.step?.tool ?? '');
          target = read
            ? mixPose(REST_POSE, P_BOOK, Math.sin(p * Math.PI))
            : mixPose(REST_POSE, { ...P_TRAY, armR: -0.4 + Math.sin(p * Math.PI * 4) * 0.25 }, Math.sin(p * Math.PI));
          break;
        }
        case 'page':
          target = mixPose(REST_POSE, P_BOOK, Math.sin(p * Math.PI));
          break;
        case 'spawn':
          target = { ...REST_POSE, turn: -1.4, armL: -2.6 + Math.sin(p * Math.PI * 4) * 0.3, headYaw: -0.3 };
          break;
        case 'ask':
          target = mixPose(REST_POSE, P_ASK, Math.sin(Math.min(p, 0.5) * Math.PI));
          break;
        case 'claim':
        case 'attach':
          target = { ...REST_POSE, armL: -1.1, armR: -1.1 };
          break;
        default:
          break;
      }
    } else if (b.actor.status === 'awaiting') {
      target = { ...P_ASK, armR: P_ASK.armR + Math.sin(t * 2) * 0.08 };
    } else if (b.actor.status === 'working') {
      // Between calls: studying the book.
      target = { ...P_STUDY, headPitch: P_STUDY.headPitch + Math.sin(t * 0.9) * 0.08 };
    } else {
      target = { ...REST_POSE, headYaw: Math.sin(t * 0.35) * 0.5, bob: Math.sin(t * 1.6) * 0.004 };
    }
    // Ease toward the target so op boundaries never pop.
    const k = 1 - Math.exp(-dtMs / 70);
    b.pose = mixPose(b.pose, target, k);
    b.fig.apply(b.pose);
  }

  // ── picking ────────────────────────────────────────────────────────────────

  pick(object: THREE.Object3D, faceIndex?: number): BrickPick | null {
    // A baked chunk answers through its vertex map.
    const owner = ownerOfFace(object, faceIndex);
    if (owner !== null) {
      for (const [n, live] of this.sets) {
        if (live.chunks.includes(object.parent as THREE.Group) || live.whole === object.parent) {
          return this.pickOf(n, live, owner);
        }
      }
    }
    let o: THREE.Object3D | null = object;
    while (o) {
      const rej = this.rejectSteps.get(o);
      if (rej) {
        const entry = this.setFor(rej.set);
        return { benchId: this.id, set: entry, brick: rej.brick, placed: rej.placed, pulled: rej.pulled, actor: null, shelved: false, setIndex: rej.set };
      }
      for (const [n, live] of this.sets) {
        const i = live.bs.bricks.indexOf(o);
        if (i !== -1) return this.pickOf(n, live, i);
      }
      o = o.parent;
    }
    return null;
  }

  /** The brick a log entry placed, wherever it now stands (open set or shelf). */
  brickForStep(actor: string, seq: number): BrickPick | null {
    for (const [n, live] of this.sets) {
      const i = live.placedBy.findIndex((s, k) => s !== null && s.seq === seq && live.placedActor[k] === actor);
      if (i !== -1) return this.pickOf(n, live, i);
    }
    return null;
  }

  private pickOf(n: number, live: LiveSet, brick: number): BrickPick {
    return {
      benchId: this.id,
      set: live.entry,
      brick,
      placed: live.placedBy[brick],
      pulled: null,
      actor: live.placedActor[brick],
      shelved: live.whole !== null,
      setIndex: n,
    };
  }

  /** A finished set: what it is and who built it. */
  shelfInfo(n: number): ShelfInfo | null {
    const live = this.sets.get(n);
    if (!live?.whole) return null;
    const by = new Map<string, number>();
    for (const a of live.placedActor) if (a) by.set(a, (by.get(a) ?? 0) + 1);
    return {
      entry: live.entry,
      file: live.entry.file,
      bricks: live.bs.bricks.length,
      builders: [...by].map(([actor, bricks]) => ({ actor, bricks })).sort((x, y) => y.bricks - x.bricks),
    };
  }

  /** Was this hit on the instruction book? */
  ownsBook(o: THREE.Object3D | null): boolean {
    for (; o; o = o.parent) if (o === this.book.group) return true;
    return false;
  }

  /** A camera square on to the open spread, close enough to read it. */
  bookView(): { target: THREE.Vector3; position: THREE.Vector3 } {
    const face = this.book.face;
    face.updateWorldMatrix(true, false);
    const target = face.getWorldPosition(new THREE.Vector3());
    const normal = new THREE.Vector3(0, 1, 0).applyQuaternion(face.getWorldQuaternion(new THREE.Quaternion()));
    return { target, position: target.clone().addScaledVector(normal, 1.55) };
  }

  /** World-space centre of a shelved set, for the camera. Null once it left the shelf. */
  shelfFocus(n: number, out = new THREE.Vector3()): THREE.Vector3 | null {
    const at = this.sets.get(n)?.shelfAt;
    if (!at?.parent) return null;
    at.updateWorldMatrix(true, true);
    const box = new THREE.Box3().setFromObject(at);
    return box.isEmpty() ? at.getWorldPosition(out) : box.getCenter(out);
  }

  /** Briefly lifts a brick so a click in the panel can show where it is. */
  highlight(brick: number) {
    const live = this.sets.get(this.openSet);
    if (!live) return;
    this.unbake(live, Math.floor(brick / CHUNK));
    this.holdBake = performance.now() + 1200;
    const b = live.bs.bricks[brick];
    if (!b || !b.visible) return;
    const home = b.userData.home as THREE.Vector3;
    const start = performance.now();
    const tick = () => {
      const f = (performance.now() - start) / 900;
      if (f >= 1 || this.disposed) {
        b.position.copy(home);
        return;
      }
      b.position.copy(home);
      b.position.y -= Math.sin(f * Math.PI) * 24;
      requestAnimationFrame(tick);
    };
    tick();
  }

  /** Ghost the bricks still to come, for the model view. */
  setGhost(on: boolean) {
    const live = this.sets.get(this.openSet);
    if (!live) return;
    // Model view edits bricks one by one, so nothing stays baked while it is open.
    this.modelView = on;
    if (on) for (let c = 0; c < live.chunks.length; c++) this.unbake(live, c);
    live.bs.bricks.forEach((b, i) => {
      if (live.placedBy[i] !== null) return;
      b.visible = on;
      b.traverse((o) => {
        const m = o as THREE.Mesh;
        if (m.isMesh) {
          if (on) {
            if (!m.userData.solid) m.userData.solid = m.material;
            m.material = GHOST;
          } else if (m.userData.solid) {
            m.material = m.userData.solid;
          }
        } else if ((o as THREE.LineSegments).isLineSegments) {
          o.visible = !on;
        }
      });
      if (!on) b.traverse((o) => ((o as THREE.LineSegments).isLineSegments ? (o.visible = true) : null));
    });
  }

  /** Model-view scrub: show only the first `count` placements of the open set. */
  scrub(count: number | null) {
    const live = this.sets.get(this.openSet);
    if (!live) return;
    const order = live.placedBy
      .map((s, i) => ({ s, i }))
      .filter((x) => x.s !== null)
      .sort((a, b) => (a.s as BuildStep).seq - (b.s as BuildStep).seq);
    const shown = new Set(order.slice(0, count ?? order.length).map((x) => x.i));
    live.bs.bricks.forEach((b, i) => {
      if (live.placedBy[i] !== null) b.visible = shown.has(i);
    });
  }

  /** What the panel preview needs: the open set and which of its bricks stand. */
  previewState(): { file: string; placed: boolean[] } | null {
    const live = this.sets.get(this.openSet);
    if (!live) return null;
    return { file: live.entry.file, placed: live.placedBy.map((s) => s !== null) };
  }

  placedCount(): number {
    const live = this.sets.get(this.openSet);
    return live ? live.placedBy.filter((s) => s !== null).length : 0;
  }

  /** World-space centre of the open model, for the camera. */
  focusPoint(out = new THREE.Vector3()): THREE.Vector3 {
    return this.group.localToWorld(out.copy(TT_CENTER).setY(0.5));
  }

  dispose() {
    this.disposed = true;
    this.group.removeFromParent();
    this.book.dispose();
  }
}

function isUnder(o: THREE.Object3D, root: THREE.Object3D): boolean {
  for (let p: THREE.Object3D | null = o.parent; p; p = p.parent) if (p === root) return true;
  return false;
}

let poolTex: THREE.Texture | null = null;
/** A soft warm radial falloff, drawn once and shared by every bench. */
function poolTexture(): THREE.Texture {
  if (poolTex) return poolTex;
  const c = document.createElement('canvas');
  c.width = c.height = 256;
  const g = c.getContext('2d') as CanvasRenderingContext2D;
  const grad = g.createRadialGradient(128, 128, 0, 128, 128, 128);
  grad.addColorStop(0, 'rgba(255, 226, 190, 0.20)');
  grad.addColorStop(0.55, 'rgba(255, 226, 190, 0.07)');
  grad.addColorStop(1, 'rgba(255, 226, 190, 0)');
  g.fillStyle = grad;
  g.fillRect(0, 0, 256, 256);
  poolTex = new THREE.CanvasTexture(c);
  poolTex.colorSpace = THREE.SRGBColorSpace;
  return poolTex;
}

const TRAY_MAT = new THREE.MeshStandardMaterial({ color: 0x2a3242, roughness: 0.7 });

const GHOST = new THREE.MeshBasicMaterial({ color: 0x9fc4ff, transparent: true, opacity: 0.13, depthWrite: false });

const PAGE_W_WORLD = 0.56;
const PAGE_TILT = 0.06;
/** Page centre height: the cover's top (0.02) plus the drop of the tilted outer edge. */
const PAGE_Y = 0.02 + (PAGE_W_WORLD / 2) * Math.sin(PAGE_TILT) + 0.003;
/** The turning page pivots at the spine, the pages' highest line. */
const FLIP_Y = 0.02 + PAGE_W_WORLD * Math.sin(PAGE_TILT) + 0.005;

/** The instruction book: two pages on a spine, and a page that turns between them. */
class Book {
  readonly group = new THREE.Group();
  /** The open spread, tilted toward the yard: its +y is the way the pages face. */
  readonly face = new THREE.Group();
  private readonly left: THREE.Mesh;
  private readonly right: THREE.Mesh;
  private readonly flipper: THREE.Group;
  private readonly flipFront: THREE.Mesh;
  private readonly flipBack: THREE.Mesh;
  private flip = -1;
  private flipDir = 1;
  private riffleT = -1;
  private pendingLeft: THREE.Texture | null = null;
  rightCanvas: HTMLCanvasElement | null = null;
  private textures: THREE.Texture[] = [];

  constructor() {
    const w = PAGE_W_WORLD;
    const h = (w * PAGE_H) / PAGE_W;
    const cover = new THREE.Mesh(
      new THREE.BoxGeometry(w * 2 + 0.05, 0.02, h + 0.04),
      new THREE.MeshStandardMaterial({ color: 0x0f2f5c, roughness: 0.6 }),
    );
    cover.position.y = 0.01;
    cover.castShadow = true;
    // Paper is unlit and skips tone mapping: lit through ACES, a white page read as a
    // grey card and the ink lost its contrast.
    // Fog off too: the yard fog tinted far pages the blue of the sky.
    const blank = new THREE.MeshBasicMaterial({ color: 0xdfe9f2, toneMapped: false, fog: false });
    const pageGeo = new THREE.PlaneGeometry(w, h);
    this.left = new THREE.Mesh(pageGeo, blank.clone());
    this.right = new THREE.Mesh(pageGeo, blank.clone());
    this.left.rotation.x = -Math.PI / 2;
    this.right.rotation.x = -Math.PI / 2;
    // The tilt drops each page's outer edge, so lift the pages until that edge clears
    // the cover: at 0.022 the outer third sank into it and the page read as cropped.
    this.left.position.set(-w / 2 - 0.004, PAGE_Y, 0);
    this.right.position.set(w / 2 + 0.004, PAGE_Y, 0);
    // A gentle tilt, like a book lying open: pages rise toward the spine.
    this.left.rotation.y = -PAGE_TILT;
    this.right.rotation.y = PAGE_TILT;
    this.flipper = new THREE.Group();
    this.flipper.position.set(0, FLIP_Y, 0);
    const front = pageGeo.clone().translate(w / 2, 0, 0);
    this.flipFront = new THREE.Mesh(front, blank.clone());
    this.flipBack = new THREE.Mesh(front.clone(), blank.clone());
    this.flipFront.rotation.x = -Math.PI / 2;
    this.flipBack.rotation.x = Math.PI / 2;
    this.flipBack.scale.x = -1;
    this.flipper.add(this.flipFront, this.flipBack);
    this.flipper.visible = false;
    // Propped on a stand and tilted toward the camera, so a page reads from the yard.
    const tilt = this.face;
    tilt.add(cover, this.left, this.right, this.flipper);
    const lean = 0.62;
    tilt.rotation.x = lean;
    tilt.position.set(0, (h / 2) * Math.sin(lean) + 0.01, -(h / 2) * (1 - Math.cos(lean)));
    const stand = new THREE.Mesh(
      new THREE.BoxGeometry(w * 1.6, 0.03, 0.05),
      new THREE.MeshStandardMaterial({ color: 0x1a2130, roughness: 0.7 }),
    );
    stand.position.set(0, (h / 2) * Math.sin(lean) * 0.9, -(h / 2) * Math.cos(lean) * 0.9);
    stand.rotation.x = lean;
    this.group.add(tilt, stand);
  }

  private tex(c: HTMLCanvasElement): THREE.Texture {
    const t = new THREE.CanvasTexture(c);
    t.colorSpace = THREE.SRGBColorSpace;
    t.anisotropy = 16;
    this.textures.push(t);
    if (this.textures.length > 12) this.textures.shift()?.dispose();
    return t;
  }

  private setMap(m: THREE.Mesh, t: THREE.Texture | null) {
    const mat = m.material as THREE.MeshBasicMaterial;
    mat.map = t;
    mat.color.set(t ? 0xffffff : 0xdfe9f2);
    mat.needsUpdate = true;
  }

  /** Turn to a new spread. The old right page turns over and becomes the new left. */
  turn(left: HTMLCanvasElement | null, right: HTMLCanvasElement, forward: boolean) {
    const oldRight = (this.right.material as THREE.MeshBasicMaterial).map;
    const lt = left ? this.tex(left) : null;
    const rt = this.tex(right);
    this.rightCanvas = right;
    this.flipDir = forward ? 1 : -1;
    if (forward) {
      this.setMap(this.flipFront, oldRight);
      this.setMap(this.flipBack, lt);
      this.setMap(this.right, rt);
      this.pendingLeft = lt;
    } else {
      this.setMap(this.flipFront, rt);
      this.setMap(this.flipBack, (this.left.material as THREE.MeshBasicMaterial).map);
      this.setMap(this.left, lt);
      this.pendingLeft = null;
      this.setMap(this.right, rt);
    }
    this.flip = 0;
    this.flipper.visible = true;
  }

  /** A quick lift-and-settle of the right page: someone skimming. */
  riffle(full = false) {
    if (this.flip >= 0) return;
    this.riffleT = full ? 0.0001 : 0;
  }

  update(dtMs: number) {
    if (this.flip >= 0) {
      this.flip = Math.min(1, this.flip + dtMs / 520);
      const f = smooth(this.flip);
      // Forward turns right-to-left about the spine.
      this.flipper.rotation.z = this.flipDir > 0 ? f * Math.PI : (1 - f) * Math.PI;
      this.flipper.position.y = FLIP_Y + Math.sin(f * Math.PI) * 0.03;
      if (this.flip >= 1) {
        this.flip = -1;
        this.flipper.visible = false;
        if (this.pendingLeft !== null || this.flipDir > 0) this.setMap(this.left, this.pendingLeft);
      }
    } else if (this.riffleT >= 0) {
      this.riffleT = Math.min(1, this.riffleT + dtMs / 420);
      this.right.rotation.z = Math.sin(this.riffleT * Math.PI) * 0.5;
      this.right.position.y = PAGE_Y + Math.sin(this.riffleT * Math.PI) * 0.02;
      if (this.riffleT >= 1) {
        this.riffleT = -1;
        this.right.rotation.z = 0;
        this.right.position.y = PAGE_Y;
      }
    }
  }

  dispose() {
    for (const t of this.textures) t.dispose();
  }
}
