// StarCatalog.tsx — the deep-space sky the war room lives inside.
//
// This replaces drei's <Stars>/<Sparkles> (a few thousand chunky, fast-drifting
// motes that read as foreground confetti) with a custom multi-layer star CATALOG:
// ~8k sub-pixel points spread across three nested spherical shells. The point is
// *recession* — you read the data first, then notice the sky breathing behind it.
//
//   • depth        three shells (far/mid/near) drifting at different glacial rates
//                  → real parallax, not a flat backdrop.
//   • density      many, fine, faint — a field, never confetti.
//   • palette      cool blue-white dust with a *barely-there* scatter of harness
//                  coral/teal, so the sky subliminally belongs to WARDEN.
//   • motion       an order of magnitude slower than the old field, plus a soft
//                  per-star twinkle; frozen entirely under prefers-reduced-motion.
//
// Pure ambiance: this is the one layer that is NOT a data signal — it's the void
// the signals hang in, and it stays deliberately subordinate (low alpha, tiny
// points, renderOrder -1) so it never competes with the lattice orbs.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import * as THREE from 'three';

// Deterministic RNG (mulberry32) so the sky is stable across re-renders / HMR
// instead of re-shuffling every mount.
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const COLOR_WHITE = new THREE.Color('#cfe2ff'); // cool blue-white base
const COLOR_CORAL = new THREE.Color('#ff7d50'); // Claude tint (rare)
const COLOR_TEAL = new THREE.Color('#2de2c0'); // Codex tint (rare)
const _scratch = new THREE.Color();

const STAR_VERT = /* glsl */ `
  uniform float uTime;
  uniform float uPixelRatio;
  uniform float uSizeScale;
  uniform float uTwinkleAmp;
  attribute float aSize;
  attribute float aPhase;
  attribute float aTwinkle;
  attribute vec3 aColor;
  varying vec3 vColor;
  varying float vAlpha;
  void main() {
    vColor = aColor;
    // soft per-star twinkle (kept gentle; amplitude drops to ~0 for reduced motion)
    float tw = 1.0 - uTwinkleAmp + uTwinkleAmp * (0.5 + 0.5 * sin(uTime * aTwinkle + aPhase));
    vAlpha = tw;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // distance attenuation so far shells stay genuinely tiny
    gl_PointSize = aSize * uSizeScale * uPixelRatio * (1.0 / -mv.z);
    gl_Position = projectionMatrix * mv;
  }
`;

const STAR_FRAG = /* glsl */ `
  uniform float uOpacity;
  varying vec3 vColor;
  varying float vAlpha;
  void main() {
    // round, soft-edged point — no texture fetch
    vec2 d = gl_PointCoord - 0.5;
    float r = length(d);
    if (r > 0.5) discard;
    float soft = smoothstep(0.5, 0.06, r);
    gl_FragColor = vec4(vColor, soft * vAlpha * uOpacity);
  }
`;

type LayerSpec = {
  count: number;
  radius: number; // shell radius
  spread: number; // radial jitter so the shell has thickness
  sizeScale: number; // overall point-size multiplier for this shell
  sizeMin: number;
  sizeMax: number;
  opacity: number;
  drift: number; // radians/sec of y-rotation (glacial)
  tilt: number; // radians/sec of x-rotation (even slower wobble)
  seed: number;
};

