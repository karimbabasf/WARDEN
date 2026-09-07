// HudPanel.tsx: the island. The panel that grows out of the tray icon,
// holds the fleet, and genies back into it.
//
// ONE animation loop, and it lives inside the R3F <Canvas>. That is deliberate. The
// panel's box, its clip polygon, every cell's transform, and every globe's position
// all have to agree on the same frame, and two rAF loops (one for the DOM, R3F's own
// for the scene) agree only by luck of registration order: the visible symptom is
// labels sliding a frame ahead of the globes they name. Driving the DOM from
// `useFrame` makes that impossible by construction and costs zero React renders while
// the panel moves.
//
// Four nested boxes, each with exactly one job, because they cannot be collapsed:
//   .wd-hud-shade   carries the shadow as a `drop-shadow` FILTER. A filter applies to
//                   its child's already-clipped output, so the shadow traces the genie
//                   funnel instead of staying a rectangle around a shape that is no
//                   longer one. A `box-shadow` on the clipped element would be clipped
//                   away entirely.
//   .wd-hud-clip    carries the size, the position and the clip polygon.
//   .wd-hud-panel   carries the material: background, border, and the radius that
//                   morphs pill-to-rectangle. Its radius must NOT live on the clipped
//                   element, because a polygon replaces the radius and would square the
//                   corners off the moment the genie is armed.
//   .wd-hud-stage   carries the genie's content transform, and it holds EVERYTHING the
//                   panel draws: the hover plates, the WebGL canvas and the DOM labels.
//                   It used to be the labels alone, and the globes, sitting still in an
//                   untransformed canvas, were guillotined by the rising bottom edge
//                   halfway through the close while the captions above them slid up
//                   through the neck. One box, one transform: the board funnels whole.
//
// The canvas is sized ONCE, to the largest panel the layout can produce, and clipped by
// the panel's `overflow: hidden`. Resizing a WebGL drawing buffer every frame of a
// spring would reallocate it every frame.
//
// ── cells are keyed by agent, and each one animates for itself ───────────────
// A cell's SLOT comes from the grid; WHERE IT IS comes from a pair of springs per
// agent. So a board that reflows (an agent arriving, one leaving, one moving to the
// front because it started asking) slides every cell to its new slot instead of
// teleporting the row, which is what it did when a cell's position was read straight
// off its index. The springs run RELATIVE TO THE PANEL'S CENTRE LINE, and the live
// width places that line each frame. Two things follow. The panel is centred under
// the icon, so a cell whose slot has not moved is dead still in window space while
// the box springs open around it. And when the grid gains a column the target
// moves, not the frame of reference: in grid coordinates the same cell would have
// jumped half a column the instant the layout changed, before any spring could act.
//
// A cell that joins an OPEN board blooms in place; one that leaves shrinks and fades
// in place while its neighbours close ranks. It stays mounted for the length of its
// exit (hudLinger) but holds no slot. Neither is the opening stagger, which only the
// first batch after a summon gets.
//
// ── what is clickable ─────────────────────────────────────────────────────────
// Root sessions only. A root is a real session with a window behind it, so picking one
// is a jump you can take; a subagent is work happening inside that session and has no
// window of its own, so it is DRAWN and never offered. Its moons live in the canvas
// (which is `pointer-events: none`) and nowhere in the DOM, which is what makes them
// inert by construction rather than by remembering to say so.

import { useCallback, useMemo, useRef, type ReactNode, type RefObject } from 'react';
import { Canvas, useFrame } from '@react-three/fiber';

