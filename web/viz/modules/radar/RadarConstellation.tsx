// RadarConstellation.tsx — the LIVE agent-forest scene (the Radar tab).
//
// A sibling of WarRoom's `Scene`: same cinematic shell (black space, Environment
// probe, starfield, Bloom/Vignette, the shared free-orbit CameraRig) but its nodes
// are live agents/subagents, not anti-patterns. Geometry comes from
// `layoutRadarScene` (planets + orbiting moons, depth-N); a globe's size is its
// layout radius (context occupancy + hierarchy boost) and its colour is its
// harness hue HEATED BY FILL (`radarNodeColor`). Parent->child links glow along the
// tree. Lifecycle scale (spawn/implode) is injected by the parent (Task 16); the
// render multiplies the mesh scale by it so nothing ever snaps.
//
// Reuse-first: the Habits `Orb`/`AnimatedLinks` are coloured by the anti-pattern
// palette, so Radar uses its OWN heat-coloured globe + link mesh here rather than
// mutating those — but mirrors their lattice look so the two constellations match.

import { useEffect, useMemo, useRef, useState, type CSSProperties, type MutableRefObject } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Environment, Lightformer, Html } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode, OrbLayout, OrbLink } from '@/viz/shared/types/orbTypes';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { layoutRadarScene, type RadarCluster, type RadarLayout } from './radarLayout';
import { radarHarness } from './radarTheme';
import { RadarGlobeBody, damp, radarNodeColor, seedOf } from './RadarGlobeBody';
import { StarCatalog } from '@/viz/shared/scene/StarCatalog';
import { FoldGroup } from '@/viz/shared/scene/Transition';
import { CameraRig } from '@/viz/shared/scene/CameraRig';
import { frameloopFor } from '@/viz/shared/scene/frameloop';
import {
  reconcileLifecycle,
  pruneGone,
  isVisible,
  linkDrawProgress,
  type LifecycleEntry,
  type LifecycleMap,
  type LinkEndpointState,
  type LiveId,
} from './radarLifecycle';
import { RadarHoverCard } from './RadarHoverCard';
import { radarCanvasCamera } from '@/viz/shared/scene/useOrbCamera';
import { useReducedMotion } from '@/viz/shared/scene/reducedMotion';
import { targetDim, matchesFilter, type EmphasisFilter } from '@/viz/shared/lib/emphasis';

const BG = '#020403';

// A globe's own look now has ONE definition, shared with the menu-bar HUD (see
// RadarGlobeBody). Re-exported from here because this file was their address before
// the split, and a caller should not have to know that the body moved.
export {
  radarNodeColor,
  radarGlowTarget,
  radarLivenessColorScale,
} from './RadarGlobeBody';

type LinkFadeEndpoint = LinkEndpointState;

function endpointFadeFactor({ entry, gone = false }: LinkFadeEndpoint): number {
  if (gone || entry?.phase === 'gone') return 0;
  if (!entry) return 1;
  const scale = Math.max(0, Math.min(1, entry.scale));
  return entry.phase === 'imploding' ? Math.pow(scale, 1.4) : scale;
}

export function radarLinkFadeFactor(source: LinkFadeEndpoint, target: LinkFadeEndpoint): number {
  return Math.min(endpointFadeFactor(source), endpointFadeFactor(target));
}


// ── one live agent globe: WHERE it is, and what it is worth clicking ──────────
/**
 * Where every mounted globe ACTUALLY is this frame, id to world position.
 *
 * The layout hands out target positions; a globe damps toward its target over about
 * 0.75s. Anything that has to stay attached to a globe (today: the parent/child
 * tethers) needs the animated value, not the target, or it arrives first and hangs in
 * space until the globe catches up. Written by each globe in its own `useFrame` and
 * read by `RadarLinks` in the same frame: a plain mutable Map, never state, so
 * publishing a position can never trigger a render.
 */
export type LivePositions = Map<string, { x: number; y: number; z: number }>;

/**
 * One globe on the board.
 *
 * This component owns everything about a globe that is SPECIFIC to the war room:
 * where the layout put it, the spawn/implode lifecycle scale, the idle bob, the
 * invisible hit sphere, and the live-position entry the tethers hang off. How the
 * globe LOOKS is not here at all: `RadarGlobeBody` draws that, and the HUD draws the
 * same body from the same file, which is the only way the two screens can agree.
 */
