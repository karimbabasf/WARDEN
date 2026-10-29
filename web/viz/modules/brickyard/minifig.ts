// minifig.ts: a builder, made of the stock LDraw minifig parts and animated at the
// joints a real minifig has. No skeleton: every LDraw part's origin already sits on
// its joint, so a pivot group per joint is the whole rig.
//
// Joints: the head turns (yaw), each arm swings at the shoulder (pitch), each hand
// twists at the wrist, each leg swings at the hip. The body itself turns and bobs.
// A minifig arm has ONE joint, so a reach is a swing plus a lean, never an elbow.

import * as THREE from 'three';
import { LDU, loadTemplate } from './setModel';

export type MinifigPose = {
  /** Body yaw in radians (0 = facing the rig's +z). */
  turn: number;
  /** Forward lean of the whole figure, radians. */
  lean: number;
  /** Vertical bob, world units. */
  bob: number;
  headYaw: number;
  headPitch: number;
  /** Shoulder pitch: 0 hangs down, -PI/2 points forward. */
  armL: number;
  armR: number;
  wristL: number;
  wristR: number;
  /** Hip pitch, + forward. */
  legL: number;
  legR: number;
};

export const REST_POSE: MinifigPose = {
  turn: 0,
  lean: 0,
  bob: 0,
  headYaw: 0,
  headPitch: 0,
  armL: 0,
  armR: 0,
  wristL: 0,
  wristR: 0,
  legL: 0,
  legR: 0,
};

export type Minifig = {
  /** Place this in the world. Feet are at y = 0, facing +z. */
  root: THREE.Group;
  /** World-space anchor in the right hand, where a carried brick sits. */
  handR: THREE.Object3D;
  handL: THREE.Object3D;
  apply(p: MinifigPose): void;
};

/** LDU from a head's origin (its crown) down to its neck, where it sits on the torso. */
const NECK = 24;
/** Largest head nod, radians. */
const MAX_NOD = 0.14;

/** LDraw part files that make up the figure, in minifig.mpd order. */
const PART = {
  head: '3626bp01.dat',
  torso: '973.dat',
  armL: '3819.dat',
  armR: '3818.dat',
  hand: '3820.dat',
  hips: '3815.dat',
  legR: '3816.dat',
  legL: '3817.dat',
};

function partName(o: THREE.Object3D): string {
  return String(o.userData?.fileName ?? o.name ?? '').toLowerCase();
}

/** A pivot at `part`'s joint (its origin), holding the part with its rest rotation. */
function pivotFor(part: THREE.Object3D): THREE.Group {
  const pivot = new THREE.Group();
  pivot.position.copy(part.position);
  part.position.set(0, 0, 0);
  pivot.add(part);
  return pivot;
}

function tint(part: THREE.Object3D, mat: THREE.Material) {
  part.traverse((o) => {
    const m = o as THREE.Mesh;
    if (m.isMesh) {
      m.material = mat;
      m.castShadow = true;
    }
  });
}

/** Faces and headgear in minifig-extras.mpd. Each builder draws one of each by seed. */
const FACES = ['3626bp01.dat', '3626bp02.dat', '3626bp03.dat', '3626bp05.dat', '3626bp07.dat', '3626bp04.dat'];
const HAIR = ['3901.dat', '4530.dat', '92081.dat'];
const HAIR_COLORS = [0x4a2c1a, 0x1b1b1b, 0x8a5a2b, 0xc89b5a];

export type MinifigLook = {
  /** Shirt colour: the lead wears its harness hue, helpers a neutral. */
  torso: THREE.ColorRepresentation;
  legs: THREE.ColorRepresentation;
  /** The lead wears a construction helmet; helpers wear hair. */
  helmet: boolean;
  /** Picks the face, the hair and its colour, so a builder looks the same every time. */
  seed: number;
};

/**
 * Re-colour a part's MAIN colour only: printed detail (a vest, pockets, a face) keeps
 * the colours LDraw gave it, the way a real printed part would.
 */
function tintMain(part: THREE.Object3D, code: string, mat: THREE.Material) {
  part.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh) return;
    m.material = Array.isArray(m.material)
      ? m.material.map((x) => (x.userData?.code === code ? mat : x))
      : m.material.userData?.code === code
        ? mat
        : m.material;
  });
}