import { useHostVisible } from '@/viz/shared/state/hudTransport';
import * as THREE from 'three';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { radarHarness } from '@/viz/modules/radar/radarTheme';
import { useReducedMotion } from '@/viz/shared/scene/reducedMotion';
import { spring, springSnap, springStep, type Spring } from '@/viz/shared/lib/spring';
import { genieFrame, genieProgress, smootherstep } from './hudGenie';
import { HudBloom, HudGlobe, HudSceneRig } from './HudGlobe';
import {
  HUD_CELL_H,
  HUD_CELL_W,
  HUD_GLOBE_OFFSET_Y,
  HUD_KID_RADIUS,
  HUD_MAX_H,
  HUD_MAX_W,
  hudCellCentre,
  hudEmbedLayout,
  hudGlobeRadius,
  hudKidCentre,
  hudLayout,
  hudPanelLeft,
  type HudGrid,
} from './hudLayout';
import { useLingering, type Lingering } from './hudLinger';
import { hudCellLabel, hudCellTooltip, hudKidCount, hudSummary, hudSummaryLabel, type HudNode } from './hudSort';

export type HudPhase = 'closed' | 'opening' | 'open' | 'closing';

/** The tray icon, in window-local CSS px: where the island comes from and returns to. */
export type HudNeck = { centreX: number; width: number };

/** The island at rest, before it has grown: a menu-bar item's own height. */
const SEED_H = 22;

/** The box. Width leads and height follows, so the island widens under the icon first
 *  and then drops, the way a pill would, rather than zooming out of the icon as one
 *  rectangle: one response for both read as a pop. Each lands with a touch of
 *  overshoot (about 2% on the width, under 1% on the height), which is the difference
 *  between arriving and stopping. The same springs re-target when the fleet changes
 *  size mid-open. */
const W_RESPONSE = 0.3;
const W_DAMPING = 0.8;
const H_RESPONSE = 0.36;
const H_DAMPING = 0.88;
/** The material fades up over the first frames, so a 24px box does not simply appear
 *  under the icon a frame before it starts to grow. */
const MATERIAL_IN_MS = 70;
/** The header holds back until the box has room for it. Revealed by the growing clip
 *  alone, it read as a line of text being cut at both ends. */
const HEAD_DELAY_MS = 90;
const HEAD_IN_MS = 200;

/** The opening batch: cells arrive staggered on a short rise, once the box has some
 *  room for them, and their globes grow in from a third of their size. */
const CELLS_LEAD_MS = 60;
const STAGGER_MS = 38;
const CELL_IN_MS = 210;
const BATCH_GLOBE_FROM = 0.35;
/** For this long after a summon a first-seen cell still belongs to the opening batch:
 *  the radar can push its first frame a beat after the tray click. */
const OPEN_BATCH_MS = 120;
/** A cell that joins an OPEN board blooms in place: the caption fades up while the
 *  globe grows from a point and settles with a little overshoot. */
const BLOOM_IN_MS = 260;
const BLOOM_FROM = 0.25;
const BLOOM_RESPONSE = 0.36;
const BLOOM_DAMPING = 0.7;
/** A cell that leaves shrinks and fades where it stands while its neighbours close
 *  ranks. hudLinger keeps its element mounted for exactly this long. */
export const HUD_EXIT_MS = 220;
/** How fast a cell slides to a new slot. */
const REFLOW_RESPONSE = 0.34;
/** The reduced-motion path: a plain cross-fade, no travel. */
const FADE_MS = 140;

type Placement = { x: number; y: number; opacity: number; scale: number };

/** What the driver writes each frame and the scene reads back on the same frame. */
type FrameBus = { cells: Map<string, Placement>; kids: Map<string, Placement> };

/** One moon, flattened out of the tree so the frame loop can index it in one pass. It
 *  names its root by id, not by cell index: a root's index changes when the board
 *  reflows, and a leaving root has no index at all. */
type KidSlot = { agent: RadarAgent; parentId: string; index: number; count: number };

const nodeId = (n: HudNode) => n.agent.id;
const kidId = (k: KidSlot) => k.agent.id;