function RadarGlobe({
  node,
  selected,
  hovered,
  dimmed,
  dimTarget = 0,
  emphasis = false,
  reduced = false,
  interactive = true,
  lifecycleRef,
  livePosRef,
  onHover,
  onLeave,
  onSelect,
}: {
  node: LayoutNode;
  selected: boolean;
  hovered: boolean;
  dimmed: boolean;
  /** Legend colour-only filter, 0 = full colour .. 1 = fully dimmed. Eased per
   *  frame by the body and applied to COLOUR ONLY (never scale/opacity/geometry). */
  dimTarget?: number;
  /** This globe MATCHES the active legend harness filter: a gentle extra glow so the
   *  selection POPS, not just dims everything else. */
  emphasis?: boolean;
  /** prefers-reduced-motion: hold the resting spin rate whatever the status is. */
  reduced?: boolean;
  /** Mount the hit-sphere? A peer's board is a read-only frame, so it raycasts
   *  nothing: no hover, no cursor change, no click, and no raycast cost either. */
  interactive?: boolean;
  /** Live read of the reconciler's per-id scale (no re-render on tween). */
  lifecycleRef: MutableRefObject<LifecycleMap>;
  /** Write-side of the shared live-position registry (see `LivePositions`). */
  livePosRef: MutableRefObject<LivePositions>;
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
}) {
  const group = useRef<THREE.Group>(null!);
  const agent = node.radarAgent!;
  const isRoot = node.depth === 0;
  const seed = useMemo(() => seedOf(node.id), [node.id]);

  const sim = useRef({ scale: 0.0001, pos: { ...node.position } });

  // Publish this globe's ANIMATED position into the shared registry, and take the
  // entry back out when it unmounts so a dead id can never anchor a tether. One
  // object per id, mutated in place: the registry is read every frame by RadarLinks.
  // Keyed on the id ALONE: a re-layout must not re-seed the entry, or the tether would
  // jump to the new target a beat before the globe damps its way there.
  useEffect(() => {
    const reg = livePosRef.current;
    const id = node.id;
    return () => {
      reg.delete(id);
    };
  }, [livePosRef, node.id]);

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    const boost = selected ? 0.22 : hovered ? 0.07 : 0;
    // lifecycle scale (0..1) is the spawn-in / implode-out factor from the pure
    // reconciler, read live from the ref so a tween never triggers a re-render.
    // A missing entry means full scale (a fresh, not-yet-reconciled node) EXCEPT
    // for an already-closed agent: it is dead, so an absent entry (e.g. just pruned
    // as `gone`) reads 0, never a one-frame full-scale flash before it unmounts.
    const lifecycleEntry = lifecycleRef.current[node.id];
    const lifecycleScale = lifecycleEntry?.scale ?? (agent.status === 'closed' || agent.status === 'terminated' ? 0 : 1);
    const targetScale = node.radius * (1 + boost) * Math.max(0, lifecycleScale);

    // spawn eases in fast; implode collapses fast, both damped (never snap).
    const scaleLambda = lifecycleEntry?.phase === 'imploding' ? 18 : lifecycleScale < 0.999 ? 10 : 6;
    s.scale = damp(s.scale, targetScale, scaleLambda, dt);
    // damp the node toward its layout position so re-layouts glide, not jump.
    s.pos.x = damp(s.pos.x, node.position.x, 4, dt);
    s.pos.y = damp(s.pos.y, node.position.y, 4, dt);
    s.pos.z = damp(s.pos.z, node.position.z, 4, dt);

    // The body multiplies its own breath into THIS scale, so the two groups together
    // produce exactly what one group used to.
    group.current.scale.setScalar(s.scale);
    const bobY = s.pos.y + Math.sin(t * 0.6 + seed * 6.28) * 0.05;
    group.current.position.set(s.pos.x, bobY, s.pos.z);
    // The tether reads THIS, not the layout target, so a link stays welded to the two
    // globes it hangs between while they glide (see RadarLinks). Deliberately the
    // DAMPED position without the idle bob: the bob never stops, so publishing it
    // would mark every link dirty on every frame and cost the whole board a buffer
    // upload forever, to chase a 0.05-unit wobble that is inside the globe anyway.
    const slot = livePosRef.current.get(node.id);
    if (slot) {
      slot.x = s.pos.x;
      slot.y = s.pos.y;
      slot.z = s.pos.z;
    } else {
      livePosRef.current.set(node.id, { x: s.pos.x, y: s.pos.y, z: s.pos.z });
    }
  });

  return (
    <group ref={group} position={[node.position.x, node.position.y, node.position.z]}>
      {/* tight invisible hit-sphere: the only interactive object (R3F raycasts
          only handler-bearing meshes); sized inside the lattice so clicks land on
          the globe, not the empty space around it. Dropped entirely on a read-only
          board (a watched peer), so there is nothing to raycast and nothing to click. */}
      {interactive && (
        <mesh
          onPointerOver={(e) => {
            e.stopPropagation();
            document.body.style.cursor = 'pointer';
            onHover(node);
          }}
          onPointerOut={(e) => {
            e.stopPropagation();
            document.body.style.cursor = '';
            onLeave(node);
          }}
          onClick={(e) => {
            e.stopPropagation();
            onSelect(node);
          }}
        >
          <sphereGeometry args={[0.8, 16, 16]} />
          <meshBasicMaterial transparent opacity={0} depthWrite={false} />
        </mesh>
      )}

      <RadarGlobeBody
        id={node.id}
        harness={agent.harness}
        status={agent.status}
        isRoot={isRoot}
        selected={selected}
        hovered={hovered}
        dimmed={dimmed}
        dimTarget={dimTarget}
        emphasis={emphasis}
        reduced={reduced}
      />
    </group>
  );
}

