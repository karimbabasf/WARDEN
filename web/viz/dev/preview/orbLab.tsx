// orbLab.tsx — standalone "orb studio" served by vite at /orbs.html.
//
// Karim picked the LATTICE direction. This is the refined study of that single
// orb: an emerald network shell with glowing nodes, a faceted crystal heart,
// floating in pure black space — inspected with a free camera (drag · scroll ·
// right-drag). No floor, no clutter.
//
// Deliberately self-contained — it imports nothing from the live war-room so it
// can never collide with parallel work on the app's interaction layer. Palette
// hexes are inlined for the same reason. Once the look is signed off, this orb
// graduates into the real Scene.

import { useEffect, useMemo, useRef } from 'react';
import { createRoot } from 'react-dom/client';
import * as THREE from 'three';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { OrbitControls, Environment, Lightformer, Sparkles, Wireframe } from '@react-three/drei';
import {
  EffectComposer,
  Bloom,
  Vignette,
  Noise,
  ChromaticAberration,
  ToneMapping,
} from '@react-three/postprocessing';
import { BlendFunction, ToneMappingMode } from 'postprocessing';

// ---- WARDEN phosphor palette (inlined to stay decoupled) --------------------
const BG = '#020403';
const EMERALD = '#3dffa0'; // primary phosphor
const MINT = '#c8ffe0'; // bright inner wire
const NODE = '#9bffc6'; // node glow

// A round, soft sprite (white → transparent) tinted per-use at the material.
function makeDotTexture(): THREE.Texture {
  const s = 64;
  const c = document.createElement('canvas');
  c.width = c.height = s;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
  g.addColorStop(0.0, 'rgba(255,255,255,1)');
  g.addColorStop(0.35, 'rgba(255,255,255,0.85)');
  g.addColorStop(1.0, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, s, s);
  const tex = new THREE.CanvasTexture(c);
  tex.needsUpdate = true;
  return tex;
}

// Wider, softer falloff for the core's radiant halo.
function makeGlowTexture(): THREE.Texture {
  const s = 128;
  const c = document.createElement('canvas');
  c.width = c.height = s;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
  g.addColorStop(0.0, 'rgba(255,255,255,1)');
  g.addColorStop(0.16, 'rgba(255,255,255,0.8)');
  g.addColorStop(0.45, 'rgba(255,255,255,0.24)');
  g.addColorStop(1.0, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, s, s);
  const tex = new THREE.CanvasTexture(c);
  tex.needsUpdate = true;
  return tex;
}

// =============================================================================
// Crystal heart — a faceted polyhedron gem, not a flat sphere. Flat shading +
// env reflections make every facet read in 3D; it glints as it turns. A soft
// halo behind keeps it reading as a glowing core; a tiny hot center anchors it.
// =============================================================================
function CrystalCore() {
  const gem = useRef<THREE.Mesh>(null!);
  const halo = useMemo(() => makeGlowTexture(), []);

  useFrame((_, dt) => {
    // counter-spin on two axes so facets catch the env from changing angles
    gem.current.rotation.y += dt * 0.5;
    gem.current.rotation.x += dt * 0.22;
  });

  return (
    <group>
      {/* soft aura sitting just behind the gem — reads as a glowing rim, not a
          flat disc, because the opaque gem occludes its centre */}
      <sprite scale={[0.85, 0.85, 1]} position={[0, 0, -0.1]}>
        <spriteMaterial
          map={halo}
          color={EMERALD}
          transparent
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
          opacity={0.35}
        />
      </sprite>

      {/* the gem: a polished emerald dodecahedron. metalness ~1 + flat shading
          means each facet mirrors a different part of the env — bright glints vs
          dark faces give the 3D read; a low emissive keeps an inner ember. */}
      <mesh ref={gem}>
        <icosahedronGeometry args={[0.33, 0]} />
        <meshStandardMaterial
          color={'#0f3f29'}
          emissive={EMERALD}
          emissiveIntensity={0.14}
          metalness={0.5}
          roughness={0.26}
          flatShading
          envMapIntensity={1.5}
        />
      </mesh>
    </group>
  );
}

