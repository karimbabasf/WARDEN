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
import { layoutRadarScene, type RadarCluster } from './radarLayout';
import { radarHarness } from './radarTheme';
import { AgentCore } from './AgentCore';
import { StarCatalog } from './StarCatalog';
import { FoldGroup } from './Transition';
import { CameraRig } from './CameraRig';
import { frameloopFor } from './WarRoom';
import { reconcileLifecycle, pruneGone, isVisible, type LifecycleEntry, type LifecycleMap, type LiveId } from './radarLifecycle';
import { RadarHoverCard } from './RadarHoverCard';
import { radarCanvasCamera } from './useOrbCamera';
import { targetDim, matchesFilter, type EmphasisFilter } from './emphasis';

const BG = '#020403';
const WHITE = new THREE.Color('#ffffff');

/**
 * The colour of a radar globe: its harness hue, FLAT. Colour is identity only —
 * never load and never liveness. Fill drives SIZE (the layout radius) and working
 * drives BRIGHTNESS (the per-frame blaze), so the hue itself stays constant and a
 * harness reads the same whether it's busy or idle. Pure + exported so it is
 * unit-tested without WebGL (the house pattern).
 */
export function radarNodeColor(agent: RadarAgent): string {
  return radarHarness(agent.harness).color;
}

/**
 * The glow TARGET a globe damps toward — the single brightness signal, and it is
 * LIVENESS, full stop. A working globe blazes (a big `liveLift`); an idle/closed one
 * falls back to a deliberately dim resting floor so it sinks below the bloom
 * threshold and the working ones are the only things that light the room. Fill is
 * intentionally absent: context is the SIZE channel, not the brightness channel.
 * Selection/hover/legend-emphasis add on top so the focused globe still pops.
 */