// Mutable scratch endpoints, reused for every link on every frame. The lifecycle
// helpers take a shaped endpoint (readable, and how their tests call them), and this
// is what keeps that from meaning two object literals per link per frame.
const linkSrcScratch: LinkEndpointState = {};
const linkDstScratch: LinkEndpointState = {};

// ── parent -> child glowing links (depth-N), mirroring Habits' AnimatedLinks but
// flowing parent → child and radar-tinted by the PARENT's heat colour.
//
// A link ANIMATES ALONG ITS LENGTH, in lockstep with the child globe: it draws out
// from the parent as the child blooms and is reeled back into the parent as the child
// implodes, so a subagent arrives on a tether and leaves on one instead of a
// full-length line blinking out around it. `linkDrawProgress` (pure, tested) is the
// drawn fraction; the SAME fixed segments are just redistributed over
// [parent, parent + progress * (child - parent)], so nothing is allocated and no
// geometry is rebuilt. Because the colour ramp is not redistributed with them, the
// gradient compresses into the drawn part and the head always carries the child's
// hue, which is what makes the stroke read as travelling toward the child.
//
// Claude agent-teams and Codex subagents are different concepts upstream and share
// this path exactly: nothing below branches on harness.
function RadarLinks({
  layout,
  lifecycleRef,
  goneIdsRef,
  livePosRef,
  reduced = false,
}: {
  layout: OrbLayout;
  lifecycleRef: MutableRefObject<LifecycleMap>;
  goneIdsRef: MutableRefObject<Set<string>>;
  /** Read-side of the live-position registry: where the two globes ACTUALLY are. */
  livePosRef: MutableRefObject<LivePositions>;
  /** prefers-reduced-motion: the link snaps between drawn and absent, never travels. */
  reduced?: boolean;
}) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(() => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)), [layout, byId]);

  // Immutable full-brightness colour base. The live colour attribute is this scaled
  // by each link's endpoint lifecycle factor per frame (scaling the attribute in
  // place would compound; the base never changes after layout).
  const baseLineColors = useRef<Float32Array>(new Float32Array(0));

  // One STRAIGHT parent→child strand per edge, sampled into a short gradient polyline:
  // brightest at the parent anchor and easing toward the child, so the tether reads as
  // a clean, direct "this spawned that" line (no bowed cable, no travelling mote).
  const SEG = 6;
  const { lineGeo, meta, anchors, lastDrawn, lastLit } = useMemo(() => {
    const linePos = new Float32Array(links.length * SEG * 6);
    const lineCol = new Float32Array(links.length * SEG * 6);
    // Both endpoints per link, so the per-frame redraw can re-lerp the strand without
    // walking back into the layout (or allocating a Vector3 to do it).
    const anchors = new Float32Array(links.length * 6);
    // Last progress / brightness actually written, so a settled link costs no writes.
    // -1 is unreachable for both, so the first frame always draws.
    const lastDrawn = new Float32Array(links.length).fill(-1);
    const lastLit = new Float32Array(links.length).fill(-1);
    const a = new THREE.Vector3();
    const b = new THREE.Vector3();
    const meta = links.map((link, idx) => {
      const parent = byId.get(link.source)!;
      const child = byId.get(link.target)!;
      const h = new THREE.Vector3(parent.position.x, parent.position.y, parent.position.z);
      const c = new THREE.Vector3(child.position.x, child.position.y, child.position.z);

      const cParent = new THREE.Color(radarNodeColor(parent.radarAgent!));
      const cChild = new THREE.Color(radarNodeColor(child.radarAgent!));
      const base = idx * SEG * 6;
      const an = idx * 6;
      anchors[an] = h.x; anchors[an + 1] = h.y; anchors[an + 2] = h.z;
      anchors[an + 3] = c.x; anchors[an + 4] = c.y; anchors[an + 5] = c.z;
      for (let s = 0; s < SEG; s++) {
        const ta = s / SEG;
        const tb = (s + 1) / SEG;
        a.copy(h).lerp(c, ta);
        b.copy(h).lerp(c, tb);
        // brightest at the parent anchor, easing toward the child globe.
        const ca = cParent.clone().lerp(cChild, ta).multiplyScalar(0.92 - 0.3 * ta);
        const cb = cParent.clone().lerp(cChild, tb).multiplyScalar(0.92 - 0.3 * tb);
        const o = base + s * 6;
        linePos[o] = a.x; linePos[o + 1] = a.y; linePos[o + 2] = a.z;
        linePos[o + 3] = b.x; linePos[o + 4] = b.y; linePos[o + 5] = b.z;
        lineCol[o] = ca.r; lineCol[o + 1] = ca.g; lineCol[o + 2] = ca.b;
        lineCol[o + 3] = cb.r; lineCol[o + 4] = cb.g; lineCol[o + 5] = cb.b;
      }
      return { sourceId: link.source, targetId: link.target };
    });

    baseLineColors.current = lineCol.slice();
    const lineGeo = new THREE.BufferGeometry();
    lineGeo.setAttribute('position', new THREE.BufferAttribute(linePos, 3));
    lineGeo.setAttribute('color', new THREE.BufferAttribute(lineCol, 3));
    return { lineGeo, meta, anchors, lastDrawn, lastLit };
  }, [links, byId]);

  const lineMat = useRef<THREE.LineBasicMaterial>(null);

  useEffect(() => () => { lineGeo.dispose(); }, [lineGeo]);

  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const lc = lifecycleRef.current;
    const gone = goneIdsRef.current;
    const livePos = livePosRef.current;
    const posAttr = lineGeo.getAttribute('position') as THREE.BufferAttribute;
    const colAttr = lineGeo.getAttribute('color') as THREE.BufferAttribute;
    const posArr = posAttr.array as Float32Array;
    const colArr = colAttr.array as Float32Array;
    const baseLine = baseLineColors.current;
    const stride = SEG * 6;
    let posDirty = false;
    let colDirty = false;

    for (let i = 0; i < meta.length; i++) {
      const m = meta[i];
      linkSrcScratch.entry = lc[m.sourceId];
      linkSrcScratch.gone = gone.has(m.sourceId);
      linkDstScratch.entry = lc[m.targetId];
      linkDstScratch.gone = gone.has(m.targetId);

      // LENGTH: how far along the parent→child vector the stroke currently reaches.
      const drawn = linkDrawProgress(linkSrcScratch, linkDstScratch, reduced);
      const l = i * stride;
      const an = i * 6;
      // WHERE the two ends are. The anchors baked at layout time are the globes'
      // TARGET positions, but a globe damps its way there over about 0.75s, so a
      // board that reflows (a subagent finishes, its siblings close ranks) drew every
      // tether snapped to where its globes were going while the globes were still on
      // their way. Reading the live registry welds the line to the actual globes; the
      // baked anchor stays as the fallback for a node that has not rendered a frame.
      const src = livePos.get(m.sourceId);
      const dst = livePos.get(m.targetId);
      const ax = src ? src.x : anchors[an];
      const ay = src ? src.y : anchors[an + 1];
      const az = src ? src.z : anchors[an + 2];
      const bx = dst ? dst.x : anchors[an + 3];
      const by = dst ? dst.y : anchors[an + 4];
      const bz = dst ? dst.z : anchors[an + 5];
      // `anchors` doubles as "what we last drew", so this stays a pure change check:
      // a settled board still uploads nothing.
      const moved =
        ax !== anchors[an] ||
        ay !== anchors[an + 1] ||
        az !== anchors[an + 2] ||
        bx !== anchors[an + 3] ||
        by !== anchors[an + 4] ||
        bz !== anchors[an + 5];
      if (lastDrawn[i] !== drawn || moved) {
        lastDrawn[i] = drawn;
        anchors[an] = ax; anchors[an + 1] = ay; anchors[an + 2] = az;
        anchors[an + 3] = bx; anchors[an + 4] = by; anchors[an + 5] = bz;
        const dx = (bx - ax) * drawn;
        const dy = (by - ay) * drawn;
        const dz = (bz - az) * drawn;
        for (let s = 0; s < SEG; s++) {
          const ta = s / SEG;
          const tb = (s + 1) / SEG;
          const k = l + s * 6;
          posArr[k] = ax + dx * ta; posArr[k + 1] = ay + dy * ta; posArr[k + 2] = az + dz * ta;
          posArr[k + 3] = ax + dx * tb; posArr[k + 4] = ay + dy * tb; posArr[k + 5] = az + dz * tb;
        }
        posDirty = true;
      }

      // BRIGHTNESS: the surviving stroke also dims with whichever endpoint globe is
      // shrinking, so a retracting tether goes out as it goes home rather than
      // snapping dark at the last frame. Live link (both alive) → factor 1 → untouched.
      const lit = radarLinkFadeFactor(linkSrcScratch, linkDstScratch);
      if (lastLit[i] !== lit) {
        lastLit[i] = lit;
        for (let k = 0; k < stride; k++) colArr[l + k] = baseLine[l + k] * lit;
        colDirty = true;
      }
    }
    // Only flag a re-upload when something actually moved: a settled board (every
    // link fully drawn, nothing spawning or imploding) uploads zero bytes per frame.
    if (posDirty) posAttr.needsUpdate = true;
    if (colDirty) colAttr.needsUpdate = true;
    // The slow opacity breath is motion for its own sake, so reduced motion holds it.
    if (lineMat.current) lineMat.current.opacity = reduced ? 0.42 : 0.4 + Math.sin(t * 1.3) * 0.07;
  });

  if (links.length === 0) return null;
  return (
    <group>
      <lineSegments geometry={lineGeo}>
        <lineBasicMaterial ref={lineMat} vertexColors transparent opacity={0.42} depthWrite={false} toneMapped={false} blending={THREE.AdditiveBlending} />
      </lineSegments>
    </group>
  );
}