function buildLayerGeometry(spec: LayerSpec): THREE.BufferGeometry {
  const rng = mulberry32(spec.seed);
  const { count } = spec;
  const positions = new Float32Array(count * 3);
  const colors = new Float32Array(count * 3);
  const sizes = new Float32Array(count);
  const phases = new Float32Array(count);
  const twinkles = new Float32Array(count);

  for (let i = 0; i < count; i++) {
    // even direction on the sphere (avoid pole clustering), jittered radius
    const u = rng();
    const v = rng();
    const theta = 2 * Math.PI * u;
    const phi = Math.acos(2 * v - 1);
    const r = spec.radius + (rng() - 0.5) * 2 * spec.spread;
    const sinPhi = Math.sin(phi);
    positions[i * 3] = r * sinPhi * Math.cos(theta);
    positions[i * 3 + 1] = r * sinPhi * Math.sin(theta);
    positions[i * 3 + 2] = r * Math.cos(phi);

    // colour: mostly cool white, a small warm/cool harness scatter at low saturation.
    // brightness skewed low (rng²) → a sea of faint dust with a few bright accents,
    // the depth-of-field a real sky has (vs a flat wash of identical dots).
    const tint = rng();
    const brightness = 0.32 + rng() * rng() * 0.95;
    if (tint > 0.93) {
      _scratch.copy(COLOR_WHITE).lerp(COLOR_CORAL, 0.45);
    } else if (tint > 0.86) {
      _scratch.copy(COLOR_WHITE).lerp(COLOR_TEAL, 0.45);
    } else {
      _scratch.copy(COLOR_WHITE);
    }
    colors[i * 3] = _scratch.r * brightness;
    colors[i * 3 + 1] = _scratch.g * brightness;
    colors[i * 3 + 2] = _scratch.b * brightness;

    // size skewed small (rng^2) so most stars are dust, a few slightly larger
    const sk = rng() * rng();
    sizes[i] = spec.sizeMin + (spec.sizeMax - spec.sizeMin) * sk;
    phases[i] = rng() * Math.PI * 2;
    twinkles[i] = 0.25 + rng() * 0.9;
  }

  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  g.setAttribute('aColor', new THREE.BufferAttribute(colors, 3));
  g.setAttribute('aSize', new THREE.BufferAttribute(sizes, 1));
  g.setAttribute('aPhase', new THREE.BufferAttribute(phases, 1));
  g.setAttribute('aTwinkle', new THREE.BufferAttribute(twinkles, 1));
  return g;
}

function StarLayer({ spec, motion }: { spec: LayerSpec; motion: boolean }) {
  const ref = useRef<THREE.Points>(null);
  const pixelRatio = typeof window !== 'undefined' ? Math.min(window.devicePixelRatio, 2) : 1;

  const geo = useMemo(() => buildLayerGeometry(spec), [spec]);
  const mat = useMemo(
    () =>
      new THREE.ShaderMaterial({
        uniforms: {
          uTime: { value: 0 },
          uPixelRatio: { value: pixelRatio },
          uSizeScale: { value: spec.sizeScale },
          uOpacity: { value: spec.opacity },
          uTwinkleAmp: { value: motion ? 0.45 : 0.12 },
        },
        vertexShader: STAR_VERT,
        fragmentShader: STAR_FRAG,
        transparent: true,
        depthWrite: false,
        blending: THREE.AdditiveBlending,
      }),
    [spec, pixelRatio, motion],
  );

  useEffect(() => () => { geo.dispose(); mat.dispose(); }, [geo, mat]);

  useFrame((state, dt) => {
    mat.uniforms.uTime.value = state.clock.elapsedTime;
    if (motion && ref.current) {
      ref.current.rotation.y += dt * spec.drift;
      ref.current.rotation.x += dt * spec.tilt;
    }
  });

  return <points ref={ref} geometry={geo} material={mat} renderOrder={-1} frustumCulled={false} />;
}

// Three shells tuned so the *far* layer is the dense fine dust and the *near*
// layer is a sparser, slightly larger parallax foreground. All drift rates are
// ~10× slower than the old starfield's speed=0.1. sizeScale is calibrated so even
// far-shell points rasterize at ≳1px (sub-1px points silently vanish on the GPU).
const LAYERS: LayerSpec[] = [
  { count: 22000, radius: 76, spread: 12, sizeScale: 108, sizeMin: 0.8, sizeMax: 1.9, opacity: 0.36, drift: 0.0045, tilt: 0.0016, seed: 0x51ed1 },
  { count: 12000, radius: 52, spread: 9, sizeScale: 80, sizeMin: 0.8, sizeMax: 2.1, opacity: 0.44, drift: 0.0075, tilt: 0.0026, seed: 0x9a2b7 },
  { count: 6000, radius: 33, spread: 7, sizeScale: 56, sizeMin: 0.8, sizeMax: 1.9, opacity: 0.52, drift: 0.0125, tilt: 0.004, seed: 0x1c0de },
];

export function StarCatalog() {
  const motion = useMemo(
    () =>
      typeof window === 'undefined' ||
      !window.matchMedia?.('(prefers-reduced-motion: reduce)').matches,
    [],
  );

  return (
    <group>
      {LAYERS.map((spec) => (
        <StarLayer key={spec.seed} spec={spec} motion={motion} />
      ))}
    </group>
  );
}

export default StarCatalog;
