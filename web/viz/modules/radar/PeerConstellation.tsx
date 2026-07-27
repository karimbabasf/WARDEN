// PeerConstellation.tsx: somebody ELSE'S swarm, standing beside your own.
//
// Watching a peer is a SLIDE, not a swap. Their constellation is mounted in the same
// world as yours, parked to the right of it by `peerFraming.peerWorldPlacement`, and
// the camera trucks laterally across (CameraRig's `viewTarget`). Nothing implodes,
// nothing cross-fades, nothing remounts: you can see both boards and the distance
// between them, which is the whole point of "slide over to theirs".
//
// It renders through the SAME `RadarForest` the local board uses, deliberately. A peer
// globe must be the same object as yours, or comparing the two boards means nothing.
// What differs is only that it is READ-ONLY (`interactive={false}`: no hit-spheres, no
// hover card, no rail clicks) and that it is unmistakably FRAMED as not-yours.
//
// THE MARK. `.wd-observe-radar` in style.css already answers this question in 2D: a
// dashed amber border plus a 135deg hazard-stripe wash, because a redacted frame from
// someone else's machine must never read as your own panel. This carries that exact
// treatment into 3D as a curtain BEHIND the peer's board: a hazard-striped backdrop
// plane, a dashed amber perimeter, and the standard amber kicker naming whose board it
// is. It sits behind the globes, so it never fights the data it is labelling, and it
// is visible from the first pixel of the truck, so you can never arrive at the peer's
// board without having seen it declared.
//
// FSD: this is still a radar, so it lives in modules/radar and takes an ALREADY
// NORMALIZED scene model as a prop. It never reaches into modules/observe.

import { useEffect, useMemo, type CSSProperties } from 'react';
import * as THREE from 'three';
import { Html } from '@react-three/drei';
import type { LayoutNode } from '@/viz/shared/types/orbTypes';
import type { RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { FoldGroup } from '@/viz/shared/scene/Transition';
import { type Box } from '@/viz/shared/scene/cameraFraming';
import { RadarForest } from './RadarConstellation';
import { peerLayoutBox, type PeerPlacement } from './peerFraming';

// The amber the DOM uses for every "this came off another machine" surface (--warn).
const HAZARD_RGB: [number, number, number] = [255, 201, 77];
// Void black, matching the .wd-observe-radar backdrop wash.
const VOID_RGB: [number, number, number] = [2, 9, 6];

// Air between the peer's outermost globe and its frame, as a fraction of the board's
// smaller side, floored so a one-agent peer still gets a frame it can breathe inside.
const FRAME_PAD_FRACTION = 0.09;
const FRAME_PAD_MIN = 1.1;
// One stripe cycle in world units. Fixed, so the stripes stay the same physical size
// whatever the board's dimensions are and never stretch into a moire on a wide one.
const STRIPE_WORLD = 0.62;
// Perimeter dash rhythm, in world units.
const DASH_LEN = 0.5;
const DASH_GAP = 0.34;
// Behind the globes (the whole forest sits on z = 0), far enough back that the frame
// never z-fights a link but close enough to read as the same object.
const FRAME_Z = -1.4;

const noopNode = (_node: LayoutNode) => {};
const noop = () => {};
// Stand-in for the tab fold when the caller does not thread one (the dev harness).
const REST_SCALE = { current: 1 };

/**
 * Tileable 135deg hazard stripe: amber bands over a dark wash, the 3D reading of
 * `repeating-linear-gradient(135deg, ...)`. Pixels where `(x + y) % period` is small
 * form lines of slope -1, and both axes are periodic in `period`, so a square tile
 * whose side is a multiple of it repeats with no visible seam at any scale.
 */
function makeHazardTexture(): THREE.Texture {
  const SIZE = 32;
  const PERIOD = 8;
  const THICK = 2;
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = SIZE;
  const ctx = canvas.getContext('2d')!;
  const img = ctx.createImageData(SIZE, SIZE);
  const d = img.data;
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      const i = (y * SIZE + x) * 4;
      const onStripe = (x + y) % PERIOD < THICK;
      const [r, g, b] = onStripe ? HAZARD_RGB : VOID_RGB;
      d[i] = r; d[i + 1] = g; d[i + 2] = b;
      d[i + 3] = onStripe ? 56 : 92; // both translucent: a wash, never a solid panel
    }
  }
  ctx.putImageData(img, 0, 0);
  const tex = new THREE.CanvasTexture(canvas);
  tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
  tex.needsUpdate = true;
  return tex;
}

/**
 * Perimeter of a rectangle as discrete dash segments (positions for a `lineSegments`).
 *
 * Real gaps in the geometry rather than a dashed material: `LineDashedMaterial` needs
 * per-vertex line distances and still renders screen-space dashes that swim as you
 * dolly. Each edge divides into a whole number of dash+gap cells, so the rhythm meets
 * itself at every corner instead of leaving a ragged stub.
 */
