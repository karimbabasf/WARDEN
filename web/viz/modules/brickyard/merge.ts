// merge.ts: folds many finished bricks into a few draw calls.
//
// A placed brick is two draw calls (faces and edges), so a bench of 300 bricks was 600
// calls before the shadow pass, and the yard lagged. Once a run of bricks is placed
// and still, it is baked into one mesh and one line set: opaque colours go into vertex
// colours under one shared material, and only see-through or metallic parts keep a
// material of their own. Picking still works: every merged mesh keeps a map from
// vertex ranges back to the brick that owns them.

import * as THREE from 'three';

/** One shared plastic for every baked opaque brick. */
const PLASTIC = new THREE.MeshStandardMaterial({ vertexColors: true, roughness: 0.32, metalness: 0 });
const EDGES = new THREE.LineBasicMaterial({ vertexColors: true });

type Bucket = { pos: number[]; nor: number[]; col: number[]; owners: { end: number; brick: number }[] };

function bucket(map: Map<string, Bucket>, key: string): Bucket {
  let b = map.get(key);
  if (!b) {
    b = { pos: [], nor: [], col: [], owners: [] };
    map.set(key, b);
  }
  return b;
}

const v = new THREE.Vector3();
const n = new THREE.Vector3();
const nm = new THREE.Matrix3();
const rel = new THREE.Matrix4();
const inv = new THREE.Matrix4();

/** Whether a material can be baked into vertex colours under the shared plastic. */
function bakeable(m: THREE.Material): boolean {
  const s = m as THREE.MeshStandardMaterial;
  return !s.transparent && (s.metalness ?? 0) < 0.4 && !!s.color;
}

/**
 * Merge `bricks` (with their `index` in the set) into one group expressed in `root`'s
 * space. The bricks must sit at their home transforms. Returns the group; the caller
 * hides the originals.
 */
export function mergeBricks(bricks: { obj: THREE.Object3D; index: number }[], root: THREE.Object3D): THREE.Group {
  root.updateMatrixWorld(true);
  inv.copy(root.matrixWorld).invert();
  const faces = new Map<string, Bucket>();
  const lines = new Map<string, Bucket>();
  const keepMat = new Map<string, THREE.Material>();

  for (const { obj, index } of bricks) {
    obj.traverse((c) => {
      const mesh = c as THREE.Mesh;
      const isMesh = mesh.isMesh;
      const isLine = (c as THREE.LineSegments).isLineSegments && !(c as unknown as { isConditionalLine?: boolean }).isConditionalLine;
      if (!isMesh && !isLine) return;
      const geo = mesh.geometry as THREE.BufferGeometry;
      const p = geo.getAttribute('position') as THREE.BufferAttribute | undefined;
      if (!p) return;
      const nrm = isMesh ? (geo.getAttribute('normal') as THREE.BufferAttribute | undefined) : undefined;
      rel.multiplyMatrices(inv, c.matrixWorld);
      nm.getNormalMatrix(rel);
      const flip = isMesh && rel.determinant() < 0;
      const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
      const groups = geo.groups.length > 0 ? geo.groups : [{ start: 0, count: p.count, materialIndex: 0 }];
      for (const g of groups) {
        const mat = mats[g.materialIndex ?? 0] ?? mats[0];
        if (!mat) continue;
        const bake = bakeable(mat);
        const key = bake ? (isMesh ? 'plastic' : 'edges') : mat.uuid;
        if (!bake) keepMat.set(key, mat);
        const b = bucket(isMesh ? faces : lines, key);
        const color = (mat as THREE.MeshStandardMaterial).color ?? new THREE.Color(0xffffff);
        const end = Math.min(g.start + g.count, p.count);
        const stride = isMesh ? 3 : 2;
        for (let i = g.start; i + stride <= end; i += stride) {
          // A mirrored part flips its winding; swap two vertices to keep faces outward.
          const order = flip ? [1, 0, 2] : stride === 3 ? [0, 1, 2] : [0, 1];
          for (const k of order) {
            const vi = i + k;
            v.fromBufferAttribute(p, vi).applyMatrix4(rel);
            b.pos.push(v.x, v.y, v.z);
            if (nrm) {
              n.fromBufferAttribute(nrm, vi).applyMatrix3(nm).normalize();
              b.nor.push(n.x, n.y, n.z);
            }
            b.col.push(color.r, color.g, color.b);
          }
        }
        b.owners.push({ end: b.pos.length / 3, brick: index });
      }
    });
  }

  const out = new THREE.Group();
  out.name = 'brickyard-merged';
  const build = (map: Map<string, Bucket>, isMesh: boolean) => {
    for (const [key, b] of map) {
      if (b.pos.length === 0) continue;
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.Float32BufferAttribute(b.pos, 3));
      if (isMesh && b.nor.length === b.pos.length) g.setAttribute('normal', new THREE.Float32BufferAttribute(b.nor, 3));
      const shared = key === 'plastic' || key === 'edges';
      if (shared) g.setAttribute('color', new THREE.Float32BufferAttribute(b.col, 3));
      const mat = shared ? (isMesh ? PLASTIC : EDGES) : (keepMat.get(key) as THREE.Material);
      const obj = isMesh ? new THREE.Mesh(g, mat) : new THREE.LineSegments(g, mat);
      obj.userData.owners = b.owners;
      if (isMesh) {
        obj.receiveShadow = true;
      } else {
        obj.raycast = () => {};
      }
      out.add(obj);
    }
  };
  build(faces, true);
  build(lines, false);
  return out;
}

/** The brick that owns a picked face of a merged mesh, or null. */
export function ownerOfFace(obj: THREE.Object3D, faceIndex: number | undefined): number | null {
  const owners = obj.userData?.owners as { end: number; brick: number }[] | undefined;
  if (!owners || faceIndex === undefined) return null;
  const vtx = faceIndex * 3;
  let lo = 0;
  let hi = owners.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (owners[mid].end > vtx) hi = mid;
    else lo = mid + 1;
  }
  return owners[lo]?.brick ?? null;
}

export function disposeMerged(g: THREE.Object3D) {
  g.traverse((c) => {
    const m = c as THREE.Mesh;
    if (m.geometry) m.geometry.dispose();
  });
  g.removeFromParent();
}