// =============================================================================
// Lattice orb — dashed outer network shell, a counter-rotating inner cage, a
// glowing node at every outer vertex, and the crystal heart at the centre.
// =============================================================================
function LatticeOrb() {
  const grp = useRef<THREE.Group>(null!);
  const inner = useRef<THREE.Group>(null!);
  const geoOuter = useMemo(() => new THREE.IcosahedronGeometry(1, 2), []);
  const geoInner = useMemo(() => new THREE.IcosahedronGeometry(0.62, 1), []);
  const nodeGeo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', (geoOuter.attributes.position as THREE.BufferAttribute).clone());
    return g;
  }, [geoOuter]);
  const sprite = useMemo(() => makeDotTexture(), []);

  useFrame((_, dt) => {
    const t = performance.now() / 1000;
    grp.current.rotation.y += dt * 0.1;
    grp.current.position.y = Math.sin(t * 0.6) * 0.06; // gentle float
    inner.current.rotation.y -= dt * 0.24;
    inner.current.rotation.x += dt * 0.13;
  });

  return (
    <group ref={grp}>
      {/* outer network shell — dashed, thick, anti-aliased (drei shader wire) */}
      <Wireframe
        geometry={geoOuter}
        simplify
        stroke={EMERALD}
        thickness={0.03}
        dash
        dashRepeats={5}
        fillOpacity={0}
        backfaceStroke={'#0c3b22'}
      />

      {/* counter-rotating inner cage for parallax depth */}
      <group ref={inner}>
        <Wireframe geometry={geoInner} simplify stroke={MINT} thickness={0.04} fill={EMERALD} fillOpacity={0.05} />
      </group>

      <CrystalCore />

      {/* glowing graph node at every outer vertex */}
      <points geometry={nodeGeo}>
        <pointsMaterial
          size={0.16}
          map={sprite}
          color={NODE}
          transparent
          toneMapped={false}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          sizeAttenuation
        />
      </points>
    </group>
  );
}

// ---- pure black space: no floor, just env light + a faint mote field --------
function Space() {
  return (
    <>
      <color attach="background" args={[BG]} />
      <fogExp2 attach="fog" args={[BG, 0.014]} />
      {/* Key + rim aimed at the crystal. The cage/nodes are unlit basic
          materials, so these lights only sculpt the gem — giving every facet a
          normal-based brightness that never collapses to a flat shape. */}
      <ambientLight intensity={0.1} />
      <directionalLight position={[5, 6, 4]} intensity={2.4} color={'#e6fff0'} />
      <directionalLight position={[-6, -1, -2]} intensity={0.8} color={'#9fd0ff'} />

      {/* Environment children render ONLY into the reflection probe — they give
          the metal gem its facet-by-facet sheen, never appear in the scene. */}
      <Environment resolution={256}>
        <Lightformer form="rect" intensity={2.4} color={'#bfffe0'} position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.6} color={EMERALD} position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="ring" intensity={2.2} color={'#ffffff'} position={[2, 4, 2]} scale={[2.2, 2.2, 1]} />
        <Lightformer form="rect" intensity={0.8} color={'#1f8c9c'} position={[0, -4, 3]} scale={[8, 3, 1]} />
      </Environment>

      <Sparkles count={70} scale={[16, 12, 16]} size={1.4} speed={0.16} opacity={0.4} color={EMERALD} />
    </>
  );
}

// ---- free camera: drag orbit · scroll zoom · right-drag pan, no friction ----
function FreeCamera() {
  return (
    <OrbitControls
      makeDefault
      enableDamping
      dampingFactor={0.06}
      rotateSpeed={0.9}
      zoomSpeed={1.15}
      panSpeed={0.8}
      enablePan
      minDistance={2.2}
      maxDistance={18}
      target={[0, 0, 0]}
    />
  );
}

function Post() {
  const caOffset = useMemo(() => new THREE.Vector2(0.0006, 0.0006), []);
  return (
    <EffectComposer multisampling={4}>
      <Bloom mipmapBlur intensity={1.15} luminanceThreshold={0.18} luminanceSmoothing={0.85} radius={0.82} />
      <ToneMapping mode={ToneMappingMode.ACES_FILMIC} />
      <ChromaticAberration blendFunction={BlendFunction.NORMAL} offset={caOffset} radialModulation={false} modulationOffset={0} />
      <Vignette eskil={false} offset={0.22} darkness={0.86} />
      <Noise premultiply blendFunction={BlendFunction.OVERLAY} opacity={0.04} />
    </EffectComposer>
  );
}

function Exposure() {
  const gl = useThree((s) => s.gl);
  useEffect(() => {
    gl.toneMappingExposure = 1.1;
  }, [gl]);
  return null;
}

function OrbLab() {
  return (
    <>
      <Canvas
        dpr={[1, 2]}
        gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
        camera={{ position: [0, 0.3, 5.4], fov: 42, near: 0.1, far: 140 }}
      >
        <Exposure />
        <Space />
        <LatticeOrb />
        <FreeCamera />
        <Post />
      </Canvas>

      <div className="hud">
        <div className="hud-mark">
          <span className="sig">WARDEN</span>
          <span className="ver">orb · lattice</span>
        </div>
        <div className="hud-title">core study · drag to inspect</div>

        <div className="hud-hint">
          <span><b>drag</b> orbit</span>
          <span className="dot" />
          <span><b>scroll</b> zoom</span>
          <span className="dot" />
          <span><b>right-drag</b> pan</span>
        </div>
      </div>
    </>
  );
}

createRoot(document.getElementById('orb-root')!).render(<OrbLab />);
