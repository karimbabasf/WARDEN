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

// Hard cap on a star's on-screen size (CSS px, before DPR). The 1/z size law below
// balloons any star that drifts near the camera into a big soft blob (the "falling
// snowflakes"); clamp it so the sky always reads as fine dust. Far/mid stars already
// rasterize well under this, so the tiny twinkling field is left untouched.
const MAX_STAR_PX = 2.2;

const STAR_VERT = /* glsl */ `
  uniform float uTime;
  uniform float uPixelRatio;
  uniform float uSizeScale;
  uniform float uMaxSize;
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
    // distance attenuation so far shells stay genuinely tiny, then a hard cap so a
    // star drifting close to the camera can never balloon into a big soft blob.
    gl_PointSize = min(aSize * uSizeScale * uPixelRatio * (1.0 / -mv.z), uMaxSize);
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
          uMaxSize: { value: MAX_STAR_PX * pixelRatio },
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

/** The innermost shell, and the outermost edge of the outermost one. Derived rather
 *  than typed twice, so re-tuning LAYERS cannot leave a caller sizing the sky off a
 *  number that has moved. */
const INNER_R = Math.min(...LAYERS.map((l) => l.radius));
const OUTER_R = Math.max(...LAYERS.map((l) => l.radius + l.spread));

/**
 * The sky, optionally re-sized for a camera that is not the war room's.
 *
 * The shells above are sized in the war room's world units under a PERSPECTIVE camera.
 * The HUD draws the same globes ORTHOGRAPHICALLY in CSS pixels, and an ortho projection
 * has no perspective divide: apparent size is world size, so distance cannot make a
 * 33-unit shell fill a 460px panel. Dropped in unchanged, the sky is a small ball in
 * the corner of the box, technically present and never on screen.
 *
 * Rather than write a second starfield for the notch (the 2026-08-20 lesson about the
 * globes applies just as hard to the void they hang in: two implementations drift on
 * sight), the one catalog takes the numbers that change with the camera.
 *
 * `cover` scales the shell radii AND `sizeScale` together, so the sky grows to fill the
 * box without the stars growing into blobs: the shader's `1/-mv.z` law cancels the two
 * against each other, and `MAX_STAR_PX` still caps whatever is left.
 *
 * `density` is what keeps this honest on battery. 40,000 points is a room; the notch
 * section is a strip a few hundred pixels tall that sits open on somebody's display all
 * day, and it wants a few thousand.
 */
export function StarCatalog({
  cover,
  behind,
  density = 1,
  opacity = 1,
}: {
  /** World radius the INNERMOST shell must reach. Omit for the war room's own sky. */
  cover?: number;
  /** Push the whole sky at least this far behind z=0, so no shell can cross in front
   *  of what it is a backdrop for. Omit to leave it centred on the origin. */
  behind?: number;
  /** Fraction of the war room's star count. */
  density?: number;
  /** Multiplies every layer's opacity, for a panel that is not a dark room. */
  opacity?: number;
} = {}) {
  const motion = useMemo(
    () =>
      typeof window === 'undefined' ||
      !window.matchMedia?.('(prefers-reduced-motion: reduce)').matches,
    [],
  );

  const scale = cover != null && cover > 0 ? cover / INNER_R : 1;

  // Rebuilt only when the numbers change, which for any one mount is never: the
  // geometry is tens of thousands of points and `buildLayerGeometry` keys off the
  // whole spec.
  const layers = useMemo(
    () =>
      scale === 1 && density === 1 && opacity === 1
        ? LAYERS
        : LAYERS.map((spec) => ({
            ...spec,
            count: Math.max(1, Math.round(spec.count * density)),
            radius: spec.radius * scale,
            spread: spec.spread * scale,
            sizeScale: spec.sizeScale * scale,
            opacity: spec.opacity * opacity,
          })),
    [scale, density, opacity],
  );

  const z = behind != null ? -(OUTER_R * scale + behind) : 0;

  return (
    <group position={[0, 0, z]}>
      {layers.map((spec) => (
        <StarLayer key={spec.seed} spec={spec} motion={motion} />
      ))}
    </group>
  );
}

export default StarCatalog;
