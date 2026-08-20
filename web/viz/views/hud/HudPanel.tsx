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
// Three nested boxes, each with exactly one job, because they cannot be collapsed:
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
//
// The canvas is sized ONCE, to the largest panel the layout can produce, and clipped by
// the panel's `overflow: hidden`. Resizing a WebGL drawing buffer every frame of a
// spring would reallocate it every frame.

import { useCallback, useMemo, useRef, type ReactNode, type RefObject } from 'react';
import { Canvas, useFrame } from '@react-three/fiber';
import * as THREE from 'three';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { radarHarness } from '@/viz/modules/radar/radarTheme';
import { useReducedMotion } from '@/viz/shared/scene/reducedMotion';
import { spring, springSnap, springStep, type Spring } from '@/viz/shared/lib/spring';
import { GENIE_MS, genieFrame, smootherstep } from './hudGenie';
import { HudGlobe } from './HudGlobe';
import {
  HUD_CELL_H,
  HUD_CELL_W,
  HUD_GLOBE_OFFSET_Y,
  HUD_HEADER_H,
  HUD_MAX_COLS,
  HUD_MAX_ROWS,
  HUD_OVERFLOW_H,
  HUD_PAD,
  hudCellCentre,
  hudLayout,
  hudPanelLeft,
  type HudGrid,
} from './hudLayout';
import { hudCellLabel, hudCellTooltip, hudSummary, hudSummaryLabel } from './hudSort';

export type HudPhase = 'closed' | 'opening' | 'open' | 'closing';

/** The tray icon, in window-local CSS px: where the island comes from and returns to. */
export type HudNeck = { centreX: number; width: number };

/** The canvas is cut once, at the biggest panel `hudLayout` can ask for. */
const MAX_W = HUD_PAD * 2 + HUD_MAX_COLS * HUD_CELL_W;
const MAX_H = HUD_PAD * 2 + HUD_HEADER_H + HUD_MAX_ROWS * HUD_CELL_H + HUD_OVERFLOW_H;

/** The island at rest, before it has grown: a menu-bar item's own height. */
const SEED_H = 22;

/** Per-cell entrance delay, and how long one cell takes to arrive. */
const STAGGER_MS = 34;
const CELL_IN_MS = 210;
/** The reduced-motion path: a plain cross-fade, no travel. */
const FADE_MS = 140;

type Placement = { x: number; y: number; opacity: number };

/** What the driver writes each frame and the scene reads back on the same frame. */
type FrameBus = { cells: Placement[] };

