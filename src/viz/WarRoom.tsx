// WarRoom.tsx — the cinematic R3F war-room island (spec §6). It renders ONLY
// real signals from `bridge.ts`:
//   • one Wireframe-Cell node per real candidate (+ a cluster glyph for overflow)
//   • 3 stage nodes (Diagnostician / Coach / Verifier) sized & lit by REAL
//     `fugu_usage` token weight (orchestration when present, plain tokens off-Fugu)
//   • core colour driven by REAL `finding_verdict` — emerald → amber flare + grow
//     + persist on `confirmed`; dim / collapse / die on `refuted`
//   • travelling token-pulses from REAL `fugu_delta` / `fugu_usage` activity
//   • harness identity as a thin cage-rim tint + a legend (colour ALWAYS paired
//     with glyph + label — never colour alone)
//
// Built per r3f-mastery: capped dpr, ACESFilmic tone mapping, FogExp2, an
// UnrealBloom + vignette + film-grain post stack, geometry/materials created
// once (never inside useFrame), frame-rate-independent damping, RAF paused when
// the overlay window is hidden, and full GPU disposal on unmount.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette, Noise } from '@react-three/postprocessing';
import { BlendFunction } from 'postprocessing';
import * as THREE from 'three';
import type { Bridge, SceneState, Verdict } from './bridge';
import { harnessTheme } from './harnessTheme';

// ── palette (mirrors style.css phosphor tokens) ──────────────────────────────
const CORE_IDLE = new THREE.Color('#76ff9d'); // emerald phosphor
const CORE_CONFIRM = new THREE.Color('#ff5a37'); // verdict amber/red flare
const CAGE_BASE = new THREE.Color('#1b6f3a'); // dim cage wire
const STAGE_NAMES = ['Diagnostician', 'Coach', 'Verifier'] as const;

// Frame-rate-independent damping (r3f-mastery key math pattern).
function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.lerp(current, target, 1 - Math.exp(-lambda * dt));
}

// Deterministic placement on a sphere shell so node layout is stable per index
// (Fibonacci sphere — even, non-clumping distribution).
function fib(i: number, n: number, radius: number): THREE.Vector3 {
  const golden = Math.PI * (3 - Math.sqrt(5));
  const y = n <= 1 ? 0 : 1 - (i / (n - 1)) * 2;
  const r = Math.sqrt(Math.max(0, 1 - y * y));
  const theta = golden * i;
  return new THREE.Vector3(Math.cos(theta) * r, y, Math.sin(theta) * r).multiplyScalar(radius);
}

type NodeKind = 'stage' | 'candidate';

type NodeView = {
  key: string;
  kind: NodeKind;
  position: THREE.Vector3;
  harness: string; // 'claude_code' | 'codex' | 'unknown'
  /** stage index for stage nodes (token-weight lookup), else -1 */
  stageIndex: number;
  /** finding ids that target this node's pattern (for verdict colouring) */
  findingKey: string; // pattern id for candidates, stage name for stages
};