export type RadarConstellationProps = {
  model: RadarSceneModel;
  hoveredId: string | null;
  selectedId: string | null;
  /** Active legend filter; each globe's colour-only `dimTarget` is derived from it.
   *  Harness filters apply on the radar tab; null (the default) leaves every globe at
   *  full colour. Optional so the standalone dev harness need not thread it. */
  emphasisFilter?: EmphasisFilter;
  /** Live fold scale for the constellation swap (1 = at rest). Omitted in the dev harness. */
  scaleRef?: { current: number };
  /** Read-only board (a watched peer): no hit-spheres, no hover card, no rail clicks. */
  interactive?: boolean;
  /**
   * The board's layout, when the caller has already computed it.
   *
   * `layoutRadarScene` is pure over the model, so calling it here AND in the lead
   * (which needs the same nodes for `sceneBounds`, `selectedNode` and
   * `subtreeBounds`) computed the identical board twice on every emit, and a third
   * and fourth time per emit while a peer board was mounted. That is pure waste, and
   * it lands as a hitch INSIDE the 700ms camera fly, which is what a laggy zoom
   * actually is. Passing the lead's layout down removes the duplicates without
   * changing what is drawn: same function, same input, same board.
   */
  layout?: RadarLayout;
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
  /** Click a folder tag to frame that rail. Omitted by the dev harness. */
  onPickFolder?: (key: string) => void;
};