export function radarGlowTarget({
  agent,
  isRoot,
  emphasis,
  selected,
  hovered,
}: {
  agent: Pick<RadarAgent, 'status'>;
  isRoot: boolean;
  emphasis: boolean;
  selected: boolean;
  hovered: boolean;
}): number {
  const working = agent.status === 'working';
  // Dim resting floor (idle) vs a strong live blaze (working). The ~9× gap is what
  // makes a running agent unmistakable against the dulled-down rest of the forest.
  const restFloor = isRoot ? 0.22 : 0.16;
  const liveLift = working ? 2.7 : 0;
  return Math.max(
    0.05,
    restFloor +
      liveLift +
      (emphasis ? 0.6 : 0) +
      (selected ? 1.0 : hovered ? 0.4 : 0),
  );
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

/** Colour-only liveness scale for material hues: idle recedes, working restores full colour. */
export function radarLivenessColorScale(liveK: number): number {
  const k = Math.min(1, Math.max(0, liveK));
  return 0.62 + k * 0.38;
}

type LinkFadeEndpoint = {
  entry?: LifecycleEntry;
  gone?: boolean;
};

function endpointFadeFactor({ entry, gone = false }: LinkFadeEndpoint): number {
  if (gone || entry?.phase === 'gone') return 0;
  if (!entry) return 1;
  const scale = Math.max(0, Math.min(1, entry.scale));
  return entry.phase === 'imploding' ? Math.pow(scale, 1.4) : scale;
}

export function radarLinkFadeFactor(source: LinkFadeEndpoint, target: LinkFadeEndpoint): number {
  return Math.min(endpointFadeFactor(source), endpointFadeFactor(target));
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
  emphasis = false,
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
  /** This globe MATCHES the active legend harness filter — give it a gentle extra
   *  glow so the selection POPS, not just dims everything else. */
  emphasis?: boolean;
  /** Live read of the reconciler's per-id scale (no re-render on tween). */
  lifecycleRef: MutableRefObject<LifecycleMap>;
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
}) {
  const group = useRef<THREE.Group>(null!);
  const innerCage = useRef<THREE.Group>(null!);
  const gem = useRef<THREE.Group>(null!);
  const halo = useRef<THREE.Sprite>(null!);
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
  const terminated = agent.status === 'terminated';
  // A finishing subagent flares verdict-amber as the lifecycle implodes its scale.
  const baseHex = terminated ? '#ff5a37' : radarNodeColor(agent);
  const seed = useMemo(() => seedOf(node.id), [node.id]);

  // Colour depends only on harness hue + fill heat (or the amber terminated flare);
  // `normalizeRadarState` rebuilds the whole `agent` object every emit, so key on the
  // resolved hex (not identity) to avoid rebuilding the THREE.Color every frame.
  const color = useMemo(() => new THREE.Color(baseHex), [baseHex]);
  const innerColor = useMemo(() => color.clone().lerp(WHITE, 0.24), [color]);
  const nodeColor = useMemo(() => color.clone().lerp(WHITE, 0.16), [color]);
  // Far-hemisphere lattice lines: a very dark tint of the globe's OWN hue (never the
  // old phosphor green) so the back of the sphere reads as a dim echo of the front,
  // not a chartreuse cast where it blends with the orange/cyan front lines + bloom.
  const backStroke = useMemo(() => `#${color.clone().multiplyScalar(0.16).getHexString()}`, [color]);

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

  // `live` is the eased liveness factor (0 idle .. 1 working) — the brightness
  // channel. `glow` is the damped emissive target; `colorDim` the legend filter.
  const sim = useRef({ scale: 0.0001, glow: 0.5, live: working ? 1 : 0, dim: 0, colorDim: 0, pos: { ...node.position } });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    // The slow, smooth "alive" pulse — a calm ~4.2s sine (deliberately NOT snappy)
    // that only a working globe rides. It drives a SYNCED scale + halo + glow swell
    // below, so a running agent reads as softly breathing light — the wow that makes
    // "this is working" unmistakable. Per-globe `seed` phases it so the forest breathes
    // organically rather than strobing in lockstep.
    const PULSE_RATE = 1.5; // rad/s → ~4.2s period
    const pulseWave = Math.sin(t * PULSE_RATE + seed * 6.28); // -1..1 (gated by liveK below)
    // idle keeps a slow, deep ambient breath; working swaps to the synced pulse.
    const idleBreath = Math.sin(t * 0.5 + seed * 6.28) * 0.045;
    const boost = selected ? 0.22 : hovered ? 0.07 : 0;

    // lifecycle scale (0..1) is the spawn-in / implode-out factor from the pure
    // reconciler — read live from the ref so a tween never triggers a re-render.
    // A missing entry means full scale (a fresh, not-yet-reconciled node) EXCEPT
    // for an already-closed agent: it is dead, so an absent entry (e.g. just pruned
    // as `gone`) reads 0, never a one-frame full-scale flash before it unmounts.
    const lifecycleEntry = lifecycleRef.current[node.id];
    const lifecycleScale = lifecycleEntry?.scale ?? (agent.status === 'closed' || agent.status === 'terminated' ? 0 : 1);
    const targetScale = node.radius * (1 + boost) * Math.max(0, lifecycleScale);
    // BRIGHTNESS = LIVENESS. `live` eases toward 1 while working, 0 while idle, and
    // drives the whole blaze (emissive, halo, white-hot core, lattice brightness).
    // `glow` is the damped emissive target from radarGlowTarget (also liveness-led).
    // Neither reads fill — context is the SIZE channel only (targetScale above).
    const targetLive = working ? 1 : 0;
    const targetGlow = radarGlowTarget({ agent, isRoot, emphasis, selected, hovered });
    const targetDim = dimmed ? 1 : 0;
    // colourDim = the legend filter OR the boolean other-selected dim, whichever is
    // stronger. Idle dullness is NOT folded in here (it lives in `live`), so the two
    // signals stay cleanly separable.
    const targetColorDim = Math.max(targetDim, Math.min(1, Math.max(0, dimTarget)));

    // spawn eases in fast; implode collapses fast — both damped (never snap).
    const scaleLambda = lifecycleEntry?.phase === 'imploding' ? 18 : lifecycleScale < 0.999 ? 10 : 6;
    s.scale = damp(s.scale, targetScale, scaleLambda, dt);
    s.glow = damp(s.glow, targetGlow, 5, dt);
    s.live = damp(s.live, targetLive, 3.5, dt);
    s.dim = damp(s.dim, targetDim, 6, dt);
    s.colorDim = damp(s.colorDim, targetColorDim, DIM_LAMBDA, dt);
    // damp the node toward its layout position so re-layouts glide, not jump.
    s.pos.x = damp(s.pos.x, node.position.x, 4, dt);
    s.pos.y = damp(s.pos.y, node.position.y, 4, dt);
    s.pos.z = damp(s.pos.z, node.position.z, 4, dt);

    const liveK = s.live; // 0 idle .. 1 working — the brightness channel
    const pulse = liveK * pulseWave; // gated by liveness: 0 when idle, ±liveK when working

    // scale: the idle ambient breath fades out as the globe wakes; working rides a
    // gentle synced swell of the slow pulse instead (smooth, ~±4.5%).
    const breathe = 1 + idleBreath * (1 - liveK) + pulse * 0.045;
    group.current.scale.setScalar(s.scale * breathe);
    group.current.position.set(s.pos.x, s.pos.y + Math.sin(t * 0.6 + seed * 6.28) * 0.05, s.pos.z);
    group.current.rotation.y += dt * (isRoot ? 0.08 : 0.14);

    // halo: comes alive with liveness, then breathes IN and OUT on the slow pulse —
    // the soft aura swelling and receding is the most visible "this is working" tell.
    halo.current.scale.setScalar((isRoot ? 0.74 : 0.58) * (1 + liveK * 0.95 + pulse * 0.45));

    innerCage.current.rotation.y -= dt * 0.18;
    innerCage.current.rotation.x += dt * 0.1;
    gem.current.rotation.y += dt * 0.28;
    gem.current.rotation.x += dt * 0.12;

    // ── brightness = liveness ───────────────────────────────────────────────
    // dimK: boolean other-selected opacity track. litK: the legend filter ALSO
    // crushes opacity/emissive (not just colour) so a filtered-out globe sinks below
    // the bloom threshold → near-dark, while a match keeps its full halo + blooms.
    const dimK = 1 - s.dim * 0.6;
    const litK = 1 - s.colorDim * 0.86;
    // the SAME slow pulse swells the emissive + halo + nodes together, so the whole
    // globe brightens and dims as one calm breath of light (working only — `pulse` is
    // 0 when idle, so idle globes hold perfectly steady and the contrast is obvious).
    const pulseGlow = 1 + pulse * 0.3;
    gemMat.current.emissiveIntensity = (0.3 + s.glow * 1.05) * dimK * litK * pulseGlow;
    haloMat.current.opacity = Math.min(1, (0.05 + s.glow * 0.34) * dimK * litK * (1 + pulse * 0.45));
    nodeMat.current.opacity = Math.min(1, (0.16 + s.glow * 0.32) * dimK * litK * pulseGlow);

    // ── colour: hue dulls when idle, blazes white-hot when working ───────────
    // Copy each material's base colour, fold in liveness (idle = a dim hue, working
    // = brighter + lerped toward white-hot), then scale by the eased legend dim — its
    // floor is low so a filtered-out globe goes dark. Copy-first so nothing compounds.
    const shellScaleC = dimScale(s.colorDim, 0.08);
    const innerScaleC = dimScale(s.colorDim, 0.1);
    const shellLit = 0.32 + liveK * 0.68; // idle lattices dim, working full
    const colorQuiet = radarLivenessColorScale(liveK);
    const whiteHot = liveK * 0.5 + pulse * 0.12; // working core, whitening a touch on each pulse peak
    if (!shellMat.current) shellMat.current = findWireframeMaterial(shellGroup.current);
    if (!cageMat.current) cageMat.current = findWireframeMaterial(innerGroup.current);
    if (shellMat.current) {
      shellMat.current.uniforms.stroke.value
        .copy(shellBase)
        .lerp(WHITE, whiteHot * 0.4)
        .multiplyScalar(shellScaleC * shellLit);
    }
    if (cageMat.current) {
      const u = cageMat.current.uniforms;
      u.stroke.value.copy(innerBase).lerp(WHITE, whiteHot * 0.4).multiplyScalar(innerScaleC * shellLit);
      u.fill.value.copy(shellBase).lerp(WHITE, whiteHot * 0.4).multiplyScalar(shellScaleC * shellLit);
    }
    nodeMat.current.color.copy(nodeColor).lerp(WHITE, whiteHot).multiplyScalar(shellScaleC * colorQuiet);
    haloMat.current.color.copy(color).lerp(WHITE, whiteHot).multiplyScalar(shellScaleC * colorQuiet);
    gemMat.current.emissive.copy(color).lerp(WHITE, whiteHot).multiplyScalar(shellScaleC * colorQuiet);
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
          // drei's Wireframe defaults `fill` to PURE GREEN (#00ff00); even at
          // fillOpacity 0 it bleeds through the triangle faces and tints every
          // lattice (orange+green → the chartreuse cast on Claude globes). Point it
          // at the globe's own hue so the face-fill can never reintroduce green.
          fill={shellStroke}
          fillOpacity={0}
          backfaceStroke={backStroke}
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
        <sprite ref={halo} scale={isRoot ? 0.74 : 0.58}>
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
        <AgentCore harness={agent.harness} color={color} dimmed={dimmed} active={working || selected || hovered} working={working} />
      )}
    </group>
  );
}

