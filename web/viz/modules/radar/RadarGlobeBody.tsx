// RadarGlobeBody.tsx: one agent's BODY, and the only definition of it.
//
// The war room and the menu-bar HUD draw the same object, so they draw it from the
// same file. This used to be two implementations: the constellation's lattice-shell
// plus crystal-heart globe, and a cheaper additive line sketch in the HUD. They
// drifted on sight, which is exactly what makes two screens read as two products.
//
// What lives HERE is everything that decides how a globe LOOKS and how it moves in
// place: the shell, the inner cage, the node dots, the gem heart, the halo, the
// harness core, and the per-frame heat that colours all of them. What does NOT live
// here is where a globe IS: layout position, the spawn/implode lifecycle scale, the
// hit sphere, and the tether registry all stay with the caller.
//
// The split is a group boundary, not a compromise:
//   caller's group     position, and the overall scale (layout radius, or HUD px)
//   this file's group   spin and breath, which multiply into it
// Two nested groups multiply to exactly what the single group used to do.
//
// Scale-independence is the whole point of the seam. The war room draws at world
// scale (a globe is about 0.5 units across); the HUD draws in CSS px under an
// orthographic camera (a globe is about 15 px). Everything below is expressed
// relative to a unit radius, so the caller's scale is the only thing that changes.
// The one exception is `nodeSize` (see the prop).

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import { Wireframe } from '@react-three/drei';
import * as THREE from 'three';
import type { RadarAgent, RadarStatus } from '@/viz/shared/types/radarTypes';
import { AgentCore } from '@/viz/shared/scene/AgentCore';
import { radarHarness } from './radarTheme';
import { globeSpinRate } from './radarMotion';
import { ALERT_HEX, alertBlink, alertGlowMultiplier, alertWhiteMix } from './radarAlert';

export const WHITE = new THREE.Color('#ffffff');

/** A stable 0..1 per-id phase, so the forest breathes organically rather than in
 *  lockstep. Pure: the same id always gets the same phase, on both screens. */
export function seedOf(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 1000;
  return h / 1000;
}

// soft round sprite (cached): the gem halo + the globe lattice nodes.
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
export const glowTexture = () => (glowCache ??= radialTexture(128, [[0, 1], [0.18, 0.8], [0.5, 0.22], [1, 0]]));
let dotCache: THREE.Texture | null = null;
export const dotTexture = () => (dotCache ??= radialTexture(48, [[0, 1], [0.4, 0.75], [1, 0]]));

/**
 * The colour of a radar globe: its harness hue, FLAT. Colour is identity only,
 * never load and never liveness. Fill drives SIZE (the layout radius) and working
 * drives BRIGHTNESS (the per-frame blaze), so the hue itself stays constant and a
 * harness reads the same whether it's busy or idle. Pure + exported so it is
 * unit-tested without WebGL (the house pattern).
 */
export function radarNodeColor(agent: RadarAgent): string {
  return radarHarness(agent.harness).color;
}

/**
 * The glow TARGET a globe damps toward: the single brightness signal, and it is
 * LIVENESS, full stop. A working globe blazes (a big `liveLift`); an idle/closed one
 * falls back to a deliberately dim resting floor so it sinks below the bloom
 * threshold and the working ones are the only things that light the room. Fill is
 * intentionally absent: context is the SIZE channel, not the brightness channel.
 * Selection/hover/legend-emphasis add on top so the focused globe still pops.
 *
 * An AWAITING globe takes a lift of its own, between resting and working. It is the
 * BASE the strobe then swings around (see `alertGlowMultiplier`), so the trough dips
 * under an idle globe and the crest clears a working one: the flash sweeps through the
 * whole board's range rather than sitting on top of it.
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
  const awaiting = agent.status === 'awaiting';
  // Dim resting floor (idle) vs a strong live blaze (working). The ~9x gap is what
  // makes a running agent unmistakable against the dulled-down rest of the forest.
  const restFloor = isRoot ? 0.22 : 0.16;
  const liveLift = working ? 2.7 : awaiting ? 1.5 : 0;
  return Math.max(
    0.05,
    restFloor +
      liveLift +
      (emphasis ? 0.6 : 0) +
      (selected ? 1.0 : hovered ? 0.4 : 0),
  );
}

export function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.lerp(current, target, 1 - Math.exp(-lambda * dt));
}

// ~300 ms time-constant for the legend colour-dim crossfade. `damp(.., DIM_LAMBDA, dt)`
// equals the brief's `cur += (target-cur)*(1 - exp(-dt/0.3))` (lambda = 1/0.3).
export const DIM_LAMBDA = 1 / 0.3;

// Eased dim float to a multiplicative colour scale: 1.0 at dim=0 (untouched),
// `floor` at dim=1 (matching the old boolean `multiplyScalar` endpoints). Colour
// only: callers copy a base colour, scale it, write it back; opacity/scale/
// geometry/position are never touched.
export function dimScale(dim: number, floor: number): number {
  return 1 - dim * (1 - floor);
}

/** Colour-only liveness scale for material hues: idle recedes, working restores full colour. */
export function radarLivenessColorScale(liveK: number): number {
  const k = Math.min(1, Math.max(0, liveK));
  return 0.62 + k * 0.38;
}