/** Build a builder from stock parts plus a face, headgear and a work vest. */
export async function makeMinifig(look: MinifigLook): Promise<Minifig> {
  const [template, extras] = await Promise.all([loadTemplate('minifig.mpd'), loadTemplate('minifig-extras.mpd')]);
  const src = template.clone(true);
  const parts = src.children.slice();
  const find = (file: string, nth = 0) => parts.filter((p) => partName(p).endsWith(file))[nth];
  const extra = (file: string) => {
    const hit = extras.children.find((p) => partName(p).endsWith(file));
    return hit ? hit.clone(true) : null;
  };

  const torsoMat = new THREE.MeshStandardMaterial({ color: look.torso, roughness: 0.3, metalness: 0 });
  const legMat = new THREE.MeshStandardMaterial({ color: look.legs, roughness: 0.34, metalness: 0 });

  const face = FACES[look.seed % FACES.length];
  let head = find(PART.head);
  const swapped = face === PART.head ? null : extra(face);
  if (swapped) {
    swapped.position.copy(head.position);
    head = swapped;
  }
  // The work vest: printed torso with pockets, its main colour the builder's shirt.
  const vest = extra('973p73.dat');
  const plain = find(PART.torso);
  const body = vest ?? plain;
  const armL = find(PART.armL);
  const armR = find(PART.armR);
  const hands = parts.filter((p) => partName(p).endsWith(PART.hand));
  // The left hand sits at +x (the figure's own left), the right at -x.
  const handL = hands.find((h) => h.position.x > 0) ?? hands[0];
  const handR = hands.find((h) => h.position.x < 0) ?? hands[1];
  const hips = find(PART.hips);
  const legL = find(PART.legL);
  const legR = find(PART.legR);

  if (vest) tintMain(body, '4', torsoMat);
  else tint(body, torsoMat);
  tint(armL, torsoMat);
  tint(armR, torsoMat);
  tint(hips, legMat);
  tint(legL, legMat);
  tint(legR, legMat);

  // LDraw space (Y down, origin at the top of the torso). Everything hangs off the torso.
  const torsoPivot = new THREE.Group();
  torsoPivot.add(body);
  body.position.set(0, 0, 0);
  // A head's LDraw origin is the top of its crown (it spans y 0..24 down to the neck),
  // so pivoting there swung the neck off the torso on every nod. Pivot at the neck.
  const headPivot = new THREE.Group();
  headPivot.position.copy(head.position).add(new THREE.Vector3(0, NECK, 0));
  head.position.set(0, -NECK, 0);
  headPivot.add(head);
  // Headgear shares the head's origin (the crown) and turns with it.
  const gear = look.helmet ? extra('3833.dat') : extra(HAIR[(look.seed >> 3) % HAIR.length]);
  if (gear) {
    gear.position.set(0, -NECK, 0);
    if (look.helmet) tint(gear, new THREE.MeshStandardMaterial({ color: 0xf4f4f0, roughness: 0.28 }));
    else tint(gear, new THREE.MeshStandardMaterial({ color: HAIR_COLORS[(look.seed >> 5) % HAIR_COLORS.length], roughness: 0.5 }));
    headPivot.add(gear);
  }
  const armLPivot = pivotFor(armL);
  const armRPivot = pivotFor(armR);
  // A hand rides its arm: re-express it relative to the shoulder, ignoring the arm's
  // small rest tilt, so swinging the shoulder carries the hand along.
  const handLPivot = new THREE.Group();
  handLPivot.position.copy(handL.position).sub(armLPivot.position);
  handL.position.set(0, 0, 0);
  handLPivot.add(handL);
  armLPivot.add(handLPivot);
  const handRPivot = new THREE.Group();
  handRPivot.position.copy(handR.position).sub(armRPivot.position);
  handR.position.set(0, 0, 0);
  handRPivot.add(handR);
  armRPivot.add(handRPivot);
  const legLPivot = pivotFor(legL);
  const legRPivot = pivotFor(legR);
  hips.position.set(hips.position.x, hips.position.y, hips.position.z);
  torsoPivot.add(headPivot, armLPivot, armRPivot, hips, legLPivot, legRPivot);

  // Grip anchors: a little inside each hand's C-clip.
  const gripR = new THREE.Object3D();
  gripR.position.set(0, 6, -6);
  handRPivot.add(gripR);
  const gripL = new THREE.Object3D();
  gripL.position.set(0, 6, -6);
  handLPivot.add(gripL);

  // Upright and to world scale. The feet sit 72 LDU below the torso origin.
  const lean = new THREE.Group();
  lean.add(torsoPivot);
  lean.rotation.order = 'YXZ';
  const ldraw = new THREE.Group();
  ldraw.add(lean);
  ldraw.rotation.x = Math.PI;
  ldraw.scale.setScalar(LDU);
  ldraw.position.y = 72 * LDU;
  const root = new THREE.Group();
  root.add(ldraw);
  // The model's front is LDraw -z; flipping about x turns that to +z in world space.

  const apply = (p: MinifigPose) => {
    // LDraw is Y-down and the flip about x mirrors pitch, so signs read inverted here.
    lean.rotation.y = -p.turn;
    lean.rotation.x = p.lean;
    ldraw.position.y = 72 * LDU + p.bob;
    // A real minifig head only turns; a small nod reads as looking down, a big one as broken.
    const nod = THREE.MathUtils.clamp(p.headPitch, -MAX_NOD, MAX_NOD);
    headPivot.rotation.set(-nod, -p.headYaw, 0, 'YXZ');
    armLPivot.rotation.x = -p.armL;
    armRPivot.rotation.x = -p.armR;
    handLPivot.rotation.z = p.wristL;
    handRPivot.rotation.z = p.wristR;
    legLPivot.rotation.x = -p.legL;
    legRPivot.rotation.x = -p.legR;
  };
  apply(REST_POSE);
  // Figures are few and they move: they are the one thing that casts a shadow.
  root.traverse((o) => {
    if ((o as THREE.Mesh).isMesh) o.castShadow = true;
  });

  return { root, handR: gripR, handL: gripL, apply };
}