export function HudPanel({
  agents,
  phase,
  neck,
  windowW,
  hoveredId,
  onClosed,
  onPick,
  onHover,
}: {
  agents: RadarAgent[];
  phase: HudPhase;
  neck: HudNeck;
  windowW: number;
  hoveredId: string | null;
  onClosed: () => void;
  onPick: (agent: RadarAgent) => void;
  onHover: (id: string | null) => void;
}) {
  const reduced = useReducedMotion();
  const clipRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const cellRefs = useRef<Array<HTMLButtonElement | null>>([]);

  const grid = useMemo<HudGrid>(() => hudLayout(agents.length), [agents.length]);
  const visible = useMemo(() => agents.slice(0, grid.visible), [agents, grid.visible]);
  const summary = useMemo(() => hudSummary(agents), [agents]);
  const bus = useRef<FrameBus>({ cells: [] });

  return (
    <div className="wd-hud-shade">
      <div ref={clipRef} className="wd-hud-clip">
        <div ref={panelRef} className={`wd-hud-panel${grid.visible === 0 ? ' is-pill' : ''}`}>
          <Canvas
            className="wd-hud-canvas"
            style={{ width: MAX_W, height: MAX_H }}
            orthographic
            camera={{ position: [0, 0, 40], left: 0, right: MAX_W, top: 0, bottom: -MAX_H, near: 0.1, far: 200 }}
            gl={{ alpha: true, antialias: true, powerPreference: 'high-performance' }}
            frameloop={phase === 'closed' ? 'never' : 'always'}
            dpr={[1, 2]}
            onCreated={({ gl }) => {
              gl.toneMapping = THREE.ACESFilmicToneMapping;
              gl.toneMappingExposure = 1.05;
            }}
          >
            <HudDriver
              phase={phase}
              grid={grid}
              neck={neck}
              windowW={windowW}
              count={visible.length}
              reduced={reduced}
              bus={bus}
              clipRef={clipRef}
              panelRef={panelRef}
              contentRef={contentRef}
              cellRefs={cellRefs}
              onClosed={onClosed}
            />
            {visible.map((a, i) => (
              <HudGlobeSlot key={a.id} index={i} bus={bus}>
                <HudGlobe
                  harness={a.harness}
                  status={a.status}
                  fillPct={a.fillPct}
                  hovered={hoveredId === a.id}
                  reduced={reduced}
                />
              </HudGlobeSlot>
            ))}
          </Canvas>

          <div ref={contentRef} className="wd-hud-content">
            <div className="wd-hud-head">
              {summary.awaiting > 0 && <i className="wd-hud-alert-dot" aria-hidden />}
              <span>{hudSummaryLabel(summary)}</span>
            </div>
            {visible.map((a, i) => (
              <HudCell
                key={a.id}
                agent={a}
                hovered={hoveredId === a.id}
                elRef={(el) => {
                  cellRefs.current[i] = el;
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
  );
}

/** Positions one globe from the bus each frame; the body itself is a child. */
function HudGlobeSlot({
  index,
  bus,
  children,
}: {
  index: number;
  bus: RefObject<FrameBus>;
  children: ReactNode;
}) {
  const g = useRef<THREE.Group>(null!);
  useFrame(() => {
    const p = bus.current.cells[index];
    if (!p) {
      g.current.visible = false;
      return;
    }
    g.current.visible = p.opacity > 0.01;
    g.current.position.set(p.x, -(p.y + HUD_GLOBE_OFFSET_Y), 0);
    // Globes arrive on the same curve as their labels: they grow the last few percent
    // into place rather than fading in on the spot.
    g.current.scale.setScalar(0.86 + p.opacity * 0.14);
  });
  return <group ref={g}>{children}</group>;
}

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
  count,
  reduced,
  bus,
  clipRef,
  panelRef,
  contentRef,
  cellRefs,
  onClosed,
}: {
  phase: HudPhase;
  grid: HudGrid;
  neck: HudNeck;
  windowW: number;
  count: number;
  reduced: boolean;
  bus: RefObject<FrameBus>;
  clipRef: RefObject<HTMLDivElement | null>;
  panelRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  cellRefs: RefObject<Array<HTMLButtonElement | null>>;
  onClosed: () => void;
}) {
  const w = useRef<Spring>(spring(neck.width));
  const h = useRef<Spring>(spring(SEED_H));
  const clock = useRef({ phase: 'closed' as HudPhase, openedAt: 0, closedAt: 0, elapsed: 0, done: false });

  useFrame((_state, dtRaw) => {
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
      } else if (phase === 'closing') {
        c.closedAt = c.elapsed;
        c.done = false;
      }
      c.phase = phase;
    }

    const clip = clipRef.current;
    const panel = panelRef.current;
    const content = contentRef.current;
    if (!clip || !panel || !content) return;

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
    if (!closing && !shut) {
      if (reduced) {
        w.current = springSnap(grid.width);
        h.current = springSnap(grid.height);
      } else {
        w.current = springStep(w.current, grid.width, dt);
        h.current = springStep(h.current, grid.height, dt);
      }
    }
    const aw = w.current.value;
    const ah = h.current.value;
    const left = hudPanelLeft(neck.centreX, aw, windowW);

    const genieT = shut ? 1 : closing ? Math.min(1, (c.elapsed - c.closedAt) / GENIE_MS) : 0;
    // The reduced-motion path: no funnel, no size morph, just a short cross-fade.
    const fade = shut
      ? 0
      : closing
        ? 1 - Math.min(1, (c.elapsed - c.closedAt) / FADE_MS)
        : Math.min(1, (c.elapsed - c.openedAt) / FADE_MS);
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
    clip.style.opacity = reduced ? fade.toFixed(3) : '1';
    // A pill is fully round; the rectangle settles at 18px. Deriving the radius from
    // the live height makes the corner morph free and impossible to desync.
    panel.style.borderRadius = `${Math.min(18, ah / 2).toFixed(1)}px`;

    content.style.transform = g.contentTransform;
    content.style.transformOrigin = g.contentOrigin;
    content.style.opacity = String(g.contentOpacity);

    // Cells: staggered in on open, carried wholesale by the genie on close.
    const sinceOpen = c.elapsed - c.openedAt;
    const cells = bus.current.cells;
    cells.length = count;
    for (let i = 0; i < count; i++) {
      const e = reduced || closing || shut ? 1 : smootherstep((sinceOpen - i * STAGGER_MS) / CELL_IN_MS);
      const centre = hudCellCentre(i, { ...grid, width: aw });
      const el = cellRefs.current[i];
      if (el) {
        el.style.transform =
          `translate3d(${Math.round(centre.x - HUD_CELL_W / 2)}px, ` +
          `${Math.round(centre.y - HUD_CELL_H / 2 + (1 - e) * 7)}px, 0) ` +
          `scale(${(0.94 + e * 0.06).toFixed(3)})`;
        el.style.opacity = e.toFixed(3);
        el.style.pointerEvents = e > 0.9 && !closing && !shut ? 'auto' : 'none';
      }
      cells[i] = { x: centre.x, y: centre.y, opacity: e };
    }

    if (closing && (reduced ? c.elapsed - c.closedAt >= FADE_MS : genieT >= 1) && !c.done) {
      c.done = true;
      onClosed();
    }
  });

  return null;
}

function HudCell({
  agent,
  hovered,
  elRef,
  onPick,
  onHover,
}: {
  agent: RadarAgent;
  hovered: boolean;
  elRef: (el: HTMLButtonElement | null) => void;
  onPick: (a: RadarAgent) => void;
  onHover: (id: string | null) => void;
}) {
  const theme = radarHarness(agent.harness);
  const onEnter = useCallback(() => onHover(agent.id), [onHover, agent.id]);
  const onLeave = useCallback(() => onHover(null), [onHover]);
  const status = agent.status;
  const word = status === 'awaiting' ? 'awaiting' : status === 'working' ? 'working' : 'idle';

  return (
    <button
      ref={elRef}
      type="button"
      className={`wd-hud-cell is-${status}${hovered ? ' is-hovered' : ''}`}
      style={{ width: HUD_CELL_W, height: HUD_CELL_H }}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      onClick={() => onPick(agent)}
      title={hudCellTooltip(agent, theme.label)}
    >
      <span className="wd-hud-cell-label">{hudCellLabel(agent)}</span>
      <span className="wd-hud-cell-status">
        <i className="wd-hud-cell-glyph" style={{ color: theme.color }} aria-hidden>
          {theme.glyph}
        </i>
        {word}
        {agent.childCount > 0 && <em className="wd-hud-cell-kids">·{agent.childCount}</em>}
      </span>
    </button>
  );
}