// drei's <Wireframe geometry=..> renders a private <mesh><meshWireframeMaterial/>
// and forwards no material ref, so cache the MeshWireframeMaterial (a ShaderMaterial
// whose `stroke`/`fill` colour uniforms we tint per frame) by traversing a wrapper
// group once.
export function findWireframeMaterial(root: THREE.Object3D | null): THREE.ShaderMaterial | null {
  if (!root) return null;
  let found: THREE.ShaderMaterial | null = null;
  root.traverse((o) => {
    if (found) return;
    const mat = (o as THREE.Mesh).material as THREE.ShaderMaterial | undefined;
    if (mat?.uniforms?.stroke?.value instanceof THREE.Color) found = mat;
  });
  return found;
}

/** The node-dot size a PERSPECTIVE camera wants: world units, attenuated by depth. */
export const NODE_SIZE_WORLD = { root: 0.08, sub: 0.07 };

export function RadarGlobeBody({
  id,
  harness,
  status,
  isRoot,
  selected = false,
  hovered = false,
  dimmed = false,
  dimTarget = 0,
  emphasis = false,
  reduced = false,
  nodeSize,
}: {
  /** Only used to phase this globe's breath; never rendered. */
  id: string;
  harness: string;
  status: RadarStatus;
  /** Roots get the denser lattice, the solid (not dashed) shell, and the harness core. */
  isRoot: boolean;
  selected?: boolean;
  hovered?: boolean;
  dimmed?: boolean;
  /** Legend colour-only filter, 0 = full colour .. 1 = fully dimmed. Eased per frame
   *  and applied to COLOUR ONLY (never scale/opacity/geometry). */
  dimTarget?: number;
  /** This globe MATCHES the active legend harness filter: a gentle extra glow so the
   *  selection POPS, not just dims everything else. */
  emphasis?: boolean;
  /** prefers-reduced-motion: hold the resting spin rate whatever the status is. */
  reduced?: boolean;
  /**
   * `pointsMaterial.size` for the lattice node dots. Defaults to the war room's
   * world-space value, which its PERSPECTIVE camera attenuates by depth.
   *
   * This is the one number a caller has to restate, and the reason is in three.js,
   * not here: the points shader skips size attenuation entirely under an
   * ORTHOGRAPHIC camera, so `size` stops meaning world units and starts meaning raw
   * device pixels. The HUD draws orthographically and therefore passes pixels.
   */
  nodeSize?: number;
}) {
  const body = useRef<THREE.Group>(null!);
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

  const working = status === 'working';
  const terminated = status === 'terminated';
  // The THIRD state: stopped on the operator. It takes the globe's colour AND its
  // waveform (see `radarAlert`), because a state that only changed brightness would read
  // as a busier working globe and one that only changed colour would be invisible to a
  // colour-blind operator.
  const awaiting = status === 'awaiting';
  // A finishing subagent flares verdict-amber as the lifecycle implodes its scale.
  const baseHex = terminated ? '#ff5a37' : awaiting ? ALERT_HEX : radarHarness(harness).color;
  const seed = useMemo(() => seedOf(id), [id]);

  // Colour depends only on harness hue (or the amber terminated flare); key on the
  // resolved hex (not identity) to avoid rebuilding the THREE.Color every frame.
  const color = useMemo(() => new THREE.Color(baseHex), [baseHex]);
  const innerColor = useMemo(() => color.clone().lerp(WHITE, 0.24), [color]);
  const nodeColor = useMemo(() => color.clone().lerp(WHITE, 0.16), [color]);
  // Far-hemisphere lattice lines: a very dark tint of the globe's OWN hue (never the
  // old phosphor green) so the back of the sphere reads as a dim echo of the front,
  // not a chartreuse cast where it blends with the orange/cyan front lines + bloom.
  const backStroke = useMemo(() => `#${color.clone().multiplyScalar(0.16).getHexString()}`, [color]);

  // Shell/inner BASE colour (hover/select + the boolean `dimmed`). The eased
  // legend dim multiplies ON TOP of these every frame in useFrame, colour only.
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
  const dotSize = nodeSize ?? (isRoot ? NODE_SIZE_WORLD.root : NODE_SIZE_WORLD.sub);

  useEffect(
    () => () => {
      outerGeo.dispose();
      innerGeo.dispose();
      gemGeo.dispose();
      nodeGeo.dispose();
    },
    [outerGeo, innerGeo, gemGeo, nodeGeo],
  );

  // `live` is the eased liveness factor (0 idle .. 1 working), the brightness
  // channel. `glow` is the damped emissive target; `colorDim` the legend filter.
  const sim = useRef({ glow: 0.5, live: working ? 1 : 0, dim: 0, colorDim: 0 });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;

    // The slow, smooth "alive" pulse: a calm ~4.2s sine (deliberately NOT snappy)
    // that only a working globe rides. It drives a SYNCED scale + halo + glow swell
    // below, so a running agent reads as softly breathing light, the wow that makes
    // "this is working" unmistakable. Per-globe `seed` phases it so the forest breathes
    // organically rather than strobing in lockstep.
    const PULSE_RATE = 1.5; // rad/s, so about a 4.2s period
    const pulseWave = Math.sin(t * PULSE_RATE + seed * 6.28); // -1..1 (gated by liveK below)
    // idle keeps a slow, deep ambient breath; working swaps to the synced pulse.
    const idleBreath = Math.sin(t * 0.5 + seed * 6.28) * 0.045;
    // The alert strobe. Deliberately NOT phased by `seed` the way the breath above is:
    // every waiting globe flashes on the same beat, so a board with three of them reads
    // as one alarm instead of three twitches.
    const blink = awaiting ? alertBlink(t, reduced) : 0;
    const alertGlow = awaiting ? alertGlowMultiplier(blink) : 1;

    // BRIGHTNESS = LIVENESS. `live` eases toward 1 while working, 0 while idle, and
    // drives the whole blaze (emissive, halo, white-hot core, lattice brightness).
    // `glow` is the damped emissive target from radarGlowTarget (also liveness-led).
    // Neither reads fill: context is the SIZE channel, and the caller owns size.
    const targetLive = working ? 1 : 0;
    const targetGlow = radarGlowTarget({ agent: { status }, isRoot, emphasis, selected, hovered });
    const targetDim = dimmed ? 1 : 0;
    // colourDim = the legend filter OR the boolean other-selected dim, whichever is
    // stronger. Idle dullness is NOT folded in here (it lives in `live`), so the two
    // signals stay cleanly separable.
    const targetColorDim = Math.max(targetDim, Math.min(1, Math.max(0, dimTarget)));

    s.glow = damp(s.glow, targetGlow, 5, dt);
    s.live = damp(s.live, targetLive, 3.5, dt);
    s.dim = damp(s.dim, targetDim, 6, dt);
    s.colorDim = damp(s.colorDim, targetColorDim, DIM_LAMBDA, dt);

    const liveK = s.live; // 0 idle .. 1 working, the brightness channel
    const pulse = liveK * pulseWave; // gated by liveness: 0 when idle, +/-liveK when working

    // scale: the idle ambient breath fades out as the globe wakes; working rides a
    // gentle synced swell of the slow pulse instead (smooth, about +/-4.5%). This
    // multiplies into whatever scale the CALLER set on its own group.
    body.current.scale.setScalar(1 + idleBreath * (1 - liveK) + pulse * 0.045);
    // spin: a working globe turns faster. `liveK` is already damped, so the rate
    // GLIDES between resting and working and never snaps on a status flip.
    body.current.rotation.y += dt * globeSpinRate(isRoot, liveK, reduced);

    // halo: comes alive with liveness, then breathes IN and OUT on the slow pulse.
    // The soft aura swelling and receding is the most visible "this is working" tell.
    // An awaiting globe overrides that with the strobe: the aura snaps out on each flash
    // and collapses back between them, which is the beacon read at halo scale.
    const haloBase = isRoot ? 0.74 : 0.58;
    halo.current.scale.setScalar(
      awaiting
        ? haloBase * (1 + blink * 1.25)
        : haloBase * (1 + liveK * 0.95 + pulse * 0.45),
    );

    innerCage.current.rotation.y -= dt * 0.18;
    innerCage.current.rotation.x += dt * 0.1;
    gem.current.rotation.y += dt * 0.28;
    gem.current.rotation.x += dt * 0.12;

    // ── brightness = liveness ───────────────────────────────────────────────
    // dimK: boolean other-selected opacity track. litK: the legend filter ALSO
    // crushes opacity/emissive (not just colour) so a filtered-out globe sinks below
    // the bloom threshold to near-dark, while a match keeps its full halo + blooms.
    const dimK = 1 - s.dim * 0.6;
    const litK = 1 - s.colorDim * 0.86;
    // the SAME slow pulse swells the emissive + halo + nodes together, so the whole
    // globe brightens and dims as one calm breath of light (working only: `pulse` is
    // 0 when idle, so idle globes hold perfectly steady and the contrast is obvious).
    // `alertGlow` is 1 for every other status, so the strobe rides the SAME three
    // channels the breath does rather than adding a fourth one to reason about.
    const pulseGlow = (1 + pulse * 0.3) * alertGlow;
    gemMat.current.emissiveIntensity = (0.3 + s.glow * 1.05) * dimK * litK * pulseGlow;
    haloMat.current.opacity = Math.min(
      1,
      (0.05 + s.glow * 0.34) * dimK * litK * (awaiting ? alertGlow : 1 + pulse * 0.45),
    );
    nodeMat.current.opacity = Math.min(1, (0.16 + s.glow * 0.32) * dimK * litK * pulseGlow);

    // ── colour: hue dulls when idle, blazes white-hot when working ───────────
    // Copy each material's base colour, fold in liveness (idle = a dim hue, working
    // = brighter + lerped toward white-hot), then scale by the eased legend dim: its
    // floor is low so a filtered-out globe goes dark. Copy-first so nothing compounds.
    const shellScaleC = dimScale(s.colorDim, 0.08);
    const innerScaleC = dimScale(s.colorDim, 0.1);
    // An awaiting globe's LATTICE strobes too, not just its core: driving the shell from
    // the blink is what makes the whole body flash rather than a bright dot inside a
    // dark cage. It is the same expression as the working lattice, fed a different wave.
    const shellLit = awaiting ? 0.34 + blink * 0.66 : 0.32 + liveK * 0.68;
    const colorQuiet = awaiting ? 0.7 + blink * 0.3 : radarLivenessColorScale(liveK);
    // working core, whitening a touch on each pulse peak. The alert stays RED at its
    // crest (a small mix) so a flash never reads as a white-hot working core.
    const whiteHot = awaiting ? alertWhiteMix(blink) : liveK * 0.5 + pulse * 0.12;
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
    <group ref={body}>
      {/* outer glowing network shell: root = solid lattice, sub = dashed */}
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
          // lattice (orange+green gives the chartreuse cast on Claude globes). Point
          // it at the globe's own hue so the face-fill can never reintroduce green.
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
          size={dotSize}
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

      {/* orchestrator signature: a ROOT agent (depth 0) is the one spawning the
          orbiting subagent moons, so it wears the same gyro cradle + brand heart as
          its Habits hub. Subagents stay bare lattices. Heat-coloured to match. */}
      {isRoot && (
        <AgentCore harness={harness} color={color} dimmed={dimmed} active={working || selected || hovered} working={working} />
      )}
    </group>
  );
}