// ── one Wireframe-Cell ───────────────────────────────────────────────────────
// A SINGLE wireframe icosahedron cage + a hot-white inner core. No double
// shells, no vertex sparkle — clarity over noise. The core colour is the verdict
// channel; the cage rim is tinted by harness (secondary accent).
function WireframeCell({
  node,
  verdict,
  stageWeight,
}: {
  node: NodeView;
  verdict: Verdict | undefined;
  stageWeight: number; // 0..1 real token weight (stage nodes only)
}) {
  const group = useRef<THREE.Group>(null!);
  const coreMat = useRef<THREE.MeshBasicMaterial>(null!);
  const cageMat = useRef<THREE.MeshBasicMaterial>(null!);
  const theme = harnessTheme(node.harness);

  // Geometry/materials are created ONCE via useMemo, never in useFrame.
  const cageColor = useMemo(() => CAGE_BASE.clone().lerp(new THREE.Color(theme.color), 0.55), [theme.color]);
  const baseScale = node.kind === 'stage' ? 0.62 : 0.34;

  // Smoothed visual state lives in a ref so we don't re-render per frame.
  const sim = useRef({ scale: baseScale, glow: 0.5, dead: 0 });

  useFrame((_, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05); // clamp huge tab-restore deltas
    const t = performance.now() / 1000;
    const s = sim.current;

    // Verdict → core colour + size + persistence.
    let targetColor = CORE_IDLE;
    let targetScale = baseScale;
    let targetGlow = node.kind === 'stage' ? 0.4 + stageWeight * 1.6 : 0.55;
    let targetDead = 0;

    if (verdict?.verdict === 'confirmed') {
      targetColor = CORE_CONFIRM;
      // grow with confirmed severity (real signal), then persist.
      targetScale = baseScale * (1.35 + Math.min(verdict.severity, 5) * 0.12);
      targetGlow = 2.2;
    } else if (verdict?.verdict === 'refuted') {
      // collapse + die: shrink toward nothing and fade out.
      targetScale = baseScale * 0.18;
      targetGlow = 0.05;
      targetDead = 1;
    }

    s.scale = damp(s.scale, targetScale, 5, dt);
    s.glow = damp(s.glow, targetGlow, 4, dt);
    s.dead = damp(s.dead, targetDead, 3, dt);

    if (group.current) {
      // gentle breathing + slow tumble; idle stages breathe to token weight.
      const breathe = 1 + Math.sin(t * 1.6 + node.position.x) * 0.04;
      group.current.scale.setScalar(s.scale * breathe);
      group.current.rotation.y += dt * (0.18 + stageWeight * 0.25);
      group.current.rotation.x += dt * 0.07;
    }
    if (coreMat.current) {
      coreMat.current.color.copy(targetColor);
      coreMat.current.opacity = (0.85 + s.glow * 0.05) * (1 - s.dead);
    }
    if (cageMat.current) {
      cageMat.current.opacity = (0.45 + s.glow * 0.12) * (1 - s.dead * 0.9);
    }
  });

  return (
    <group ref={group} position={node.position}>
      {/* hot-white core (verdict colour channel) */}
      <mesh>
        <icosahedronGeometry args={[0.5, 0]} />
        <meshBasicMaterial ref={coreMat} color={CORE_IDLE} transparent toneMapped={false} />
      </mesh>
      {/* single wireframe cage, harness-tinted rim */}
      <mesh scale={1.55}>
        <icosahedronGeometry args={[0.5, node.kind === 'stage' ? 1 : 0]} />
        <meshBasicMaterial ref={cageMat} color={cageColor} wireframe transparent opacity={0.5} toneMapped={false} />
      </mesh>
    </group>
  );
}

// ── nearest-neighbour edges (constellation lines) ────────────────────────────
// Built once per node set; positions are static so a single LineSegments buffer
// is enough. Each candidate links to its k nearest neighbours among all nodes.
function Edges({ nodes }: { nodes: NodeView[] }) {
  const geometry = useMemo(() => {
    const pts: number[] = [];
    const k = 2;
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      const dists = nodes
        .map((b, j) => ({ j, d: a.position.distanceToSquared(b.position) }))
        .filter(x => x.j !== i)
        .sort((x, y) => x.d - y.d)
        .slice(0, k);
      for (const { j } of dists) {
        const b = nodes[j];
        pts.push(a.position.x, a.position.y, a.position.z, b.position.x, b.position.y, b.position.z);
      }
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(pts, 3));
    return g;
  }, [nodes]);

  useEffect(() => () => geometry.dispose(), [geometry]);

  return (
    <lineSegments geometry={geometry}>
      <lineBasicMaterial color="#1b6f3a" transparent opacity={0.22} toneMapped={false} />
    </lineSegments>
  );
}

// ── travelling token-pulses ──────────────────────────────────────────────────
// One short-lived sprite per real `fugu_delta`/`fugu_usage` pulse, flying from a
// stage node outward. A fixed-size pool keyed by pulse id (mount-once buffers).
function Pulses({ pulses, stagePositions }: { pulses: SceneState['pulses']; stagePositions: THREE.Vector3[] }) {
  const POOL = 48;
  const points = useRef<THREE.Points>(null!);
  const geom = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(new Float32Array(POOL * 3), 3));
    g.setAttribute('aIntensity', new THREE.BufferAttribute(new Float32Array(POOL), 1));
    return g;
  }, []);

  // Active pulse simulation state (origin, direction, life). Mutated in place.
  const active = useRef<{ id: number; born: number; from: THREE.Vector3; dir: THREE.Vector3; intensity: number }[]>([]);
  const seen = useRef(new Set<number>());

  useEffect(() => () => geom.dispose(), [geom]);

  // Spawn newly-arrived pulses (ids we haven't seen) on each state change.
  useEffect(() => {
    for (const p of pulses) {
      if (seen.current.has(p.id)) continue;
      seen.current.add(p.id);
      const si = Math.max(0, STAGE_NAMES.indexOf(p.stage as (typeof STAGE_NAMES)[number]));
      const from = (stagePositions[si] ?? new THREE.Vector3()).clone();
      const dir = new THREE.Vector3(Math.random() - 0.5, Math.random() - 0.5, Math.random() - 0.5).normalize();
      active.current.push({ id: p.id, born: performance.now() / 1000, from, dir, intensity: p.intensity });
      if (active.current.length > POOL) active.current.shift();
    }
    // keep the seen-set bounded
    if (seen.current.size > 256) seen.current = new Set(pulses.map(p => p.id));
  }, [pulses, stagePositions]);

  useFrame(() => {
    const now = performance.now() / 1000;
    const pos = geom.getAttribute('position') as THREE.BufferAttribute;
    const inten = geom.getAttribute('aIntensity') as THREE.BufferAttribute;
    const LIFE = 1.4;
    let w = 0;
    active.current = active.current.filter(p => now - p.born < LIFE);
    for (const p of active.current) {
      if (w >= POOL) break;
      const age = (now - p.born) / LIFE; // 0..1
      const reach = 2.4 * age;
      pos.setXYZ(w, p.from.x + p.dir.x * reach, p.from.y + p.dir.y * reach, p.from.z + p.dir.z * reach);
      inten.setX(w, p.intensity * (1 - age));
      w++;
    }
    // park the rest of the pool far offscreen
    for (let i = w; i < POOL; i++) {
      pos.setXYZ(i, 0, 0, 9999);
      inten.setX(i, 0);
    }
    pos.needsUpdate = true;
    inten.needsUpdate = true;
  });

  return (
    <points ref={points} geometry={geom}>
      <pointsMaterial
        color="#b8ff6b"
        size={0.16}
        sizeAttenuation
        transparent
        opacity={0.9}
        depthWrite={false}
        blending={THREE.AdditiveBlending}
        toneMapped={false}
      />
    </points>
  );
}

