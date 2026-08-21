// HudGlobe.tsx: the war room's globe, at 40px, and the light it needs to look it.
//
// There is no HUD globe any more. `RadarGlobeBody` is the one definition of an agent's
// body and both screens mount it; what is left here is the two things that are
// genuinely local to a menu-bar panel.
//
// 1. SCALE. The HUD's camera is ORTHOGRAPHIC and its units are CSS pixels, so a globe
//    is scaled to a pixel radius rather than a world one, and the lattice node dots
//    have to be handed a pixel size (three.js skips point-size attenuation entirely
//    under an ortho camera, so `size` stops meaning world units there).
//
// 2. LIGHT. The gem at a globe's heart is a `meshPhysicalMaterial` with transmission:
//    it is lit, not emissive-only, so without a lamp and an environment probe it
//    renders as a dark bead and the globe reads as a wire cage with a hole in it.
//    `HudSceneRig` is the war room's lighting, restated at the same values. It is
//    NOT a second look: the same numbers are what make the two screens match.
//
// Bloom is here for the same reason the lights are. A globe's core is emissive well
// past 1.0 and it is the BLOOM that turns that overflow into the white-hot blaze; the
// same globe drawn without it reads as a grey bead in a wire cage, which is exactly
// how the HUD used to differ from the war room. The pass is kept narrow (no vignette,
// which would darken a floating panel's corners, and a smaller mip radius, because the
// panel is 380px wide and not a room).

import { EffectComposer, Bloom } from '@react-three/postprocessing';
import { Environment, Lightformer } from '@react-three/drei';
import { RadarGlobeBody } from '@/viz/modules/radar/RadarGlobeBody';
import type { RadarStatus } from '@/viz/shared/types/radarTypes';

/**
 * Node-dot size for the HUD, in DEVICE PIXELS (see the note above about ortho
 * cameras). Tuned against the war room's apparent dot size at its default framing:
 * big enough to read as a lattice vertex, small enough not to become a bead.
 */
const HUD_NODE_PX = 2.4;
/** A moon's lattice is denser relative to its size, so its dots go down with it. */
const HUD_KID_NODE_PX = 1.6;

/**
 * The war room's lights and environment probe, restated for the HUD's canvas.
 *
 * Mount this ONCE per canvas, not once per globe: `Environment` builds a cube render
 * target, and one per globe would build fifteen of them.
 */
export function HudSceneRig() {
  return (
    <>
      {/* Lights sculpt only the crystal gem hearts (the cages/nodes are unlit
          emissive); the Environment probe gives each facet its glint. */}
      <ambientLight intensity={0.085} />
      <directionalLight position={[5, 6, 4]} intensity={2.1} color="#fff3e9" />
      <directionalLight position={[-6, -1, -2]} intensity={0.65} color="#bfe2ff" />
      <Environment resolution={64}>
        {/* one shared probe: Claude-tangerine, Codex-cyan, plus warm formers so every
            gem glints in its own hue without the void changing. */}
        <Lightformer form="rect" intensity={1.7} color="#ffcaa0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.4} color="#bfeaff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={1.0} color="#ffd9b8" position={[0, -3, -4]} scale={[6, 4, 1]} />
        <Lightformer form="ring" intensity={1.1} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>
    </>
  );
}

/**
 * The war room's bloom, at panel scale.
 *
 * Mounted as its own component and LAST in the canvas, because a composer renders the
 * whole scene: everything that should bloom has to already be in the tree.
 *
 * `EffectComposer` owns the canvas once mounted, and this canvas has to stay
 * transparent (the panel's own material and backdrop blur are behind it). That is
 * what `renderPriority` and the composer's alpha-preserving default buffer give us;
 * if the panel ever paints as a black rectangle, this pass is the first suspect.
 */
export function HudBloom() {
  return (
    <EffectComposer multisampling={4} renderPriority={1}>
      <Bloom intensity={1.3} luminanceThreshold={0.22} luminanceSmoothing={0.9} mipmapBlur radius={0.6} />
    </EffectComposer>
  );
}

/**
 * One agent's body, sized for the HUD.
 *
 * The caller positions it (see `HudGlobeSlot` in HudPanel); this only sets the pixel
 * radius and hands the shared body the two numbers that do not survive the change of
 * camera.
 */
export function HudGlobe({
  id,
  harness,
  status,
  radius,
  isRoot,
  hovered = false,
  reduced = false,
}: {
  id: string;
  harness: string;
  status: RadarStatus;
  /** Body radius in CSS px (see `hudGlobeRadius` / `HUD_KID_RADIUS`). */
  radius: number;
  isRoot: boolean;
  hovered?: boolean;
  reduced?: boolean;
}) {
  return (
    <group scale={radius}>
      <RadarGlobeBody
        id={id}
        harness={harness}
        status={status}
        isRoot={isRoot}
        hovered={hovered}
        reduced={reduced}
        nodeSize={isRoot ? HUD_NODE_PX : HUD_KID_NODE_PX}
      />
    </group>
  );
}