export function HudPanel({
  nodes,
  phase,
  neck,
  windowW,
  embedded,
  hostBox,
  hoveredId,
  onClosed,
  onPick,
  onHover,
}: {
  nodes: HudNode[];
  phase: HudPhase;
  neck: HudNeck;
  windowW: number;
  /** Painting inside somebody else's panel (the notch section), not our own window.
   *  The host owns the material, the corner radius, the shadow and the open/close
   *  animation, so this panel draws none of them: see the `.is-embedded` block in
   *  hud.css and the `embedded` branches in HudDriver. */
  embedded: boolean;
  /** The host's content box in CSS px. Ignored unless `embedded`. */
  hostBox: { width: number; height: number };
  hoveredId: string | null;
  onClosed: () => void;
  onPick: (agent: RadarAgent) => void;
  onHover: (id: string | null) => void;
}) {
  const hostVisible = useHostVisible();
  const reduced = useReducedMotion();
  const clipRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const headRef = useRef<HTMLDivElement>(null);
  const cellEls = useRef(new Map<string, HTMLButtonElement>());
  const plateEls = useRef(new Map<string, HTMLDivElement>());

  // Height depends on the moons, not just the session count, so the layout is fed the
  // per-root kid counts rather than a total (see hudLayout).
  const kidCounts = useMemo(() => nodes.map((n) => n.kids.length), [nodes]);
  const grid = useMemo<HudGrid>(
    () => (embedded ? hudEmbedLayout(kidCounts, hostBox) : hudLayout(kidCounts)),
    [kidCounts, embedded, hostBox],
  );
  // The canvas is cut once at the largest box its host can ask for. In our own window
  // that is the island at full size; in the section it is the host's box, which is
  // fixed for the life of the view. The camera's frustum is `manual` (R3F marks it so
  // the moment a `left`/`right` is passed) and therefore never follows a resize on its
  // own, so the driver syncs it: see the top of its frame loop.
  const canvasW = embedded ? grid.width : HUD_MAX_W;
  const canvasH = embedded ? grid.height : HUD_MAX_H;
  const visible = useMemo(() => nodes.slice(0, grid.visible), [nodes, grid.visible]);
  const summary = useMemo(() => hudSummary(nodes.map((n) => n.agent)), [nodes]);
  const kidSlots = useMemo<KidSlot[]>(
    () =>
      visible.flatMap((n) =>
        n.kids.map((agent, index) => ({ agent, parentId: n.agent.id, index, count: n.kids.length })),
      ),
    [visible],
  );
  // The live cells and moons, plus the ones still leaving. The grid above is laid out
  // from the live list only: a leaving cell holds no slot.
  const shown = useLingering(visible, nodeId, HUD_EXIT_MS);
  const shownKids = useLingering(kidSlots, kidId, HUD_EXIT_MS);
  const bus = useRef<FrameBus>({ cells: new Map(), kids: new Map() });

  return (
    <div className="wd-hud-shade">
      <div ref={clipRef} className="wd-hud-clip">
        <div ref={panelRef} className={`wd-hud-panel${grid.visible === 0 ? ' is-pill' : ''}`}>
          <div ref={stageRef} className="wd-hud-stage">
            {/* The hover plate, and it is BEFORE the canvas on purpose. It used to be a
                background on the cell button itself, which lives above the canvas, so
                hovering a session painted an opaque rectangle over its own globe: the
                one thing you were pointing at was the one thing that disappeared.
                Behind the canvas, the globe sits ON the plate and hover reads as the
                cell lighting up under it, which is also the direction a hover should
                go. */}
            <div className="wd-hud-plates" aria-hidden>
              {shown.map(({ item: n }) => (
                <div
                  key={n.agent.id}
                  ref={(el) => {
                    if (el) plateEls.current.set(n.agent.id, el);
                    else plateEls.current.delete(n.agent.id);
                  }}
                  className={`wd-hud-plate${hoveredId === n.agent.id ? ' is-hovered' : ''}`}
                  style={{ width: grid.pitch, height: HUD_CELL_H }}
                />
              ))}
            </div>

            <Canvas
              className="wd-hud-canvas"
              style={{ width: canvasW, height: canvasH }}
              orthographic
              camera={{ position: [0, 0, 40], left: 0, right: HUD_MAX_W, top: 0, bottom: -HUD_MAX_H, near: 0.1, far: 200 }}
              gl={{ alpha: true, antialias: true, powerPreference: 'high-performance' }}
              // 'never' when the host is not showing us. In our own window `hostVisible`
              // is a constant true and this is exactly the old expression; in the notch
              // section the panel is ALWAYS open, so without this term the scene would
              // render at display rate forever behind a closed notch.
              frameloop={phase === 'closed' || !hostVisible ? 'never' : 'always'}
              dpr={[1, 2]}
              onCreated={({ gl }) => {
                gl.toneMapping = THREE.ACESFilmicToneMapping;
                gl.toneMappingExposure = 1.05;
              }}
            >
              <HudSceneRig />
              <HudDriver
                phase={phase}
                grid={grid}
                neck={neck}
                windowW={windowW}
                embedded={embedded}
                canvasW={canvasW}
                canvasH={canvasH}
                shown={shown}
                shownKids={shownKids}
                reduced={reduced}
                bus={bus}
                clipRef={clipRef}
                panelRef={panelRef}
                stageRef={stageRef}
                headRef={headRef}
                cellEls={cellEls}
                plateEls={plateEls}
                onClosed={onClosed}
              />
              {shown.map(({ item: n }) => (
                <HudGlobeSlot key={n.agent.id} id={n.agent.id} track="cells" bus={bus}>
                  <HudGlobe
                    id={n.agent.id}
                    harness={n.agent.harness}
                    status={n.agent.status}
                    radius={hudGlobeRadius(n.agent.fillPct)}
                    isRoot
                    hovered={hoveredId === n.agent.id}
                    reduced={reduced}
                  />
                </HudGlobeSlot>
              ))}
              {shownKids.map(({ item: k }) => (
                <HudGlobeSlot key={k.agent.id} id={k.agent.id} track="kids" bus={bus}>
                  <HudGlobe
                    id={k.agent.id}
                    harness={k.agent.harness}
                    status={k.agent.status}
                    radius={HUD_KID_RADIUS}
                    isRoot={false}
                    reduced={reduced}
                  />
                </HudGlobeSlot>
              ))}
              {/* LAST: a composer renders the whole scene, so everything that blooms
                  has to be in the tree above it. */}
              <HudBloom />
            </Canvas>

            <div className="wd-hud-content">
              {/* Dropped in the section, and not only to save its 24px. The notch draws
                  its own header naming this tab, so a second centred caps line under it
                  was the same label twice; the alert it carries is already on every
                  awaiting cell, in the same crimson on the same beat. */}
              {!embedded && (
                <div ref={headRef} className="wd-hud-head">
                  {summary.awaiting > 0 && <i className="wd-hud-alert-dot" aria-hidden />}
                  <span>{hudSummaryLabel(summary)}</span>
                </div>
              )}
              {shown.map(({ item: n, leaving }) => (
                <HudCell
                  key={n.agent.id}
                  node={n}
                  width={grid.pitch}
                  leaving={leaving}
                  hovered={hoveredId === n.agent.id}
                  elRef={(el) => {
                    if (el) cellEls.current.set(n.agent.id, el);
                    else cellEls.current.delete(n.agent.id);
                  }}
                  onPick={onPick}
                  onHover={onHover}
                />
              ))}
              {grid.overflow > 0 && (
                <div className="wd-hud-overflow">+{grid.overflow} more in the war room</div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Positions one globe from the bus each frame; the body itself is a child. */
function HudGlobeSlot({
  id,
  track,
  bus,
  children,
}: {
  id: string;
  /** Which lane of the bus to read: root cells, or the moons under them. */
  track: 'cells' | 'kids';
  bus: RefObject<FrameBus>;
  children: ReactNode;
}) {
  const g = useRef<THREE.Group>(null!);
  useFrame(() => {
    const p = bus.current[track].get(id);
    if (!p) {
      g.current.visible = false;
      return;
    }
    g.current.visible = p.opacity > 0.01 && p.scale > 0.01;
    g.current.position.set(p.x, -p.y, 0);
    // The globe has no opacity of its own (its materials are shared with the war
    // room), so its arrival and its exit are both told with size: it grows in and
    // implodes out, which is also what the constellation does.
    g.current.scale.setScalar(Math.max(0.001, p.scale));
  });
  return <group ref={g}>{children}</group>;
}

/** Where a moon sits relative to its root's cell centre. Independent of the slot, so
 *  it can be added to a SPRUNG centre rather than a grid one. */
function kidOffset(index: number, count: number, grid: HudGrid): { x: number; y: number } {
  const cell = hudCellCentre(0, grid);
  const kid = hudKidCentre(0, index, count, grid);
  return { x: kid.x - cell.x, y: kid.y - cell.y };
}

/** One cell's motion state, keyed by agent and owned by the frame loop. */
type CellAnim = {
  x: Spring;
  y: Spring;
  /** Frame-clock time this cell starts arriving. In the future while it waits its
   *  turn in the opening stagger. */
  bornAt: number;
  /** Joined an open board, so it blooms rather than rides the opening stagger. */
  bloom: boolean;
  /** The globe's scale while blooming. */
  globe: Spring;
  /** Frame-clock time the exit began, once the cell is leaving. */
  diedAt: number | null;
};

type KidAnim = Omit<CellAnim, 'x' | 'y'>;

/**
 * The whole HUD animation, in one place.
 *
 * Opening springs the box; closing runs the genie over it. They are deliberately NOT
 * the same motion reversed: the expansion has to read as an arrival (so it settles,
 * with the content cross-fading in behind it), while the collapse has to read as the
 * panel going back where it came from (so it funnels, and the funnel is what points at
 * the icon). Both start and end at the tray icon, which is the part that matters.
 */
function HudDriver({
  phase,
  grid,
  neck,
  windowW,
  embedded,
  canvasW,
  canvasH,
  shown,
  shownKids,
  reduced,
  bus,
  clipRef,
  panelRef,
  stageRef,
  headRef,
  cellEls,
  plateEls,
  onClosed,
}: {
  phase: HudPhase;
  grid: HudGrid;
  neck: HudNeck;
  windowW: number;
  embedded: boolean;
  canvasW: number;
  canvasH: number;
  shown: Lingering<HudNode>[];
  shownKids: Lingering<KidSlot>[];
  reduced: boolean;
  bus: RefObject<FrameBus>;
  clipRef: RefObject<HTMLDivElement | null>;
  panelRef: RefObject<HTMLDivElement | null>;
  stageRef: RefObject<HTMLDivElement | null>;
  /** Null in the section, where the host draws the header. */
  headRef: RefObject<HTMLDivElement | null>;
  cellEls: RefObject<Map<string, HTMLButtonElement>>;
  /** The hover plates, drawn behind the canvas and moved with their cells. */
  plateEls: RefObject<Map<string, HTMLDivElement>>;
  onClosed: () => void;
}) {
  const w = useRef<Spring>(spring(neck.width));
  const h = useRef<Spring>(spring(SEED_H));
  const clock = useRef({ phase: 'closed' as HudPhase, openedAt: 0, closedAt: 0, elapsed: 0, done: false });
  const cellAnims = useRef(new Map<string, CellAnim>());
  const kidAnims = useRef(new Map<string, KidAnim>());

  useFrame((state, dtRaw) => {
    // The ortho frustum is in CSS px and R3F will not maintain it: passing `left`/
    // `right` marks the camera `manual`, which is exactly what stops it recentring on
    // (0,0) but also what stops it following the canvas. One compare a frame, and a
    // section that opens on a different display gets the right frustum on the next.
    const cam = state.camera as THREE.OrthographicCamera;
    if (cam.right !== canvasW || cam.bottom !== -canvasH) {
      cam.right = canvasW;
      cam.bottom = -canvasH;
      cam.updateProjectionMatrix();
    }

    const dt = Math.min(dtRaw, 0.05);
    const c = clock.current;
    c.elapsed += dt * 1000;

    // Phase transitions are detected HERE, not in an effect. A passive effect is not
    // guaranteed to have run before R3F's next frame, and when it had not, the first
    // closing frame read a `closedAt` of 0 against an `elapsed` of several seconds,
    // so the genie computed as already finished and the panel vanished in one frame
    // instead of funnelling. Reading the transition inside the loop makes the timestamp
    // and the frame that uses it atomic. It also means only a real TRANSITION re-seeds:
    // a radar push arriving mid-open cannot snap the panel back to the icon.
    if (phase !== c.phase) {
      if (phase === 'opening') {
        w.current = reduced ? springSnap(grid.width) : spring(neck.width);
        h.current = reduced ? springSnap(grid.height) : spring(SEED_H);
        c.openedAt = c.elapsed;
        c.done = false;
        // Whatever was on the board when the panel last closed, every cell now
        // belongs to the opening batch.
        cellAnims.current.clear();
        kidAnims.current.clear();
      } else if (phase === 'closing') {
        c.closedAt = c.elapsed;
        c.done = false;
      }
      c.phase = phase;
    }

    const clip = clipRef.current;
    const panel = panelRef.current;
    const stage = stageRef.current;
    const head = headRef.current;
    if (!clip || !panel || !stage) return;

    const closing = phase === 'closing';
    // `closed` has to hold the COLLAPSED frame, not fall through to the open one.
    // R3F runs at least one more frame after `onClosed` re-renders with
    // frameloop='never', and without this that frame repaints the panel at full size,
    // invisible in the app (the window is already hidden) but wrong, and the state the
    // next open would have to start from.
    const shut = phase === 'closed';
    // Size springs while the panel is on screen; frozen while the genie plays, because
    // the clip is already doing the moving and a box that also shrinks reads as two
    // effects fighting.
    // THE SECTION HAS NO ISLAND. Its box is the host's box: it does not grow out of a
    // tray icon, it cannot be dismissed into one, and the notch is already running its
    // own open spring around it. A second spring here would be two panels arriving on
    // two curves, which is the drift the whole embed was built to avoid.
    if (embedded) {
      w.current = springSnap(grid.width);
      h.current = springSnap(grid.height);
    } else if (!closing && !shut) {
      if (reduced) {
        w.current = springSnap(grid.width);
        h.current = springSnap(grid.height);
      } else {
        w.current = springStep(w.current, grid.width, dt, W_RESPONSE, W_DAMPING);
        h.current = springStep(h.current, grid.height, dt, H_RESPONSE, H_DAMPING);
      }
    }
    const aw = w.current.value;
    const ah = h.current.value;
    const left = embedded ? 0 : hudPanelLeft(neck.centreX, aw, windowW);
    const sinceOpen = c.elapsed - c.openedAt;
    const sinceClose = c.elapsed - c.closedAt;

    const genieT = embedded ? 0 : shut ? 1 : closing ? genieProgress(sinceClose) : 0;
    // The reduced-motion path: no funnel, no size morph, just a short cross-fade.
    const fade = shut
      ? 0
      : closing
        ? 1 - Math.min(1, sinceClose / FADE_MS)
        : Math.min(1, sinceOpen / FADE_MS);
    const g = genieFrame(reduced ? 0 : genieT, {
      width: aw,
      height: ah,
      neckLeft: neck.centreX - left - neck.width / 2,
      neckRight: neck.centreX - left + neck.width / 2,
    });

    clip.style.width = `${aw}px`;
    clip.style.height = `${ah}px`;
    clip.style.transform = `translate3d(${Math.round(left)}px, 0, 0)`;
    clip.style.clipPath = g.clipPath;
    // The material fades up as the box starts to grow; the genie owns the other end.
    clip.style.opacity = embedded
      ? '1'
      : reduced
        ? fade.toFixed(3)
        : shut
          ? '0'
          : Math.min(1, sinceOpen / MATERIAL_IN_MS).toFixed(3);
    // A pill is fully round; the rectangle settles at 18px. Deriving the radius from
    // the live height makes the corner morph free and impossible to desync. The
    // section has no corners of its own to round: the notch's are the only ones.
    panel.style.borderRadius = embedded ? '0px' : `${Math.min(18, ah / 2).toFixed(1)}px`;

    stage.style.transform = g.contentTransform;
    stage.style.transformOrigin = g.contentOrigin;
    stage.style.opacity = String(g.contentOpacity);
    if (head) {
      head.style.opacity = (
        reduced || closing || shut ? 1 : smootherstep((sinceOpen - HEAD_DELAY_MS) / HEAD_IN_MS)
      ).toFixed(3);
    }

    // ── cells ──────────────────────────────────────────────────────────────────
    // The springs run relative to the panel's centre line (see the header note); the
    // live width places that line each frame.
    const mid = grid.width / 2;
    // The genie takes the board wholesale: no cell arrives or leaves on its own once
    // the close has started, and reduced motion never travels.
    const carried = reduced || closing || shut;
    const inBatch = sinceOpen < OPEN_BATCH_MS;
    const cells = bus.current.cells;
    const anims = cellAnims.current;
    const seen = new Set<string>();
    let slot = 0;
    for (const { item: n, leaving } of shown) {
      const id = n.agent.id;
      seen.add(id);
      const i = leaving ? -1 : slot++;
      const target = leaving ? null : hudCellCentre(i, grid);
      let a = anims.get(id);
      if (!a) {
        // A ghost the loop never saw alive has nowhere to leave from.
        if (!target) continue;
        a = {
          x: spring(target.x - mid),
          y: spring(target.y),
          bornAt: inBatch ? c.openedAt + CELLS_LEAD_MS + i * STAGGER_MS : c.elapsed,
          bloom: !inBatch,
          globe: spring(inBatch ? BATCH_GLOBE_FROM : BLOOM_FROM),
          diedAt: null,
        };
        anims.set(id, a);
      }
      if (target) {
        a.diedAt = null;
        a.x = reduced ? springSnap(target.x - mid) : springStep(a.x, target.x - mid, dt, REFLOW_RESPONSE);
        a.y = reduced ? springSnap(target.y) : springStep(a.y, target.y, dt, REFLOW_RESPONSE);
      } else if (a.diedAt === null) {
        a.diedAt = c.elapsed;
      }

      const born = c.elapsed - a.bornAt;
      const e = carried ? 1 : smootherstep(born / (a.bloom ? BLOOM_IN_MS : CELL_IN_MS));
      const exit = a.diedAt === null ? 0 : reduced ? 1 : smootherstep((c.elapsed - a.diedAt) / HUD_EXIT_MS);
      let globe: number;
      if (a.bloom && !carried) {
        if (born >= 0) a.globe = springStep(a.globe, 1, dt, BLOOM_RESPONSE, BLOOM_DAMPING);
        globe = a.globe.value;
      } else {
        globe = BATCH_GLOBE_FROM + (1 - BATCH_GLOBE_FROM) * e;
      }
      const opacity = e * (1 - exit);
      const x = a.x.value + aw / 2;
      const y = a.y.value;
      // One transform, two elements: the plate lives in a different stacking layer from
      // the cell (it has to, to sit under the globes) but must never drift from it.
      const transform =
        `translate3d(${Math.round(x - grid.pitch / 2)}px, ` +
        `${Math.round(y - HUD_CELL_H / 2 + (1 - e) * 7)}px, 0) ` +
        `scale(${((0.94 + e * 0.06) * (1 - exit * 0.08)).toFixed(3)})`;
      const el = cellEls.current.get(id);
      if (el) {
        el.style.transform = transform;
        el.style.opacity = opacity.toFixed(3);
        el.style.pointerEvents = target && e > 0.9 && !closing && !shut ? 'auto' : 'none';
      }
      const plate = plateEls.current.get(id);
      if (plate) {
        plate.style.transform = transform;
        plate.style.opacity = opacity.toFixed(3);
      }
      // The globe sits above its own caption, so it is offset off the cell centre.
      cells.set(id, { x, y: y + HUD_GLOBE_OFFSET_Y, opacity, scale: globe * (1 - exit) });
    }
    for (const id of anims.keys()) {
      if (!seen.has(id)) {
        anims.delete(id);
        cells.delete(id);
      }
    }

    // ── moons ──────────────────────────────────────────────────────────────────
    // A moon hangs off its root's SPRUNG centre, so a strip slides with its session
    // and fades with it: one strip cannot arrive ahead of the globe it belongs to,
    // which is what would make them read as separate agents. A moon of its own that
    // joins or leaves a live session blooms or implodes on the same curves as a cell.
    const kids = bus.current.kids;
    const kanims = kidAnims.current;
    const kseen = new Set<string>();
    for (const { item: k, leaving } of shownKids) {
      const id = k.agent.id;
      kseen.add(id);
      const parent = anims.get(k.parentId);
      const root = cells.get(k.parentId);
      // The root is off the board entirely: nothing left to hang from.
      if (!parent || !root) continue;
      let a = kanims.get(id);
      if (!a) {
        if (leaving) continue;
        a = {
          bornAt: inBatch ? parent.bornAt : c.elapsed,
          bloom: !inBatch,
          globe: spring(inBatch ? 1 : BLOOM_FROM),
          diedAt: null,
        };
        kanims.set(id, a);
      }
      if (!leaving) a.diedAt = null;
      else if (a.diedAt === null) a.diedAt = c.elapsed;

      const born = c.elapsed - a.bornAt;
      const exit = a.diedAt === null ? 0 : reduced ? 1 : smootherstep((c.elapsed - a.diedAt) / HUD_EXIT_MS);
      let own = 1;
      let globe = root.scale;
      if (a.bloom && !carried) {
        own = smootherstep(born / BLOOM_IN_MS);
        if (born >= 0) a.globe = springStep(a.globe, 1, dt, BLOOM_RESPONSE, BLOOM_DAMPING);
        globe = a.globe.value;
      }
      const off = kidOffset(k.index, k.count, grid);
      kids.set(id, {
        x: root.x + off.x,
        y: root.y - HUD_GLOBE_OFFSET_Y + off.y,
        opacity: root.opacity * own * (1 - exit),
        scale: globe * (1 - exit),
      });
    }
    for (const id of kanims.keys()) {
      if (!kseen.has(id)) {
        kanims.delete(id);
        kids.delete(id);
      }
    }

    if (closing && (reduced ? sinceClose >= FADE_MS : genieT >= 1) && !c.done) {
      c.done = true;
      onClosed();
    }
  });

  return null;
}

function HudCell({
  node,
  width,
  leaving,
  hovered,
  elRef,
  onPick,
  onHover,
}: {
  node: HudNode;
  /** The cell's box, which is the grid's pitch: wider in the section, where the width
   *  is given rather than derived, so a task name gets more of a line before it
   *  truncates. */
  width: number;
  /** Fading out where it stands: still drawn, no longer offered. */
  leaving: boolean;
  hovered: boolean;
  elRef: (el: HTMLButtonElement | null) => void;
  onPick: (a: RadarAgent) => void;
  onHover: (id: string | null) => void;
}) {
  const agent = node.agent;
  const theme = radarHarness(agent.harness);
  const onEnter = useCallback(() => onHover(agent.id), [onHover, agent.id]);
  const onLeave = useCallback(() => onHover(null), [onHover]);
  const status = agent.status;
  const word = status === 'awaiting' ? 'awaiting' : status === 'working' ? 'working' : 'idle';
  const kids = hudKidCount(node);

  return (
    <button
      ref={elRef}
      type="button"
      className={`wd-hud-cell is-${status}${hovered ? ' is-hovered' : ''}`}
      style={{ width, height: HUD_CELL_H }}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      onClick={() => onPick(agent)}
      title={hudCellTooltip(agent, theme.label, kids)}
      tabIndex={leaving ? -1 : 0}
      aria-hidden={leaving || undefined}
    >
      <span className="wd-hud-cell-label">{hudCellLabel(agent)}</span>
      <span className="wd-hud-cell-status">
        <i className="wd-hud-cell-glyph" style={{ color: theme.color }} aria-hidden>
          {theme.glyph}
        </i>
        {word}
        {kids > 0 && <em className="wd-hud-cell-kids">·{kids}</em>}
      </span>
    </button>
  );
}