// ── parent -> child glowing links (depth-N), mirroring Habits' AnimatedLinks but
// flowing parent → child and radar-tinted by the PARENT's heat colour. Each link's
// brightness is multiplied every frame by min(parentScale, childScale) read live
// from the SAME lifecycle map the globes use, so a link to an imploding/gone globe
// fades out in lockstep with it (no dangling full-brightness glow to an empty point).
function RadarLinks({
  layout,
  lifecycleRef,
  goneIdsRef,
}: {
  layout: OrbLayout;
  lifecycleRef: MutableRefObject<LifecycleMap>;
  goneIdsRef: MutableRefObject<Set<string>>;
}) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(() => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)), [layout, byId]);

  // Immutable full-brightness colour bases. The live color attributes are these
  // scaled by each link's endpoint lifecycle factor per frame (multiplying the
  // attribute in place would compound; the base never changes after layout).
  const baseLineColors = useRef<Float32Array>(new Float32Array(0));
  const baseDotColors = useRef<Float32Array>(new Float32Array(0));

  // One bowed bezier per parent→child edge, sampled into a gradient polyline — the
  // SAME constellation treatment Habits gives its tethers (curved volume, not flat
  // spokes), so every agent + its subagents read as one drawn figure. Tinted by each
  // endpoint's radar heat colour and brightest at the parent, the constellation's anchor.
  const SEG = 22;
  const { lineGeo, dotGeo, curves, meta } = useMemo(() => {
    const UP = new THREE.Vector3(0, 1, 0);
    const curves: THREE.QuadraticBezierCurve3[] = [];
    const linePos = new Float32Array(links.length * SEG * 6);
    const lineCol = new Float32Array(links.length * SEG * 6);
    const dotPos = new Float32Array(links.length * 3);
    const dotCol = new Float32Array(links.length * 3);
    const meta = links.map((link, idx) => {
      const parent = byId.get(link.source)!;
      const child = byId.get(link.target)!;
      const h = new THREE.Vector3(parent.position.x, parent.position.y, parent.position.z);
      const c = new THREE.Vector3(child.position.x, child.position.y, child.position.z);
      const dir = new THREE.Vector3().subVectors(c, h);
      const len = dir.length() || 1;
      const mid = new THREE.Vector3().addVectors(h, c).multiplyScalar(0.5);
      // bow the cable off the straight line (perpendicular + a little lift) so the
      // constellation has real 3D depth instead of collapsing onto flat spokes.
      const perp = new THREE.Vector3().crossVectors(dir, UP);
      if (perp.lengthSq() < 1e-4) perp.set(1, 0, 0);
      perp.normalize();
      const ctrl = mid.clone().add(perp.multiplyScalar(len * 0.16)).add(UP.clone().multiplyScalar(len * 0.06));
      const curve = new THREE.QuadraticBezierCurve3(h, ctrl, c);
      curves.push(curve);

      const cParent = new THREE.Color(radarNodeColor(parent.radarAgent!));
      const cChild = new THREE.Color(radarNodeColor(child.radarAgent!));
      const pts = curve.getPoints(SEG);
      const base = idx * SEG * 6;
      for (let s = 0; s < SEG; s++) {
        const ta = s / SEG;
        const tb = (s + 1) / SEG;
        const a = pts[s];
        const b = pts[s + 1];
        // brightest at the parent anchor, easing toward the child globe. Kept punchy
        // so the parent→child strand reads as a real drawn tether (the Habits look).
        const ca = cParent.clone().lerp(cChild, ta).multiplyScalar(0.92 - 0.3 * ta);
        const cb = cParent.clone().lerp(cChild, tb).multiplyScalar(0.92 - 0.3 * tb);
        const o = base + s * 6;
        linePos[o] = a.x; linePos[o + 1] = a.y; linePos[o + 2] = a.z;
        linePos[o + 3] = b.x; linePos[o + 4] = b.y; linePos[o + 5] = b.z;
        lineCol[o] = ca.r; lineCol[o + 1] = ca.g; lineCol[o + 2] = ca.b;
        lineCol[o + 3] = cb.r; lineCol[o + 4] = cb.g; lineCol[o + 5] = cb.b;
      }
      dotCol[idx * 3] = cParent.r; dotCol[idx * 3 + 1] = cParent.g; dotCol[idx * 3 + 2] = cParent.b;
      return { sourceId: link.source, targetId: link.target, phase: (idx * 0.37) % 1 };
    });

    baseLineColors.current = lineCol.slice();
    baseDotColors.current = dotCol.slice();
    const lineGeo = new THREE.BufferGeometry();
    lineGeo.setAttribute('position', new THREE.BufferAttribute(linePos, 3));
    lineGeo.setAttribute('color', new THREE.BufferAttribute(lineCol, 3));
    const dotGeo = new THREE.BufferGeometry();
    dotGeo.setAttribute('position', new THREE.BufferAttribute(dotPos, 3));
    dotGeo.setAttribute('color', new THREE.BufferAttribute(dotCol, 3));
    return { lineGeo, dotGeo, curves, meta };
  }, [links, byId]);

  const lineMat = useRef<THREE.LineBasicMaterial>(null);
  const dotTex = useMemo(() => dotTexture(), []);
  const tmp = useMemo(() => new THREE.Vector3(), []);

  useEffect(() => () => { lineGeo.dispose(); dotGeo.dispose(); }, [lineGeo, dotGeo]);

  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const lc = lifecycleRef.current;
    const dotPosAttr = dotGeo.getAttribute('position') as THREE.BufferAttribute;
    const lineColAttr = lineGeo.getAttribute('color') as THREE.BufferAttribute;
    const dotColAttr = dotGeo.getAttribute('color') as THREE.BufferAttribute;
    const baseLine = baseLineColors.current;
    const baseDot = baseDotColors.current;
    const lineArr = lineColAttr.array as Float32Array;
    const dotArr = dotColAttr.array as Float32Array;
    const stride = SEG * 6;

    for (let i = 0; i < meta.length; i++) {
      const m = meta[i];
      const tt = (t * 0.4 + m.phase) % 1; // mote travels parent → child (subagent spawned outward)
      curves[i].getPoint(tt, tmp);
      dotPosAttr.setXYZ(i, tmp.x, tmp.y, tmp.z);

      // fade a link out in lockstep with whichever endpoint globe is shrinking
      // (imploding/gone). Live link (both endpoints alive) → factor 1 → unchanged.
      const factor = radarLinkFadeFactor(
        { entry: lc[m.sourceId], gone: goneIdsRef.current.has(m.sourceId) },
        { entry: lc[m.targetId], gone: goneIdsRef.current.has(m.targetId) },
      );
      const l = i * stride;
      for (let k = 0; k < stride; k++) lineArr[l + k] = baseLine[l + k] * factor;
      const d = i * 3;
      for (let k = 0; k < 3; k++) dotArr[d + k] = baseDot[d + k] * factor;
    }
    dotPosAttr.needsUpdate = true;
    lineColAttr.needsUpdate = true;
    dotColAttr.needsUpdate = true;
    if (lineMat.current) lineMat.current.opacity = 0.52 + Math.sin(t * 1.3) * 0.1;
  });

  if (links.length === 0) return null;
  return (
    <group>
      <lineSegments geometry={lineGeo}>
        <lineBasicMaterial ref={lineMat} vertexColors transparent opacity={0.55} depthWrite={false} toneMapped={false} blending={THREE.AdditiveBlending} />
      </lineSegments>
      <points geometry={dotGeo}>
        <pointsMaterial
          vertexColors
          size={0.18}
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
  /** Active legend filter; each globe's colour-only `dimTarget` is derived from it.
   *  Harness filters apply on the radar tab; null (the default) leaves every globe at
   *  full colour. Optional so the standalone dev harness need not thread it. */
  emphasisFilter?: EmphasisFilter;
  /** Live fold scale for the constellation swap (1 = at rest). Omitted in the dev harness. */
  scaleRef?: { current: number };
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
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

// Per-FOLDER constellation labels — the explicit "this is the WARDEN folder / the JB
// Hunting folder" pinned under each cluster, mirroring the Habits hub labels. Colour
// + glyph come from the cluster's dominant harness (color-blind a11y); the text is the
// project folder. pointer-events off so it never steals the orbit camera or a globe click.
function RadarClusterLabels({ clusters }: { clusters: RadarCluster[] }) {
  return (
    <>
      {clusters.map((c) => {
        const t = radarHarness(c.harness);
        // sit the label just below the constellation's lowest reach so it never
        // collides with a globe (clusters are centred on the y=0 plane).
        const drop = c.radius * 0.62 + 0.7;
        return (
          <Html
            key={`cluster-${c.key}`}
            position={[c.center.x, c.center.y - drop, c.center.z]}
            center
            zIndexRange={[6, 0]}
            style={{ pointerEvents: 'none' } as CSSProperties}
          >
            <div className="wd-hub-label wd-folder-label" style={{ '--harness': t.color } as CSSProperties}>
              <span className="wd-hub-label-glyph" aria-hidden="true">{t.glyph}</span>
              {c.label}
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
export function RadarForest({ model, hoveredId, selectedId, emphasisFilter = null, scaleRef, onHover, onLeave, onSelect, onClear }: RadarConstellationProps) {
  // The dev harness mounts the radar without a fold; default to a stable scale-1 ref.
  const fallbackScale = useRef(1);
  const sref = scaleRef ?? fallbackScale;
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
  const layoutModel = useMemo(() => radarModelWithoutGone(model, goneIdsRef.current), [model, renderTick]);
  const layout = useMemo(() => layoutRadarScene(layoutModel), [layoutModel]);
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
          <RadarLinks layout={layout} lifecycleRef={lifecycleRef} goneIdsRef={goneIdsRef} />
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
              lifecycleRef={lifecycleRef}
              onHover={onHover}
              onLeave={onLeave}
              onSelect={onSelect}
            />
          ))}
        </group>

        {/* one "this is the WARDEN folder" label under each constellation */}
        <RadarClusterLabels clusters={layout.clusters} />

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
