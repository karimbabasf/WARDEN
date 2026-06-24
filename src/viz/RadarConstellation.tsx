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
import { Environment, Lightformer, Wireframe, Html } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode, OrbLayout } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { layoutRadarScene } from './radarLayout';
import { radarHarness, heatColor } from './radarTheme';
import { AgentCore } from './AgentCore';
import { StarCatalog } from './StarCatalog';
import { FoldGroup } from './Transition';
import { CameraRig } from './CameraRig';
import { frameloopFor } from './WarRoom';
import { reconcileLifecycle, pruneGone, isVisible, type LifecycleMap, type LiveId } from './radarLifecycle';
import { RadarHoverCard } from './RadarHoverCard';
import { radarCanvasCamera } from './useOrbCamera';

const BG = '#020403';
const WHITE = new THREE.Color('#ffffff');

/**
 * The colour of a radar globe: the agent's harness hue heated by its fill level.
 * Pure + exported so it is unit-tested without WebGL (the house pattern).
 */
export function radarNodeColor(agent: RadarAgent): string {
  return heatColor(radarHarness(agent.harness).color, agent.fillPct);
}

function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.lerp(current, target, 1 - Math.exp(-lambda * dt));
}

// ~300 ms time-constant for the legend colour-dim crossfade. `damp(.., DIM_LAMBDA, dt)`
// equals the brief's `cur += (target-cur)*(1 - exp(-dt/0.3))` (lambda = 1/0.3).
const DIM_LAMBDA = 1 / 0.3;

// Eased dim float -> a multiplicative colour scale: 1.0 at dim=0 (untouched),
// `floor` at dim=1 (matching the old boolean `multiplyScalar` endpoints). Colour
// only — callers copy a base colour, scale it, write it back; opacity/scale/
// geometry/position are never touched.
function dimScale(dim: number, floor: number): number {
  return 1 - dim * (1 - floor);
}

// drei's <Wireframe geometry=..> renders a private <mesh><meshWireframeMaterial/>
// and forwards no material ref, so cache the MeshWireframeMaterial (a ShaderMaterial
// whose `stroke`/`fill` colour uniforms we tint per frame) by traversing a wrapper
// group once.
function findWireframeMaterial(root: THREE.Object3D | null): THREE.ShaderMaterial | null {
  if (!root) return null;
  let found: THREE.ShaderMaterial | null = null;
  root.traverse((o) => {
    if (found) return;
    const mat = (o as THREE.Mesh).material as THREE.ShaderMaterial | undefined;
    if (mat?.uniforms?.stroke?.value instanceof THREE.Color) found = mat;
  });
  return found;
}

function seedOf(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 1000;
  return h / 1000;
}

// soft round sprite (cached) — the gem halo + the travelling link dots.
function radialTexture(size: number, stops: Array<[number, number]>): THREE.Texture {
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  for (const [at, a] of stops) g.addColorStop(at, `rgba(255,255,255,${a})`);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  const tex = new THREE.CanvasTexture(c);
  tex.needsUpdate = true;
  return tex;
}
let glowCache: THREE.Texture | null = null;
const glowTexture = () => (glowCache ??= radialTexture(128, [[0, 1], [0.18, 0.8], [0.5, 0.22], [1, 0]]));
let dotCache: THREE.Texture | null = null;
const dotTexture = () => (dotCache ??= radialTexture(48, [[0, 1], [0.4, 0.75], [1, 0]]));

