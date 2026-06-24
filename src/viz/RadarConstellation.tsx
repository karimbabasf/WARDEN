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

import { useEffect, useMemo, useRef } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Sparkles, Stars, Environment, Lightformer, Wireframe } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode, OrbLayout } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { layoutRadarScene } from './radarLayout';
import { radarHarness, heatColor } from './radarTheme';
import { CameraRig } from './CameraRig';
import { frameloopFor } from './WarRoom';

// Structural shape of the lifecycle reconciler's output (Task 16 owns the full
// module + reducer). Declared locally so this render compiles independently and
// only consumes the `scale` it needs; the reconciler's richer entry is assignable.
export type LifecycleMap = Record<string, { scale: number }>;

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
  lifecycleScale,
  onHover,
  onLeave,
  onSelect,
}: {
  node: LayoutNode;
  selected: boolean;
  hovered: boolean;
  dimmed: boolean;
  lifecycleScale: number;
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

  const agent = node.radarAgent!;
  const isRoot = node.depth === 0;
  const working = agent.status === 'working';
  const seed = useMemo(() => seedOf(node.id), [node.id]);

  const color = useMemo(() => new THREE.Color(radarNodeColor(agent)), [agent]);
  const innerColor = useMemo(() => color.clone().lerp(WHITE, 0.24), [color]);
  const nodeColor = useMemo(() => color.clone().lerp(WHITE, 0.16), [color]);

  const shellStroke = useMemo(() => {
    const c = color.clone();
    if (dimmed) c.multiplyScalar(0.42);
    else if (selected || hovered) c.lerp(WHITE, 0.18);
    return `#${c.getHexString()}`;
  }, [color, dimmed, selected, hovered]);
  const innerStroke = useMemo(
    () => `#${innerColor.clone().multiplyScalar(dimmed ? 0.45 : 1).getHexString()}`,
    [innerColor, dimmed],
  );

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

  const sim = useRef({ scale: 0.0001, glow: 0.5, dim: 0, pos: { ...node.position } });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    // breathing: working = a faint quick shimmer; idle = a slow, deeper breath.
    const breatheAmp = working ? 0.02 : 0.045;
    const breatheRate = working ? 1.6 : 0.5;
    const breathe = 1 + Math.sin(t * breatheRate + seed * 6.28) * breatheAmp;
    const boost = selected ? 0.22 : hovered ? 0.07 : 0;

    // lifecycleScale (0..1) is the spawn-in / implode-out factor from the reconciler.
    const targetScale = node.radius * (1 + boost) * Math.max(0, lifecycleScale);
    const fillGlow = 0.25 + agent.fillPct * 0.65; // fuller = hotter core
    const idleDim = working ? 0 : 0.28; // idle agents read dimmer (at-a-glance who's thinking)
    const targetGlow = (isRoot ? 0.8 : 0.55) + fillGlow - idleDim + (selected ? 0.9 : hovered ? 0.35 : 0);
    const targetDim = dimmed ? 1 : 0;

    // spawn eases in fast; implode collapses fast — both damped (never snap).
    const scaleLambda = lifecycleScale < 0.999 ? 10 : 6;
    s.scale = damp(s.scale, targetScale, scaleLambda, dt);
    s.glow = damp(s.glow, targetGlow, 5, dt);
    s.dim = damp(s.dim, targetDim, 6, dt);
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

    const dimK = 1 - s.dim * 0.6;
    gemMat.current.emissiveIntensity = (0.55 + s.glow * 0.6) * dimK;
    haloMat.current.opacity = (0.2 + s.glow * 0.28) * dimK;
    nodeMat.current.opacity = (0.45 + s.glow * 0.32) * dimK;
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

      <group ref={innerCage}>
        <Wireframe geometry={innerGeo} simplify stroke={innerStroke} thickness={0.022} fill={shellStroke} fillOpacity={0.035} />
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
    </group>
  );
}

// ── parent -> child glowing links (depth-N), mirroring Habits' AnimatedLinks but
// flowing parent → child and radar-tinted by the PARENT's heat colour. ──────────
function RadarLinks({ layout }: { layout: OrbLayout }) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(() => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)), [layout, byId]);

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
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('color', new THREE.BufferAttribute(colors, 3));
    return g;
  }, [links, byId]);

  const meta = useMemo(
    () =>
      links.map((link, i) => ({
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
    const attr = dotGeo.getAttribute('position') as THREE.BufferAttribute;
    for (let i = 0; i < meta.length; i++) {
      const m = meta[i];
      const tt = (t * 0.4 + m.phase) % 1; // parent → child (subagent travels out)
      attr.setXYZ(
        i,
        m.parent.x + (m.child.x - m.parent.x) * tt,
        m.parent.y + (m.child.y - m.parent.y) * tt,
        m.parent.z + (m.child.z - m.parent.z) * tt,
      );
    }
    attr.needsUpdate = true;
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
  lifecycle?: LifecycleMap;
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
};

// The scene body (lights + space + nodes). Pulled out so it can sit inside the
// shared <Canvas> in WarRoom OR a standalone <Canvas> in the dev harness.
export function RadarSceneBody({ model, hoveredId, selectedId, lifecycle, onHover, onLeave, onSelect, onClear }: RadarConstellationProps) {
  const { gl } = useThree();
  useEffect(() => {
    gl.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    gl.toneMapping = THREE.ACESFilmicToneMapping;
    gl.toneMappingExposure = 1.05;
  }, [gl]);

  const layout = useMemo(() => layoutRadarScene(model), [model]);
  const selectedNode = useMemo(() => layout.nodes.find((n) => n.id === selectedId) ?? null, [layout, selectedId]);
  const selectedAgentId = selectedNode?.id ?? null;

  return (
    <>
      <color attach="background" args={[BG]} />
      <fogExp2 attach="fog" args={[BG, 0.012]} />

      <ambientLight intensity={0.1} />
      <directionalLight position={[5, 6, 4]} intensity={2.2} color="#e6fff0" />
      <directionalLight position={[-6, -1, -2]} intensity={0.7} color="#9fd0ff" />
      <Environment resolution={128}>
        {/* warm + cool formers so both Claude-orange and Codex-violet gems glint */}
        <Lightformer form="rect" intensity={1.8} color="#ffd9b8" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.3} color="#cab8ff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="ring" intensity={1.2} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      <Stars radius={36} depth={48} count={3800} factor={5} saturation={0} fade speed={0.08} />
      <Sparkles count={40} scale={[28, 16, 26]} size={1.4} speed={0.05} opacity={0.22} color="#d8c8ff" />

      <CameraRig selected={selectedNode} />

      <group onPointerMissed={onClear}>
        <RadarLinks layout={layout} />
        {layout.nodes.map((node) => (
          <RadarGlobe
            key={node.id}
            node={node}
            selected={selectedId === node.id}
            hovered={hoveredId === node.id}
            dimmed={Boolean(selectedId && selectedId !== node.id && node.id !== selectedAgentId)}
            lifecycleScale={lifecycle?.[node.id]?.scale ?? 1}
            onHover={onHover}
            onLeave={onLeave}
            onSelect={onSelect}
          />
        ))}
      </group>

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
      camera={{ position: [0, 1.4, 11.5], fov: 46, near: 0.1, far: 140 }}
    >
      <RadarSceneBody {...props} />
    </Canvas>
  );
}

export default RadarConstellation;