export function radarModelWithoutGone(model: RadarSceneModel, goneIds: ReadonlySet<string>): RadarSceneModel {
  if (goneIds.size === 0) return model;
  const agents = model.agents.filter((a) => !goneIds.has(a.id));
  return agents.length === model.agents.length ? model : { ...model, agents };
}

// Steps the PURE lifecycle reconciler once per frame into a ref the globes read
// live (no re-render on tween). Must live inside the Canvas to get useFrame's dt.
//
// After reconciling it PRUNES fully-collapsed (`gone`) entries so a node unmounts
// promptly the frame it finishes imploding instead of lingering (invisible, but
// still a mounted globe with a hit-sphere) until the next model emit. The mount set
// is driven off the React tree, so the only re-render trigger is `onRenderSetChange`
// — fired ONLY when the set of gone/ghost ids actually changes (≈ once per node
// death, never per frame), keeping the tween path itself re-render-free.
function LifecycleDriver({
  live,
  mapRef,
  goneIdsRef,
  onRenderSetChange,
}: {
  live: LiveId[];
  mapRef: MutableRefObject<LifecycleMap>;
  /** Ids whose globe should unmount this frame (finished imploding). */
  goneIdsRef: MutableRefObject<Set<string>>;
  onRenderSetChange: () => void;
}) {
  const sigRef = useRef('');
  useFrame((_, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const reconciled = reconcileLifecycle(mapRef.current, live, dt);

    // ids that just finished imploding (drop them from the mount set) + ids still
    // mid-implosion that are no longer live (ghosts kept mounted to finish the anim).
    const liveSet = new Set(live.map((l) => l.id));
    const gone = new Set<string>();
    const renderable: string[] = [];
    for (const id in reconciled) {
      const phase = reconciled[id].phase;
      if (phase === 'gone') gone.add(id);
      else if (!liveSet.has(id)) renderable.push(id); // a mounted ghost
    }

    mapRef.current = pruneGone(reconciled); // drop gone now → prompt unmount
    goneIdsRef.current = gone;

    // re-render only when the mounted-ghost/gone signature changes (node birth or
    // death), never on every tween frame.
    const sig = `${[...gone].sort().join(',')}|${renderable.sort().join(',')}`;
    if (sig !== sigRef.current) {
      sigRef.current = sig;
      onRenderSetChange();
    }
  });
  return null;
}

