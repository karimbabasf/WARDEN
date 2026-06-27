// AgentCore.tsx — the signature that marks an ORCHESTRATOR globe.
//
// Habit/issue/subagent globes are bare lattices. An *agent* globe (a Claude or
// Codex hub in Habits, a root agent in Radar) is the thing that RUNS the others —
// so it carries an extra core the bare globes never do, without leaving the
// lattice family:
//
//   • gyro rings   two thin great-circle rings gyrating slowly around the heart,
//                  like an armillary / instrument cradle. A ringed, governed body
//                  reads as the authority next to loose lattice moons. This is the
//                  one bold move; everything else stays the shared lattice.
//   • brand heart  the harness's own mark, drawn in light at the very centre:
//                    – Claude → a radiant SUNBURST spark (Anthropic's mark).
//                    – Codex  → a six-fold BLOSSOM rosette (OpenAI's mark).
//                    – unknown → a quiet neutral star (honest-viz: an unrecognised
//                      harness never borrows Claude's or Codex's identity).
//
// All geometry is additive glowing line-work (1px, bloom does the rest), matching
// the tether/figure lines elsewhere. Self-contained: own rotation in useFrame, own
// disposal. Rendered in the orb's LOCAL unit space (the parent group already scales
// by node.radius), so it sizes with the globe and never clips its territory.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import * as THREE from 'three';

const WHITE = new THREE.Color('#ffffff');

type Motif = 'sunburst' | 'blossom' | 'star';

/** Pick the brand motif from the snake_case harness id (unknown → neutral star). */
export function motifFor(harness: string): Motif {
  if (harness === 'claude_code') return 'sunburst';
  if (harness === 'codex') return 'blossom';
  return 'star';
}

/** Working roots get a slightly faster gyro cradle; idle roots stay calm. */
export function agentCoreSpinMultiplier(working: boolean): number {
  return working ? 1.35 : 1;
}

export type AgentCorePulseState = {
  heartScale: number;
  ringScale: number;
  glowMultiplier: number;
};

/** Working roots breathe at the core; idle roots stay visually steady. */
export function agentCorePulseState(working: boolean, wave: number): AgentCorePulseState {
  if (!working) return { heartScale: 1, ringScale: 1, glowMultiplier: 1 };
  const w = Math.max(-1, Math.min(1, wave));
  const crest = (w + 1) / 2;
  return {
    heartScale: 1.04 + w * 0.08,
    ringScale: 1.02 + w * 0.05,
    glowMultiplier: 1 + crest * 0.28,
  };
}

// ── geometry builders (all in local unit space, the gem heart is ~0.26) ─────────

