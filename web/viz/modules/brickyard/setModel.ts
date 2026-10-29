// setModel.ts: loads a packed LDraw set once, and hands each bench its own copy with
// the bricks listed in build order.
//
// A "brick" is one LDraw part instance (a Part or Shortcut group). Its build order is
// the loader's own traversal order, which is the order `computeBuildingSteps` numbers
// the instruction steps in, so brick N always belongs to the step the book shows for
// it. Clones share geometry and materials, so a fourth bench on the same set costs
// one scene graph, not a second parse.

import * as THREE from 'three';
import { LDrawLoader } from 'three/examples/jsm/loaders/LDrawLoader.js';
import { LDrawConditionalLineMaterial } from 'three/examples/jsm/materials/LDrawConditionalLineMaterial.js';
import { assetUrl } from './catalog';
import type { SetShape, SubRange } from './planner';

/** World units per LDraw unit. A minifig is about 120 LDU, so it stands 1.0 tall. */
export const LDU = 1 / 120;

let loaderPromise: Promise<LDrawLoader> | null = null;

function loader(): Promise<LDrawLoader> {
  if (!loaderPromise) {
    loaderPromise = (async () => {
      const l = new LDrawLoader();
      l.setConditionalLineMaterial(LDrawConditionalLineMaterial as never);
      l.smoothNormals = true;
      await l.preloadMaterials(assetUrl('LDConfig.ldr'));
      return l;
    })();
  }
  return loaderPromise;
}

const cache = new Map<string, Promise<THREE.Group>>();

/** Parse a packed file once. The result is a template: never add it to a scene. */
export function loadTemplate(file: string): Promise<THREE.Group> {
  let p = cache.get(file);
  if (!p) {
    p = loader().then(async (l) => {
      const g = (await l.loadAsync(assetUrl(file))) as THREE.Group;
      prepare(g);
      return g;
    });
    cache.set(file, p);
  }
  return p;
}

/** One-time touches on a template, inherited by every clone. */
function prepare(g: THREE.Group) {
  // Conditional (silhouette) lines are a third of all draw calls and cost a custom
  // vertex shader each; hard edges alone keep the instruction-book look.
  const conditional: THREE.Object3D[] = [];
  g.traverse((o) => {
    if ((o as unknown as { isConditionalLine?: boolean }).isConditionalLine) conditional.push(o);
    else if ((o as THREE.Mesh).isMesh) {
      // Bricks take shadows but do not cast them: the shadow pass would double every call.
      o.castShadow = false;
      o.receiveShadow = true;
    } else if ((o as THREE.LineSegments).isLineSegments) {
      // Edges are drawn, never picked: only faces answer a click.
      o.raycast = () => {};
    }
  });
  for (const o of conditional) o.removeFromParent();
}

function isBrickGroup(o: THREE.Object3D): boolean {
  const t = o.userData?.type as string | undefined;
  return !!t && /part|shortcut/i.test(t);
}

/** Bricks in build order. Never descends into a brick (its subparts are geometry). */
export function collectBricks(root: THREE.Object3D): THREE.Object3D[] {
  const out: THREE.Object3D[] = [];
  const visit = (o: THREE.Object3D) => {
    if (o !== root && isBrickGroup(o)) {
      out.push(o);
      return;
    }
    for (const c of o.children) visit(c);
  };
  visit(root);
  return out;
}

/**
 * The submodels a helper may claim: the model children of the first level that has
 * more than one of them, minus the largest (that one is the lead's main build).
 * Only submodels of 3+ bricks count, so a helper never walks in for a single tile.
 */
export function submodelRanges(root: THREE.Object3D, bricks: THREE.Object3D[]): SubRange[] {
  const index = new Map(bricks.map((b, i) => [b, i]));
  let level: THREE.Object3D = root;
  for (let guard = 0; guard < 6; guard++) {
    const models = level.children.filter((c) => !isBrickGroup(c) && collectBricks(c).length > 0);
    if (models.length !== 1) break;
    level = models[0];
  }
  const ranges: SubRange[] = [];
  for (const c of level.children) {
    if (isBrickGroup(c)) continue;
    const own = collectBricks(c)
      .map((b) => index.get(b))
      .filter((i): i is number => i !== undefined);
    if (own.length < 3) continue;
    const start = Math.min(...own);
    const end = Math.max(...own) + 1;
    // Only a contiguous run is a submodel a helper can build on its own.
    if (end - start === own.length) ranges.push({ start, end });
  }
  if (ranges.length > 0) {
    const largest = ranges.reduce((a, b) => (b.end - b.start > a.end - a.start ? b : a));
    ranges.splice(ranges.indexOf(largest), 1);
  }
  return ranges.sort((a, b) => a.start - b.start);
}

export type BenchSet = {
  /** Holds the model at world scale, base on y = 0, centred on its turntable. */
  holder: THREE.Group;
  /** The LDraw-space model under `holder`. */
  model: THREE.Group;
  bricks: THREE.Object3D[];
  /** Each brick's instruction step. */
  stepOf: number[];
  steps: number;
  shape: SetShape;
  /** Finished height in world units. */
  height: number;
  radius: number;
};

/** Height the finished model is scaled to, and the footprint radius it must fit. */
export const MODEL_HEIGHT = 1.15;
export const MODEL_RADIUS = 0.95;

export async function benchSet(file: string): Promise<BenchSet> {
  const template = await loadTemplate(file);
  const model = template.clone(true) as THREE.Group;
  const bricks = collectBricks(model);
  const stepOf = bricks.map((b) => (b.userData.buildingStep as number | undefined) ?? 0);
  const steps = (model.userData.numBuildingSteps as number | undefined) ?? Math.max(1, ...stepOf) + 1;

  // LDraw is Y-down. Flip it upright, then measure and fit it to the bench.
  model.rotation.set(Math.PI, 0, 0);
  const holder = new THREE.Group();
  holder.add(model);
  holder.updateMatrixWorld(true);
  const box = new THREE.Box3().setFromObject(model);
  const size = box.getSize(new THREE.Vector3());
  const centre = box.getCenter(new THREE.Vector3());
  const radius = Math.max(size.x, size.z) / 2;
  const scale = Math.min(MODEL_HEIGHT / Math.max(size.y, 1e-6), MODEL_RADIUS / Math.max(radius, 1e-6));
  model.scale.setScalar(scale);
  model.position.set(-centre.x * scale, -box.min.y * scale, -centre.z * scale);

  return {
    holder,
    model,
    bricks,
    stepOf,
    steps,
    shape: { bricks: bricks.length, submodels: submodelRanges(model, bricks) },
    height: size.y * scale,
    radius: radius * scale,
  };
}

const shapes = new Map<string, Promise<SetShape>>();

/** A set's shape (brick count and claimable submodels), from its template. */
export function shapeFor(file: string): Promise<SetShape> {
  let p = shapes.get(file);
  if (!p) {
    p = loadTemplate(file).then((t) => {
      const bricks = collectBricks(t);
      return { bricks: bricks.length, submodels: submodelRanges(t, bricks) };
    });
    shapes.set(file, p);
  }
  return p;
}