// Screen-space hover quick-glance card, pinned to the hovered globe via drei
// <Html> (the same node-anchored, constant-pixel-size overlay pattern WarRoom uses
// for its hub labels). Sits a touch above the globe and never captures pointer
// events, so the orbit camera and the globe's own hit-sphere stay fully reachable.
// Hidden while that same globe is the active selection — the detail panel owns the
// readout then, and a card stacked over the dimmed globe would just be noise.
function RadarHoverLayer({ node, suppressed }: { node: LayoutNode | null; suppressed: boolean }) {
  if (!node || suppressed) return null;
  const agent = node.radarAgent;
  if (!agent) return null;
  // lift the card above the globe by its layout radius so it clears the lattice.
  const lift = Math.max(0.6, node.radius) + 0.5;
  return (
    <Html
      position={[node.position.x, node.position.y + lift, node.position.z]}
      center
      zIndexRange={[8, 0]}
      style={{ pointerEvents: 'none' } as CSSProperties}
    >
      <RadarHoverCard agent={agent} />
    </Html>
  );
}

// Per-FOLDER rail titles: a well-set title at each rail head naming the project that
// rail belongs to. Right-aligned (via CSS) so the title ENDS just left of the first
// bead and never stabs through the beads. A path label is split into a dim parent
// prefix + a bright basename so the project name reads as the title; the harness glyph
// is a small colour accent (colour + glyph + text = color-blind a11y). Click frames
// the rail (onPick); the Html wrapper stays pass-through so it never eats a globe click.
function RadarClusterLabels({ clusters, onPick }: { clusters: RadarCluster[]; onPick?: (key: string) => void }) {
  return (
    <>
      {clusters.map((c) => {
        const t = radarHarness(c.harness);
        const parts = c.label.split('/').filter(Boolean);
        const name = parts.length ? parts[parts.length - 1] : c.label;
        const prefix = parts.length > 1 ? parts.slice(0, -1).join('/') + '/' : '';
        return (
          <Html
            key={`cluster-${c.key}`}
            position={[c.center.x, c.center.y, c.center.z]}
            zIndexRange={[6, 0]}
            style={{ pointerEvents: 'none' } as CSSProperties}
          >
            <div
              className="wd-rail-title"
              style={{ '--harness': t.color, pointerEvents: onPick ? 'auto' : 'none', cursor: onPick ? 'pointer' : 'default' } as CSSProperties}
              onClick={onPick ? () => onPick(c.key) : undefined}
              role={onPick ? 'button' : undefined}
              title={onPick ? `Focus ${c.label}` : undefined}
            >
              <span className="wd-rail-title-glyph" aria-hidden="true">{t.glyph}</span>
              <span className="wd-rail-title-text">
                {prefix ? <span className="wd-rail-title-prefix">{prefix}</span> : null}
                <span className="wd-rail-title-name">{name}</span>
              </span>
            </div>
          </Html>
        );
      })}
    </>
  );
}