/** A flat circle as a line loop in the XY plane — one gyro ring. */
function ringGeometry(radius: number, segments = 96): THREE.BufferGeometry {
  const pos = new Float32Array((segments + 1) * 3);
  for (let i = 0; i <= segments; i++) {
    const a = (i / segments) * Math.PI * 2;
    pos[i * 3] = Math.cos(a) * radius;
    pos[i * 3 + 1] = Math.sin(a) * radius;
    pos[i * 3 + 2] = 0;
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  return g;
}

// Claude's sunburst: rays fired from the centre outward. We seat them on the 3D
// directions of an icosahedron's vertices so the spark reads as a radiant star
// from ANY orbit angle (a flat logo would vanish edge-on), with alternating ray
// lengths for the long/short cadence of the real mark.
function sunburstGeometry(): THREE.BufferGeometry {
  const t = (1 + Math.sqrt(5)) / 2;
  const verts = [
    [-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0],
    [0, -1, t], [0, 1, t], [0, -1, -t], [0, 1, -t],
    [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1],
  ];
  const pos: number[] = [];
  verts.forEach((v, i) => {
    const d = new THREE.Vector3(v[0], v[1], v[2]).normalize();
    const inner = 0.1;
    const outer = 0.42 + (i % 2 === 0 ? 0.12 : 0); // long/short ray cadence
    pos.push(d.x * inner, d.y * inner, d.z * inner, d.x * outer, d.y * outer, d.z * outer);
  });
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  return g;
}

// OpenAI / Codex blossom: six petals in 6-fold rotational symmetry. Each petal is
// a radial ellipse (a line loop) seated just off-centre and pointing outward, so
// the six together draw the knotted rosette silhouette. Returns one merged geo.
function blossomGeometry(): THREE.BufferGeometry {
  const PETALS = 6;
  const SEG = 40;
  const major = 0.34; // radial reach of a petal
  const minor = 0.12; // petal width
  const seat = 0.12; // how far the petal centre sits from the core
  const pos: number[] = [];
  for (let p = 0; p < PETALS; p++) {
    const a = (p / PETALS) * Math.PI * 2;
    const ca = Math.cos(a);
    const sa = Math.sin(a);
    const cx = ca * seat;
    const cy = sa * seat;
    let prev: [number, number] | null = null;
    for (let s = 0; s <= SEG; s++) {
      const th = (s / SEG) * Math.PI * 2;
      // ellipse in petal-local axes (major along the radial dir, minor across it)
      const ex = Math.cos(th) * major;
      const ey = Math.sin(th) * minor;
      const x = cx + ex * ca - ey * sa;
      const y = cy + ex * sa + ey * ca;
      if (prev) pos.push(prev[0], prev[1], 0, x, y, 0);
      prev = [x, y];
    }
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  return g;
}

function heartGeometry(motif: Motif): THREE.BufferGeometry {
  if (motif === 'sunburst') return sunburstGeometry();
  if (motif === 'blossom') return blossomGeometry();
  // neutral star — a sparse, quiet burst (fewer/shorter rays than Claude's spark).
  const g = new THREE.BufferGeometry();
  const pos: number[] = [];
  for (let i = 0; i < 6; i++) {
    const a = (i / 6) * Math.PI * 2;
    pos.push(0, 0, 0, Math.cos(a) * 0.3, Math.sin(a) * 0.3, 0);
  }
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  return g;
}

export function AgentCore({
  harness,
  color,
  dimmed,
  active,
  working = false,
}: {
  /** snake_case harness id; selects the brand motif. */
  harness: string;
  /** The globe's own colour (harness hue / heat colour) so the core matches it. */
  color: THREE.Color | string;
  dimmed: boolean;
  /** Selected or hovered — the core brightens a touch in sympathy with the globe. */
  active: boolean;
  /** True while the owning root agent is actively generating. */
  working?: boolean;
}) {
  const ringA = useRef<THREE.Group>(null!);
  const ringB = useRef<THREE.Group>(null!);
  const heart = useRef<THREE.Group>(null!);
  const ringMatA = useRef<THREE.LineBasicMaterial>(null!);
  const ringMatB = useRef<THREE.LineBasicMaterial>(null!);
  const heartMat = useRef<THREE.LineBasicMaterial>(null!);

  const motif = useMemo(() => motifFor(harness), [harness]);
  const base = useMemo(() => new THREE.Color(color as THREE.Color | string), [color]);
  // Rings sit a hair toward white so the cradle stays legible over the hue-matched
  // lattice; the heart stays the pure brand hue.
  const ringColor = useMemo(() => base.clone().lerp(WHITE, 0.3), [base]);

  const ringGeo = useMemo(() => ringGeometry(1.22), []);
  const heartGeo = useMemo(() => heartGeometry(motif), [motif]);

  useEffect(
    () => () => {
      ringGeo.dispose();
      heartGeo.dispose();
    },
    [ringGeo, heartGeo],
  );

  const sim = useRef({ glow: 1, dim: 0 });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    // slow gyroscope — authority, not alarm. Working agents get a small velocity
    // lift so the cradle reads more alive without turning into a spinner.
    const spin = agentCoreSpinMultiplier(working);
    const pulse = agentCorePulseState(working, Math.sin(t * 2.2));
    ringA.current.scale.setScalar(pulse.ringScale);
    ringB.current.scale.setScalar(pulse.ringScale);
    heart.current.scale.setScalar(pulse.heartScale);
    ringA.current.rotation.y += dt * 0.22 * spin;
    ringA.current.rotation.x += dt * 0.05 * spin;
    ringB.current.rotation.x += dt * 0.18 * spin;
    ringB.current.rotation.z -= dt * 0.07 * spin;
    heart.current.rotation.y += dt * 0.3;
    heart.current.rotation.z += dt * 0.11;

    const targetGlow = (active ? 1.35 : 1) * pulse.glowMultiplier * (1 + Math.sin(t * 1.4) * 0.03);
    const targetDim = dimmed ? 1 : 0;
    s.glow = THREE.MathUtils.lerp(s.glow, targetGlow, 1 - Math.exp(-5 * dt));
    s.dim = THREE.MathUtils.lerp(s.dim, targetDim, 1 - Math.exp(-6 * dt));

    const k = (1 - s.dim * 0.66) * s.glow;
    ringMatA.current.opacity = 0.34 * k;
    ringMatB.current.opacity = 0.28 * k;
    heartMat.current.opacity = 0.9 * k;
  });

  return (
    <group>
      {/* gyro ring cradle — two great circles on different axes */}
      <group ref={ringA}>
        <lineLoop geometry={ringGeo}>
          <lineBasicMaterial
            ref={ringMatA}
            color={ringColor}
            transparent
            opacity={0.34}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </lineLoop>
      </group>
      <group ref={ringB} rotation={[Math.PI * 0.5, 0, Math.PI * 0.18]}>
        <lineLoop geometry={ringGeo}>
          <lineBasicMaterial
            ref={ringMatB}
            color={ringColor}
            transparent
            opacity={0.28}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </lineLoop>
      </group>

      {/* brand heart — sunburst (Claude) / blossom (Codex) / neutral star */}
      <group ref={heart}>
        <lineSegments geometry={heartGeo}>
          <lineBasicMaterial
            ref={heartMat}
            color={base}
            transparent
            opacity={0.9}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </lineSegments>
      </group>
    </group>
  );
}

export default AgentCore;