export function dashedRectPositions(
  halfW: number,
  halfH: number,
  dash = DASH_LEN,
  gap = DASH_GAP,
): Float32Array {
  const corners: Array<[number, number]> = [
    [-halfW, -halfH], [halfW, -halfH], [halfW, halfH], [-halfW, halfH],
  ];
  const out: number[] = [];
  for (let e = 0; e < 4; e++) {
    const [x0, y0] = corners[e];
    const [x1, y1] = corners[(e + 1) % 4];
    const len = Math.hypot(x1 - x0, y1 - y0);
    if (len <= 0) continue;
    const cells = Math.max(2, Math.round(len / (dash + gap)));
    const step = len / cells;
    const lit = step * (dash / (dash + gap));
    for (let i = 0; i < cells; i++) {
      const ta = (i * step) / len;
      const tb = (i * step + lit) / len;
      out.push(
        x0 + (x1 - x0) * ta, y0 + (y1 - y0) * ta, 0,
        x0 + (x1 - x0) * tb, y0 + (y1 - y0) * tb, 0,
      );
    }
  }
  return new Float32Array(out);
}

/** The hazard curtain: striped backdrop + dashed perimeter + the "not yours" kicker. */
function PeerHazardFrame({ box, label }: { box: Box; label: string }) {
  const geom = useMemo(() => {
    const w = box.max[0] - box.min[0];
    const h = box.max[1] - box.min[1];
    const pad = Math.max(FRAME_PAD_MIN, Math.min(w, h) * FRAME_PAD_FRACTION);
    const halfW = w / 2 + pad;
    const halfH = h / 2 + pad;
    return {
      cx: (box.min[0] + box.max[0]) / 2,
      cy: (box.min[1] + box.max[1]) / 2,
      halfW,
      halfH,
      // Top-left of the frame, where the kicker sits (matching the DOM panel's head).
      kickerY: halfH + pad * 0.55,
    };
  }, [box]);

  const tex = useMemo(() => {
    const t = makeHazardTexture();
    t.repeat.set((geom.halfW * 2) / STRIPE_WORLD, (geom.halfH * 2) / STRIPE_WORLD);
    return t;
  }, [geom.halfW, geom.halfH]);
  useEffect(() => () => { tex.dispose(); }, [tex]);

  const borderGeo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(dashedRectPositions(geom.halfW, geom.halfH), 3));
    return g;
  }, [geom.halfW, geom.halfH]);
  useEffect(() => () => { borderGeo.dispose(); }, [borderGeo]);

  const hazardHex = `rgb(${HAZARD_RGB[0]}, ${HAZARD_RGB[1]}, ${HAZARD_RGB[2]})`;

  return (
    <group position={[geom.cx, geom.cy, FRAME_Z]}>
      <mesh>
        <planeGeometry args={[geom.halfW * 2, geom.halfH * 2]} />
        <meshBasicMaterial map={tex} transparent opacity={0.9} depthWrite={false} />
      </mesh>

      <lineSegments geometry={borderGeo}>
        <lineBasicMaterial color={hazardHex} transparent opacity={0.55} depthWrite={false} toneMapped={false} />
      </lineSegments>

      {/* The same amber kicker the observe panel wears, so the 2D and 3D readings of
          "this is a remote, redacted frame" are literally the same treatment. */}
      <Html
        position={[-geom.halfW, geom.kickerY, 0]}
        zIndexRange={[6, 0]}
        style={{ pointerEvents: 'none', whiteSpace: 'nowrap' } as CSSProperties}
      >
        <div className="wd-observe-radar-kicker">Observing {label}</div>
      </Html>
    </group>
  );
}

export type PeerConstellationProps = {
  /** An already-normalized scene model for the peer (the lead adapts ObservedState). */
  model: RadarSceneModel;
  /** Where the peer sits in world space, from `peerWorldPlacement(model, sceneBounds)`. */
  placement: PeerPlacement;
  /** Whose board this is, shown in the hazard kicker (already redacted upstream). */
  label: string;
  /** The shared fold scale, so a tab swap folds the peer's board with everything else. */
  scaleRef?: { current: number };
};

/**
 * The peer's constellation, parked beside the local one and marked as not-yours.
 *
 * `placement` is passed in rather than derived here so the component and the camera
 * rig agree by construction: the lead computes it once and hands `offsetX` here and
 * `placement.bounds` to `<CameraRig peerBounds=...>`.
 */
export function PeerConstellation({ model, placement, label, scaleRef }: PeerConstellationProps) {
  const box = useMemo(() => peerLayoutBox(model), [model]);
  return (
    <group position={[placement.offsetX, 0, 0]}>
      {/* Its own fold group: RadarForest already folds itself, and nesting the frame
          inside that would square the scale. */}
      {box && (
        <FoldGroup scaleRef={scaleRef ?? REST_SCALE}>
          <PeerHazardFrame box={box} label={label} />
        </FoldGroup>
      )}

      <RadarForest
        model={model}
        selectedId={null}
        hoveredId={null}
        scaleRef={scaleRef}
        interactive={false}
        onHover={noopNode}
        onLeave={noopNode}
        onSelect={noopNode}
        onClear={noop}
      />
    </group>
  );
}

export default PeerConstellation;