// ── one live agent globe — lattice shell + crystal heart, heat-coloured ────────
function RadarGlobe({
  node,
  selected,
  hovered,
  dimmed,
  dimTarget = 0,
  lifecycleRef,
  onHover,
  onLeave,
  onSelect,
}: {
  node: LayoutNode;
  selected: boolean;
  hovered: boolean;
  dimmed: boolean;
  /** Legend colour-only filter, 0 = full colour .. 1 = fully dimmed. Eased per
   *  frame and applied to COLOUR ONLY (never scale/opacity/geometry). Defaults to
   *  0 so the project type-checks before Task 9 wires it. */
  dimTarget?: number;
  /** Live read of the reconciler's per-id scale (no re-render on tween). */
  lifecycleRef: MutableRefObject<LifecycleMap>;
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
}) {
  const group = useRef<THREE.Group>(null!);
  const innerCage = useRef<THREE.Group>(null!);
  const gem = useRef<THREE.Group>(null!);
  const gemMat = useRef<THREE.MeshPhysicalMaterial>(null!);
  const haloMat = useRef<THREE.SpriteMaterial>(null!);
  const nodeMat = useRef<THREE.PointsMaterial>(null!);
  // Wrapper groups around the drei <Wireframe>s, traversed once to cache the
  // MeshWireframeMaterial so the eased dim can tint its colour uniforms per frame.
  const shellGroup = useRef<THREE.Group>(null!);
  const innerGroup = useRef<THREE.Group>(null!);
  const shellMat = useRef<THREE.ShaderMaterial | null>(null);
  const cageMat = useRef<THREE.ShaderMaterial | null>(null);

  const agent = node.radarAgent!;
  const isRoot = node.depth === 0;
  const working = agent.status === 'working';
  const seed = useMemo(() => seedOf(node.id), [node.id]);

  // Colour depends only on harness hue + fill heat; `normalizeRadarState` rebuilds
  // the whole `agent` object every emit, so key on those two fields (not identity)
  // to avoid rebuilding the THREE.Color (and its derived clones) on every frame.
  const color = useMemo(() => new THREE.Color(radarNodeColor(agent)), [agent.harness, agent.fillPct]);
  const innerColor = useMemo(() => color.clone().lerp(WHITE, 0.24), [color]);
  const nodeColor = useMemo(() => color.clone().lerp(WHITE, 0.16), [color]);

  // Shell/inner BASE colour (hover/select + the boolean `dimmed`). The eased
  // legend dim multiplies ON TOP of these every frame in useFrame — colour only.
  const shellBase = useMemo(() => {
    const c = color.clone();
    if (dimmed) c.multiplyScalar(0.42);
    else if (selected || hovered) c.lerp(WHITE, 0.18);
    return c;
  }, [color, dimmed, selected, hovered]);
  const innerBase = useMemo(() => innerColor.clone().multiplyScalar(dimmed ? 0.45 : 1), [innerColor, dimmed]);
  const shellStroke = useMemo(() => `#${shellBase.getHexString()}`, [shellBase]);
  const innerStroke = useMemo(() => `#${innerBase.getHexString()}`, [innerBase]);

  const outerGeo = useMemo(() => new THREE.IcosahedronGeometry(1, isRoot ? 2 : 1), [isRoot]);
  const innerGeo = useMemo(() => new THREE.IcosahedronGeometry(0.6, 1), []);
  const gemGeo = useMemo(() => new THREE.IcosahedronGeometry(0.26, 0), []);
  const nodeGeo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', (outerGeo.attributes.position as THREE.BufferAttribute).clone());
    return g;
  }, [outerGeo]);
  const glowTex = useMemo(() => glowTexture(), []);
  const dotTex = useMemo(() => dotTexture(), []);

  useEffect(
    () => () => {
      outerGeo.dispose();
      innerGeo.dispose();
      gemGeo.dispose();
      nodeGeo.dispose();
    },
    [outerGeo, innerGeo, gemGeo, nodeGeo],
  );

  const sim = useRef({ scale: 0.0001, glow: 0.5, dim: 0, colorDim: 0, pos: { ...node.position } });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    // breathing: working = a faint quick shimmer; idle = a slow, deeper breath.
    const breatheAmp = working ? 0.02 : 0.045;
    const breatheRate = working ? 1.6 : 0.5;
    const breathe = 1 + Math.sin(t * breatheRate + seed * 6.28) * breatheAmp;
    const boost = selected ? 0.22 : hovered ? 0.07 : 0;

    // lifecycle scale (0..1) is the spawn-in / implode-out factor from the pure
    // reconciler — read live from the ref so a tween never triggers a re-render.
    // A missing entry means full scale (a fresh, not-yet-reconciled node) EXCEPT
    // for an already-closed agent: it is dead, so an absent entry (e.g. just pruned
    // as `gone`) reads 0, never a one-frame full-scale flash before it unmounts.
    const lifecycleScale = lifecycleRef.current[node.id]?.scale ?? (agent.status === 'closed' ? 0 : 1);
    const targetScale = node.radius * (1 + boost) * Math.max(0, lifecycleScale);
    const fillGlow = 0.25 + agent.fillPct * 0.65; // fuller = hotter core
    const idleDim = working ? 0 : 0.28; // idle agents read dimmer (at-a-glance who's thinking)
    const targetGlow = (isRoot ? 0.8 : 0.55) + fillGlow - idleDim + (selected ? 0.9 : hovered ? 0.35 : 0);
    const targetDim = dimmed ? 1 : 0;
    // Legend colour-dim: dims for the legend filter OR the boolean other-selected
    // state, whichever is stronger — one eased float, colour only.
    const targetColorDim = Math.max(targetDim, Math.min(1, Math.max(0, dimTarget)));

    // spawn eases in fast; implode collapses fast — both damped (never snap).
    const scaleLambda = lifecycleScale < 0.999 ? 10 : 6;
    s.scale = damp(s.scale, targetScale, scaleLambda, dt);
    s.glow = damp(s.glow, targetGlow, 5, dt);
    s.dim = damp(s.dim, targetDim, 6, dt);
    s.colorDim = damp(s.colorDim, targetColorDim, DIM_LAMBDA, dt);
    // damp the node toward its layout position so re-layouts glide, not jump.
    s.pos.x = damp(s.pos.x, node.position.x, 4, dt);
    s.pos.y = damp(s.pos.y, node.position.y, 4, dt);
    s.pos.z = damp(s.pos.z, node.position.z, 4, dt);

    group.current.scale.setScalar(s.scale * breathe);
    group.current.position.set(s.pos.x, s.pos.y + Math.sin(t * 0.6 + seed * 6.28) * 0.05, s.pos.z);
    group.current.rotation.y += dt * (isRoot ? 0.08 : 0.14);

    innerCage.current.rotation.y -= dt * 0.18;
    innerCage.current.rotation.x += dt * 0.1;
    gem.current.rotation.y += dt * 0.28;
    gem.current.rotation.x += dt * 0.12;

    // dimK (opacity/intensity) stays bound to the boolean-dim track only — the
    // legend colour-dim must NOT change opacity, so it is deliberately excluded.
    const dimK = 1 - s.dim * 0.6;
    gemMat.current.emissiveIntensity = (0.55 + s.glow * 0.6) * dimK;
    haloMat.current.opacity = (0.2 + s.glow * 0.28) * dimK;
    nodeMat.current.opacity = (0.45 + s.glow * 0.32) * dimK;

    // ── eased legend dim, COLOUR ONLY ───────────────────────────────────────
    // Copy each material's base colour, scale by the eased dim, write it back
    // (copy-then-scale so the dim never compounds frame to frame).
    const shellScaleC = dimScale(s.colorDim, 0.42);
    const innerScaleC = dimScale(s.colorDim, 0.45);
    if (!shellMat.current) shellMat.current = findWireframeMaterial(shellGroup.current);
    if (!cageMat.current) cageMat.current = findWireframeMaterial(innerGroup.current);
    if (shellMat.current) {
      shellMat.current.uniforms.stroke.value.copy(shellBase).multiplyScalar(shellScaleC);
    }
    if (cageMat.current) {
      const u = cageMat.current.uniforms;
      u.stroke.value.copy(innerBase).multiplyScalar(innerScaleC);
      u.fill.value.copy(shellBase).multiplyScalar(shellScaleC); // fill reuses shell colour
    }
    nodeMat.current.color.copy(nodeColor).multiplyScalar(shellScaleC);
    haloMat.current.color.copy(color).multiplyScalar(shellScaleC);
    gemMat.current.emissive.copy(color).multiplyScalar(shellScaleC);
  });

  return (
    <group ref={group} position={[node.position.x, node.position.y, node.position.z]}>
      {/* tight invisible hit-sphere — the only interactive object (R3F raycasts
          only handler-bearing meshes); sized inside the lattice so clicks land on
          the globe, not the empty space around it. */}
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

      {/* outer glowing network shell — root = solid lattice, sub = dashed */}
      <group ref={shellGroup}>
        <Wireframe
          geometry={outerGeo}
          simplify
          stroke={shellStroke}
          thickness={isRoot ? 0.02 : 0.016}
          dash={!isRoot}
          dashRepeats={isRoot ? 1 : 4}
          fillOpacity={0}
          backfaceStroke="#06150d"
        />
      </group>

      <group ref={innerCage}>
        <group ref={innerGroup}>
          <Wireframe geometry={innerGeo} simplify stroke={innerStroke} thickness={0.022} fill={shellStroke} fillOpacity={0.035} />
        </group>
      </group>

      <points geometry={nodeGeo}>
        <pointsMaterial
          ref={nodeMat}
          size={isRoot ? 0.08 : 0.07}
          map={dotTex}
          color={nodeColor}
          transparent
          opacity={0.6}
          toneMapped={false}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          sizeAttenuation
        />
      </points>

      <group ref={gem}>
        <sprite scale={isRoot ? 0.62 : 0.5}>
          <spriteMaterial
            ref={haloMat}
            map={glowTex}
            color={color}
            transparent
            opacity={0.2}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </sprite>
        <mesh geometry={gemGeo}>
          <meshPhysicalMaterial
            ref={gemMat}
            color="#05120b"
            emissive={color}
            emissiveIntensity={0.5}
            metalness={0}
            roughness={0.22}
            transmission={0.55}
            thickness={0.6}
            ior={1.45}
            transparent
            envMapIntensity={1.1}
            flatShading
          />
        </mesh>
      </group>

      {/* orchestrator signature — a ROOT agent (depth 0) is the one spawning the
          orbiting subagent moons, so it wears the same gyro cradle + brand heart as
          its Habits hub. Subagents stay bare lattices. Heat-coloured to match. */}
      {isRoot && (
        <AgentCore harness={agent.harness} color={color} dimmed={dimmed} active={selected || hovered} />
      )}
    </group>
  );
}