// The DATA forest only — live globes, parent→child links, lifecycle + hover, wrapped
// in the fold group. It carries NO background/lights/camera/post: those live once in
// the persistent scene shell (WarRoom's SceneShell), so a Habits↔Radar swap only ever
// remounts this forest (already folded to nothing) and the void never flickers. The
// standalone dev harness wraps this in `RadarSceneBody`, which adds its own shell.
export function RadarForest({ model, hoveredId, selectedId, emphasisFilter = null, scaleRef, interactive = true, layout: providedLayout, onHover, onLeave, onSelect, onClear, onPickFolder }: RadarConstellationProps) {
  // The dev harness mounts the radar without a fold; default to a stable scale-1 ref.
  const fallbackScale = useRef(1);
  const sref = scaleRef ?? fallbackScale;
  // One live subscription for the whole forest (never one per globe), passed down as
  // a plain boolean so a mid-session flip of the OS setting re-renders every node.
  const reduced = useReducedMotion();
  // Severity buckets are a Habits-only concept (radar globes carry no issue severity),
  // so only a HARNESS filter dims radar globes; a severity filter is a no-op here. The
  // colour-only dim itself is computed by the shared pure `emphasis.targetDim`.
  const radarFilter: EmphasisFilter = emphasisFilter?.kind === 'harness' ? emphasisFilter : null;

  // Persistent lifecycle map (frame-stepped) + a cache of the last layout node per
  // id, so an imploding agent keeps rendering at its last position until it has
  // fully collapsed-into-self (spec §8 — removals never just pop out).
  const lifecycleRef = useRef<LifecycleMap>({});
  // Ids that finished imploding this frame — filtered out of the mount set so a
  // `gone` globe (e.g. a closed agent still listed in `model.agents`) unmounts
  // promptly. `renderTick` is bumped by the driver ONLY when this set (or the live
  // ghost set) changes, so the unmount happens without waiting for the next emit.
  const goneIdsRef = useRef<Set<string>>(new Set());
  const [renderTick, setRenderTick] = useState(0);
  const nodeCache = useRef<Map<string, LayoutNode>>(new Map());
  // Live animated positions, published by the globes and read by the tethers.
  const livePosRef = useRef<LivePositions>(new Map());
  // ONE layout for the whole app. It used to be computed here over a locally-filtered
  // model (`radarModelWithoutGone`), which quietly made the forest's geometry a
  // different board from the one every consumer OUTSIDE the canvas computes (WarRoom
  // derives `selectedNode`, `sceneBounds` and `subtreeBounds` from its own call). The
  // divergence was not cosmetic: `placeSubtree` centres a parent over its children, so
  // the frame a subagent finished, dropping it re-centred its parent sideways, and the
  // camera framed a position the globe no longer occupied (measured: a root sliding
  // 0.88 world units, about 235px, straight under the fleet rack).
  //
  // The rule that came out of that is one board, and it is why the terminal-agent
  // filter now lives INSIDE `layoutRadarScene` (`boardAgents`) rather than here: every
  // caller reflows on the same frame, so the camera still frames what is on screen.
  // Unmounting is still a separate concern: `renderNodes` below filters `goneIdsRef`
  // out of the mount set, which is what actually removes a globe and its hit-sphere.
  const ownLayout = useMemo(
    () => (providedLayout ? null : layoutRadarScene(model)),
    [providedLayout, model],
  );
  const layout = providedLayout ?? (ownLayout as RadarLayout);
  // Intentional mid-render write: append-only + idempotent. We record each live
  // node's latest layout so an imploding node keeps its last position after it
  // leaves `model.agents`. Writing the same id twice with the current layout is a
  // no-op replacement (never stale — re-runs use the same `layout`), so this is
  // safe under React's double-invoked render. Entries are pruned in LifecycleDriver.
  for (const n of layout.nodes) nodeCache.current.set(n.id, n);

  const live = useMemo<LiveId[]>(
    () => model.agents.map((a) => ({ id: a.id, status: a.status })),
    [model],
  );

  // Render the live nodes PLUS any cached node still mid-implosion (present in the
  // lifecycle map, not yet `gone`, and no longer in the live layout).
  const liveIds = useMemo(() => new Set(layout.nodes.map((n) => n.id)), [layout]);
  // `renderTick` is a dep so a gone/ghost transition between emits re-runs this.
  const ghostNodes = useMemo(() => {
    const ghosts: LayoutNode[] = [];
    for (const [id, entry] of Object.entries(lifecycleRef.current)) {
      if (liveIds.has(id) || !isVisible(entry)) continue;
      const cached = nodeCache.current.get(id);
      if (cached) ghosts.push(cached);
    }
    return ghosts;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveIds, model, renderTick]);

  // Drop any layout node that has finished imploding (`gone`) so its globe — and
  // its hit-sphere — unmount immediately, even for a closed agent still present in
  // `model.agents`. Imploding (mid-collapse) nodes are NOT gone, so they stay.
  const renderNodes = useMemo(
    () => [...layout.nodes.filter((n) => !goneIdsRef.current.has(n.id)), ...ghostNodes],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layout, ghostNodes, renderTick],
  );
  // A ghost has left the layout, so its tether left `layout.links` with it, and the
  // stroke would blink out instead of being REELED HOME into the parent (spec §8).
  // Rebuild that one edge from the ghost's own cached agent while both ends are still
  // on screen; `linkDrawProgress` then retracts it exactly as before.
  const renderLayout = useMemo(() => {
    if (ghostNodes.length === 0) return { ...layout, nodes: renderNodes };
    const mounted = new Set(renderNodes.map((n) => n.id));
    const ghostLinks: OrbLink[] = [];
    for (const g of ghostNodes) {
      const parentId = g.radarAgent?.parentId;
      if (parentId && mounted.has(parentId)) {
        ghostLinks.push({ source: parentId, target: g.id, kind: 'agent_issue' });
      }
    }
    return { ...layout, nodes: renderNodes, links: [...layout.links, ...ghostLinks] };
  }, [layout, ghostNodes, renderNodes]);
  // Pin the hover card to whichever globe is currently rendered (live or a
  // still-imploding ghost) so it tracks the node even mid-lifecycle.
  const hoveredNode = useMemo(
    () => renderNodes.find((n) => n.id === hoveredId) ?? null,
    [renderNodes, hoveredId],
  );

  return (
    <>
      <LifecycleDriver
        live={live}
        mapRef={lifecycleRef}
        goneIdsRef={goneIdsRef}
        onRenderSetChange={() => setRenderTick((v) => v + 1)}
      />

      {/* The whole forest folds as one on a tab swap (Transition.tsx). */}
      <FoldGroup scaleRef={sref}>
        <group onPointerMissed={onClear}>
          {/* the ONLY linking cue: a subtle parent -> child tether per subagent,
              drawn out and reeled back in with the child it belongs to */}
          <RadarLinks layout={renderLayout} lifecycleRef={lifecycleRef} goneIdsRef={goneIdsRef} livePosRef={livePosRef} reduced={reduced} />
          {renderNodes.map((node) => (
            <RadarGlobe
              key={node.id}
              node={node}
              selected={selectedId === node.id}
              hovered={hoveredId === node.id}
              dimmed={Boolean(selectedId && selectedId !== node.id)}
              // Harness legend filter → colour-only dim (severity is Habits-only, so
              // `radarFilter` is null for a severity chip → every dimTarget 0).
              dimTarget={targetDim({ harness: node.harness }, radarFilter)}
              // …and a matching globe POPS (gentle extra glow) rather than only the
              // others dimming, so the selection reads as "these light up".
              emphasis={radarFilter !== null && matchesFilter({ harness: node.harness }, radarFilter)}
              reduced={reduced}
              interactive={interactive}
              lifecycleRef={lifecycleRef}
              livePosRef={livePosRef}
              onHover={onHover}
              onLeave={onLeave}
              onSelect={onSelect}
            />
          ))}
        </group>

        {/* one "this is the WARDEN folder" tag at each rail head; click frames the rail */}
        <RadarClusterLabels clusters={layout.clusters} onPick={interactive ? onPickFolder : undefined} />

        <RadarHoverLayer node={hoveredNode} suppressed={!interactive || Boolean(hoveredId && hoveredId === selectedId)} />
      </FoldGroup>
    </>
  );
}