// ── ambient dust ─────────────────────────────────────────────────────────────
// Static drifting motes for depth. Pure decoration (NOT a signal) — deliberately
// dim so it never reads as data.
function Dust() {
  const ref = useRef<THREE.Points>(null!);
  const geom = useMemo(() => {
    const N = 260;
    const arr = new Float32Array(N * 3);
    for (let i = 0; i < N; i++) {
      arr[i * 3] = (Math.random() - 0.5) * 16;
      arr[i * 3 + 1] = (Math.random() - 0.5) * 10;
      arr[i * 3 + 2] = (Math.random() - 0.5) * 16;
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(arr, 3));
    return g;
  }, []);
  useEffect(() => () => geom.dispose(), [geom]);
  useFrame((_, dt) => {
    if (ref.current) ref.current.rotation.y += Math.min(dt, 0.05) * 0.015;
  });
  return (
    <points ref={ref} geometry={geom}>
      <pointsMaterial color="#3dffa0" size={0.025} sizeAttenuation transparent opacity={0.28} depthWrite={false} toneMapped={false} />
    </points>
  );
}

// ── slow camera drift so the constellation feels alive ───────────────────────
function CameraRig() {
  const { camera } = useThree();
  useFrame((_, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = performance.now() / 1000;
    const tx = Math.sin(t * 0.12) * 1.1;
    const ty = Math.cos(t * 0.09) * 0.6;
    camera.position.x = damp(camera.position.x, tx, 1.5, dt);
    camera.position.y = damp(camera.position.y, ty, 1.5, dt);
    camera.lookAt(0, 0, 0);
  });
  return null;
}

// Build the full node set from scene state: 3 stage nodes (outer ring) + the
// candidate cloud (inner sphere). Layout is deterministic per index.
function useNodes(scene: SceneState): { nodes: NodeView[]; stagePositions: THREE.Vector3[] } {
  return useMemo(() => {
    const stagePositions = STAGE_NAMES.map((_, i) => {
      const a = (i / STAGE_NAMES.length) * Math.PI * 2;
      return new THREE.Vector3(Math.cos(a) * 3.4, Math.sin(a) * 0.4, Math.sin(a) * 3.4);
    });
    const stages: NodeView[] = STAGE_NAMES.map((name, i) => ({
      key: `stage-${name}`,
      kind: 'stage',
      position: stagePositions[i],
      harness: 'unknown',
      stageIndex: i,
      findingKey: name,
    }));
    const cands: NodeView[] = scene.candidates.map((c, i) => ({
      key: `cand-${c.patternId}-${c.sessionId}-${i}`,
      kind: 'candidate',
      position: fib(i + 1, scene.candidates.length + 2, 1.9),
      harness: c.harness,
      stageIndex: -1,
      findingKey: c.patternId,
    }));
    return { nodes: [...stages, ...cands], stagePositions };
  }, [scene.candidates]);
}

// Resolve the verdict that targets a given node (match on pattern id). Confirmed
// wins over refuted if (rarely) both exist for one pattern.
function verdictFor(scene: SceneState, findingKey: string): Verdict | undefined {
  let refuted: Verdict | undefined;
  for (const v of Object.values(scene.verdicts)) {
    if (v.patternId !== findingKey) continue;
    if (v.verdict === 'confirmed') return v;
    refuted = v;
  }
  return refuted;
}