// ── parent -> child glowing links (depth-N), mirroring Habits' AnimatedLinks but
// flowing parent → child and radar-tinted by the PARENT's heat colour. Each link's
// brightness is multiplied every frame by min(parentScale, childScale) read live
// from the SAME lifecycle map the globes use, so a link to an imploding/gone globe
// fades out in lockstep with it (no dangling full-brightness glow to an empty point).
function RadarLinks({ layout, lifecycleRef }: { layout: OrbLayout; lifecycleRef: MutableRefObject<LifecycleMap> }) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(() => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)), [layout, byId]);

  // Immutable full-brightness colour bases. The live color attributes are these
  // scaled by each link's endpoint lifecycle factor per frame (multiplying the
  // attribute in place would compound; the base never changes after layout).
  const baseLineColors = useRef<Float32Array>(new Float32Array(0));
  const baseDotColors = useRef<Float32Array>(new Float32Array(0));

  const lineGeo = useMemo(() => {
    const positions = new Float32Array(links.length * 6);
    const colors = new Float32Array(links.length * 6);
    links.forEach((link, i) => {
      const parent = byId.get(link.source)!;
      const child = byId.get(link.target)!;
      positions.set(
        [parent.position.x, parent.position.y, parent.position.z, child.position.x, child.position.y, child.position.z],
        i * 6,
      );
      const c = new THREE.Color(radarNodeColor(parent.radarAgent!));
      colors.set([c.r, c.g, c.b, c.r * 0.4, c.g * 0.4, c.b * 0.4], i * 6);
    });
    baseLineColors.current = colors.slice();
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('color', new THREE.BufferAttribute(colors, 3));
    return g;
  }, [links, byId]);

  const dotGeo = useMemo(() => {
    const positions = new Float32Array(links.length * 3);
    const colors = new Float32Array(links.length * 3);
    links.forEach((link, i) => {
      const c = new THREE.Color(radarNodeColor(byId.get(link.source)!.radarAgent!));
      colors.set([c.r, c.g, c.b], i * 3);
    });
    baseDotColors.current = colors.slice();
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('color', new THREE.BufferAttribute(colors, 3));
    return g;
  }, [links, byId]);

  const meta = useMemo(
    () =>
      links.map((link, i) => ({
        sourceId: link.source,
        targetId: link.target,
        parent: byId.get(link.source)!.position,
        child: byId.get(link.target)!.position,
        phase: (i * 0.37) % 1,
      })),
    [links, byId],
  );

  const lineMat = useRef<THREE.LineBasicMaterial>(null);
  const dotTex = useMemo(() => dotTexture(), []);

  useEffect(() => () => { lineGeo.dispose(); dotGeo.dispose(); }, [lineGeo, dotGeo]);

  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const lc = lifecycleRef.current;
    const dotPos = dotGeo.getAttribute('position') as THREE.BufferAttribute;
    const lineCol = lineGeo.getAttribute('color') as THREE.BufferAttribute;
    const dotCol = dotGeo.getAttribute('color') as THREE.BufferAttribute;
    const baseLine = baseLineColors.current;
    const baseDot = baseDotColors.current;
    const lineArr = lineCol.array as Float32Array;
    const dotArr = dotCol.array as Float32Array;

    for (let i = 0; i < meta.length; i++) {
      const m = meta[i];
      const tt = (t * 0.4 + m.phase) % 1; // parent → child (subagent travels out)
      dotPos.setXYZ(
        i,
        m.parent.x + (m.child.x - m.parent.x) * tt,
        m.parent.y + (m.child.y - m.parent.y) * tt,
        m.parent.z + (m.child.z - m.parent.z) * tt,
      );

      // fade a link out in lockstep with whichever endpoint globe is shrinking
      // (imploding/gone). Live link (both endpoints alive) → factor 1 → unchanged.
      const factor = Math.min(lc[m.sourceId]?.scale ?? 1, lc[m.targetId]?.scale ?? 1);
      const l = i * 6;
      for (let k = 0; k < 6; k++) lineArr[l + k] = baseLine[l + k] * factor;
      const d = i * 3;
      for (let k = 0; k < 3; k++) dotArr[d + k] = baseDot[d + k] * factor;
    }
    dotPos.needsUpdate = true;
    lineCol.needsUpdate = true;
    dotCol.needsUpdate = true;
    if (lineMat.current) lineMat.current.opacity = 0.22 + Math.sin(t * 1.4) * 0.05;
  });

  if (links.length === 0) return null;
  return (
    <group>
      <lineSegments geometry={lineGeo}>
        <lineBasicMaterial ref={lineMat} vertexColors transparent opacity={0.24} toneMapped={false} blending={THREE.AdditiveBlending} />
      </lineSegments>
      <points geometry={dotGeo}>
        <pointsMaterial
          vertexColors
          size={0.16}
          map={dotTex}
          transparent
          opacity={0.95}
          sizeAttenuation
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </points>
    </group>
  );
}