// The full radar scene body (shell + forest) — used ONLY by the standalone dev
// harness `<Canvas>`. In the live app the forest renders inside WarRoom's shared
// SceneShell instead (so the void persists across the Habits↔Radar swap).
export function RadarSceneBody(props: RadarConstellationProps) {
  const { gl } = useThree();
  useEffect(() => {
    gl.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    gl.toneMapping = THREE.ACESFilmicToneMapping;
    gl.toneMappingExposure = 1.05;
  }, [gl]);

  const layout = useMemo(() => layoutRadarScene(props.model), [props.model]);
  const selectedNode = useMemo(
    () => layout.nodes.find((n) => n.id === props.selectedId) ?? null,
    [layout, props.selectedId],
  );

  return (
    <>
      <color attach="background" args={[BG]} />
      <fogExp2 attach="fog" args={[BG, 0.014]} />

      <ambientLight intensity={0.085} />
      <directionalLight position={[5, 6, 4]} intensity={2.1} color="#fff3e9" />
      <directionalLight position={[-6, -1, -2]} intensity={0.65} color="#bfe2ff" />
      <Environment resolution={128}>
        {/* warm + cool formers so Claude tangerine + Codex cyan gems both glint */}
        <Lightformer form="rect" intensity={1.7} color="#ffcaa0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.4} color="#bfeaff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="ring" intensity={1.2} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      <StarCatalog />
      <CameraRig selected={selectedNode} />

      <RadarForest {...props} />

      <EffectComposer multisampling={4}>
        <Bloom intensity={1.3} luminanceThreshold={0.22} luminanceSmoothing={0.9} mipmapBlur radius={0.85} />
        <Vignette eskil={false} offset={0.22} darkness={0.95} />
      </EffectComposer>
    </>
  );
}

/**
 * Standalone Radar constellation in its OWN <Canvas> — used by the dev harness
 * (Task 23). In the live app the body renders inside WarRoom's shared Canvas.
 */
export function RadarConstellation(props: RadarConstellationProps & { active?: boolean }) {
  const active = props.active ?? true;
  return (
    <Canvas
      dpr={[1, 2]}
      frameloop={frameloopFor(!active)}
      gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
      // Opening pose anchored on the shared radar overview (useOrbCamera); the
      // CameraRig takes over for free-orbit + the click-to-focus dive.
      camera={radarCanvasCamera()}
    >
      <RadarSceneBody {...props} />
    </Canvas>
  );
}

export default RadarConstellation;
