// Orb.tsx — one node in the war-room mind-map, as a glowing LATTICE orb.
//
// Per Karim's signed-off /orbs.html study + direction: the orb is NOT a solid
// ball. It is a thin glowing wireframe network shell (the body), a counter-
// rotating inner cage for parallax, a glowing node at every outer vertex, and a
// small faceted CRYSTAL gem heart that glints as it turns — floating in mostly
// open space. Minimal, airy, luminous.
//
// Colour is unified: the WHOLE orb (shell + inner cage + nodes + gem + heart
// halo) glows in the orb's own colour — severity ramp for issues, harness colour
// for hubs. Harness identity for an issue is carried by the cluster/hub it links
// to (the link keeps the harness tint) and the hover label, never by the orb fill.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import { Wireframe } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from './orbTypes';
import { harnessTheme, severityColor } from './harnessTheme';
import { AgentCore } from './AgentCore';

const WHITE = new THREE.Color('#ffffff');

function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.lerp(current, target, 1 - Math.exp(-lambda * dt));
}

function seedOf(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 1000;
  return h / 1000;
}

// Soft white radial sprite, tinted per-orb at the material. Cached (one each).
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
let dotCache: THREE.Texture | null = null;
const dotTexture = () => (dotCache ??= radialTexture(64, [[0, 1], [0.4, 0.8], [1, 0]]));
let glowCache: THREE.Texture | null = null;
const glowTexture = () => (glowCache ??= radialTexture(128, [[0, 1], [0.18, 0.8], [0.5, 0.22], [1, 0]]));

export function Orb({
  node,
  selected,
  hovered,
  dimmed,
  appearDelay,
  onHover,
  onLeave,
  onSelect,
}: {
  node: LayoutNode;
  selected: boolean;
  hovered: boolean;
  dimmed: boolean;
  appearDelay: number;
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

  const isHub = node.kind === 'hub';
  const severity = node.issue?.severity ?? 1;
  const theme = harnessTheme(node.harness);
  const seed = useMemo(() => seedOf(node.id), [node.id]);

  const color = useMemo(() => new THREE.Color(isHub ? theme.color : severityColor(severity)), [isHub, severity, theme.color]);
  const innerColor = useMemo(() => color.clone().lerp(WHITE, 0.24), [color]);
  const nodeColor = useMemo(() => color.clone().lerp(WHITE, 0.14), [color]);

  // Shell stroke responds to hover/select/dim — these change on user action, not
  // per frame, so deriving them as props (re-render on change) stays cheap.
  const shellStroke = useMemo(() => {
    const c = color.clone();
    if (dimmed) c.multiplyScalar(0.42);
    else if (selected || hovered) c.lerp(WHITE, 0.18);
    return `#${c.getHexString()}`;
  }, [color, dimmed, selected, hovered]);
  const innerStroke = useMemo(() => `#${innerColor.clone().multiplyScalar(dimmed ? 0.45 : 1).getHexString()}`, [innerColor, dimmed]);

  const outerGeo = useMemo(() => new THREE.IcosahedronGeometry(1, isHub ? 2 : 1), [isHub]);
  const innerGeo = useMemo(() => new THREE.IcosahedronGeometry(0.6, 1), []);
  const gemGeo = useMemo(() => new THREE.IcosahedronGeometry(0.26, 0), []);
  const nodeGeo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', (outerGeo.attributes.position as THREE.BufferAttribute).clone());
    return g;
  }, [outerGeo]);
  const dotTex = useMemo(() => dotTexture(), []);
  const glowTex = useMemo(() => glowTexture(), []);

  useEffect(
    () => () => {
      outerGeo.dispose();
      innerGeo.dispose();
      gemGeo.dispose();
      nodeGeo.dispose();
    },
    [outerGeo, innerGeo, gemGeo, nodeGeo],
  );

  const sim = useRef({ scale: 0.0001, glow: 0.6, dim: 0, born: -1 });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;
    if (s.born < 0) s.born = t + appearDelay;
    const alive = t >= s.born;

    const breathe = 1 + Math.sin(t * 1.1 + seed * 6.28) * 0.02;
    const boost = selected ? 0.22 : hovered ? 0.07 : 0;
    const severityGlow = isHub ? 0.0 : (severity / 5) * 0.4;
    const targetScale = alive ? node.radius * (1 + boost) : 0.0001;
    const targetGlow = (isHub ? 0.85 : 0.5) + severityGlow + (selected ? 0.9 : hovered ? 0.35 : 0);
    const targetDim = dimmed ? 1 : 0;

    s.scale = damp(s.scale, targetScale, alive ? 6 : 14, dt);
    s.glow = damp(s.glow, targetGlow, 5, dt);
    s.dim = damp(s.dim, targetDim, 6, dt);

    group.current.scale.setScalar(s.scale * breathe);
    group.current.rotation.y += dt * (isHub ? 0.08 : 0.14);
    group.current.position.y = node.position.y + Math.sin(t * 0.6 + seed * 6.28) * 0.05;

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
      {/* Tight invisible hit-sphere — the ONLY interactive object in the orb (R3F
          only raycasts objects that carry handlers). Sized INSIDE the visible
          shell so hover/click fire when the cursor is on the globe, not from the
          empty space around the lattice — which was stealing clicks + scroll. */}
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

      {/* outer glowing network shell — the orb's body */}
      <Wireframe
        geometry={outerGeo}
        simplify
        stroke={shellStroke}
        thickness={isHub ? 0.02 : 0.016}
        dash={!isHub}
        dashRepeats={isHub ? 1 : 4}
        fillOpacity={0}
        backfaceStroke="#06150d"
      />

      {/* counter-rotating inner cage for parallax depth */}
      <group ref={innerCage}>
        <Wireframe geometry={innerGeo} simplify stroke={innerStroke} thickness={0.022} fill={shellStroke} fillOpacity={0.035} />
      </group>

      {/* glowing graph node at every outer vertex */}
      <points geometry={nodeGeo}>
        <pointsMaterial
          ref={nodeMat}
          size={isHub ? 0.08 : 0.07}
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

      {/* faceted crystal heart + soft halo */}
      <group ref={gem}>
        <sprite scale={isHub ? 0.62 : 0.5}>
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

      {/* orchestrator signature — only the agent hubs (Claude/Codex) wear the gyro
          cradle + brand heart, so they read as the things RUNNING the habit orbs
          they tether out to, never as just another bigger lattice. */}
      {isHub && <AgentCore harness={node.harness} color={color} dimmed={dimmed} active={selected || hovered} />}
    </group>
  );
}

export default Orb;