// Real token weight for a stage, normalised 0..1. Orchestration tokens (Fugu)
// preferred; degrade to plain tokens off-Fugu — honest, never fabricated.
function stageWeight(scene: SceneState, stage: string): number {
  const u = scene.usage[stage];
  if (!u) return 0;
  const orch = u.orchIn + u.orchOut;
  const tokens = orch > 0 ? orch : u.in + u.out;
  if (tokens <= 0) return 0;
  return Math.min(1, Math.log10(tokens + 10) / 5);
}

function Scene({ scene }: { scene: SceneState }) {
  const { gl } = useThree();
  const { nodes, stagePositions } = useNodes(scene);

  // r3f-mastery: cap dpr at 2 + ACESFilmic tone mapping.
  useEffect(() => {
    gl.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    gl.toneMapping = THREE.ACESFilmicToneMapping;
    gl.toneMappingExposure = 1.05;
  }, [gl]);

  return (
    <>
      <fogExp2 attach="fog" args={['#020403', 0.085]} />
      <color attach="background" args={['#020403']} />
      <CameraRig />
      <Dust />
      <Edges nodes={nodes} />
      {nodes.map(n => (
        <WireframeCell
          key={n.key}
          node={n}
          verdict={n.kind === 'candidate' ? verdictFor(scene, n.findingKey) : undefined}
          stageWeight={n.kind === 'stage' ? stageWeight(scene, n.findingKey) : 0}
        />
      ))}
      <Pulses pulses={scene.pulses} stagePositions={stagePositions} />
      <EffectComposer>
        <Bloom intensity={1.0} luminanceThreshold={0.15} luminanceSmoothing={0.9} mipmapBlur radius={0.7} />
        <Vignette eskil={false} offset={0.25} darkness={0.85} />
        <Noise premultiply blendFunction={BlendFunction.OVERLAY} opacity={0.06} />
      </EffectComposer>
    </>
  );
}

// ── legend (colour ALWAYS paired with glyph + label — a11y) ──────────────────
function Legend({ scene }: { scene: SceneState }) {
  // surface which harnesses are actually present, plus a verdict key.
  const present = useMemo(() => {
    const set = new Set(scene.candidates.map(c => (c.harness === 'codex' ? 'codex' : c.harness === 'claude_code' ? 'claude_code' : 'unknown')));
    if (set.size === 0) set.add('claude_code');
    return Array.from(set);
  }, [scene.candidates]);

  const confirmed = Object.values(scene.verdicts).filter(v => v.verdict === 'confirmed').length;
  const refuted = Object.values(scene.verdicts).filter(v => v.verdict === 'refuted').length;

  return (
    <div className="viz-legend" aria-hidden="false">
      {present.map(h => {
        const t = harnessTheme(h);
        return (
          <span className="viz-legend-item" key={h}>
            <span className="viz-glyph" style={{ color: t.color }}>{t.glyph}</span>
            {t.label}
          </span>
        );
      })}
      <span className="viz-legend-item">
        <span className="viz-glyph" style={{ color: '#ff5a37' }}>◆</span>
        confirmed {confirmed > 0 ? `· ${confirmed}` : ''}
      </span>
      {refuted > 0 && (
        <span className="viz-legend-item viz-legend-dim">
          <span className="viz-glyph" style={{ color: '#1b6f3a' }}>×</span>
          refuted · {refuted}
        </span>
      )}
      {scene.clustered > 0 && (
        <span className="viz-legend-item viz-legend-dim">
          <span className="viz-glyph">⊕</span>
          +{scene.clustered} clustered
        </span>
      )}
    </div>
  );
}

/**
 * The mounted island. Subscribes to the bridge for live `SceneState`, renders
 * the R3F constellation, and pauses the render loop when the overlay window is
 * hidden (summon-cost / battery hygiene).
 */
export function WarRoom({ bridge }: { bridge: Bridge }) {
  const [scene, setScene] = useState<SceneState>(() => ({
    phase: 'idle',
    candidates: [],
    verdicts: {},
    pulses: [],
    usage: {},
    clustered: 0,
  }));
  const [active, setActive] = useState(() => !document.hidden);

  useEffect(() => bridge.subscribe(setScene), [bridge]);

  // Pause RAF when the overlay is hidden; resume on show.
  useEffect(() => {
    const onVis = () => setActive(!document.hidden);
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, []);

  return (
    <div className={`viz-root viz-phase-${scene.phase}`}>
      <Canvas
        dpr={[1, 2]}
        frameloop={active ? 'always' : 'never'}
        gl={{ antialias: true, alpha: true, powerPreference: 'high-performance' }}
        camera={{ position: [0, 0, 8.5], fov: 50, near: 0.1, far: 100 }}
      >
        <Scene scene={scene} />
      </Canvas>
      <Legend scene={scene} />
    </div>
  );
}

export default WarRoom;