export type RadarConstellationProps = {
  model: RadarSceneModel;
  hoveredId: string | null;
  selectedId: string | null;
  /** Live fold scale for the constellation swap (1 = at rest). Omitted in the dev harness. */
  scaleRef?: { current: number };
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
};

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

// The DATA forest only — live globes, parent→child links, lifecycle + hover, wrapped
// in the fold group. It carries NO background/lights/camera/post: those live once in
// the persistent scene shell (WarRoom's SceneShell), so a Habits↔Radar swap only ever
// remounts this forest (already folded to nothing) and the void never flickers. The
// standalone dev harness wraps this in `RadarSceneBody`, which adds its own shell.
export function RadarForest({ model, hoveredId, selectedId, scaleRef, onHover, onLeave, onSelect, onClear }: RadarConstellationProps) {
  // The dev harness mounts the radar without a fold; default to a stable scale-1 ref.
  const fallbackScale = useRef(1);
  const sref = scaleRef ?? fallbackScale;

  const layout = useMemo(() => layoutRadarScene(model), [model]);

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
          <RadarLinks layout={layout} lifecycleRef={lifecycleRef} />
          {renderNodes.map((node) => (
            <RadarGlobe
              key={node.id}
              node={node}
              selected={selectedId === node.id}
              hovered={hoveredId === node.id}
              dimmed={Boolean(selectedId && selectedId !== node.id)}
              lifecycleRef={lifecycleRef}
              onHover={onHover}
              onLeave={onLeave}
              onSelect={onSelect}
            />
          ))}
        </group>

        <RadarHoverLayer node={hoveredNode} suppressed={Boolean(hoveredId && hoveredId === selectedId)} />
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

      <ambientLight intensity={0.1} />
      <directionalLight position={[5, 6, 4]} intensity={2.2} color="#e6fff0" />
      <directionalLight position={[-6, -1, -2]} intensity={0.7} color="#9fd0ff" />
      <Environment resolution={128}>
        {/* warm + cool formers so both Claude-orange and Codex-violet gems glint */}
        <Lightformer form="rect" intensity={1.8} color="#ffd9b8" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.3} color="#cab8ff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="ring" intensity={1.2} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      <StarCatalog />
      <CameraRig selected={selectedNode} />

      <RadarForest {...props} />

      <EffectComposer multisampling={4}>
        <Bloom intensity={0.95} luminanceThreshold={0.26} luminanceSmoothing={0.95} mipmapBlur radius={0.74} />
        <Vignette eskil={false} offset={0.2} darkness={0.92} />
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
