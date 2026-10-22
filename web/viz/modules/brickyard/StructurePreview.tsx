// StructurePreview.tsx: the structure on its own, in the panel. What it looks like now
// (built bricks solid, the rest as a ghost) or what it will look like finished, on a
// slow turntable you can drag. Baked into a few draw calls, rebuilt only when the
// brick count changes.

import { useEffect, useMemo, useState } from 'react';
import { Canvas } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import { collectBricks, loadTemplate } from './setModel';
import { disposeMerged, mergeBricks } from './merge';

const GHOST = new THREE.MeshBasicMaterial({ color: 0x9fc4ff, transparent: true, opacity: 0.12, depthWrite: false });
const GHOST_EDGE = new THREE.LineBasicMaterial({ color: 0x9fc4ff, transparent: true, opacity: 0.18 });

function PreviewModel({ file, placed, finished }: { file: string; placed: boolean[]; finished: boolean }) {
  const [group, setGroup] = useState<THREE.Group | null>(null);
  const key = finished ? 'all' : placed.map((p) => (p ? 1 : 0)).join('');
  useEffect(() => {
    let alive = true;
    let made: THREE.Group | null = null;
    void loadTemplate(file).then((t) => {
      if (!alive) return;
      const model = t.clone(true) as THREE.Group;
      model.rotation.set(Math.PI, 0, 0);
      model.updateMatrixWorld(true);
      const bricks = collectBricks(model);
      const solid = bricks.map((obj, index) => ({ obj, index })).filter(({ index }) => finished || placed[index]);
      const ghost = bricks.map((obj, index) => ({ obj, index })).filter(({ index }) => !finished && !placed[index]);
      const root = new THREE.Group();
      const holder = new THREE.Group();
      root.add(holder);
      holder.rotation.copy(model.rotation);
      if (solid.length) holder.add(mergeBricks(solid, model));
      if (ghost.length) {
        const g = mergeBricks(ghost, model);
        g.traverse((o) => {
          const m = o as THREE.Mesh;
          if (m.isMesh) m.material = GHOST;
          else if ((o as THREE.LineSegments).isLineSegments) (o as THREE.LineSegments).material = GHOST_EDGE;
        });
        holder.add(g);
      }
      // Fit the whole set (built or not) to a unit sphere, standing on y = 0.
      root.updateMatrixWorld(true);
      const box = new THREE.Box3().setFromObject(model);
      const size = box.getSize(new THREE.Vector3());
      const centre = box.getCenter(new THREE.Vector3());
      const scale = 2 / Math.max(size.x, size.y, size.z, 1e-6);
      root.scale.setScalar(scale);
      root.position.set(-centre.x * scale, -box.min.y * scale, -centre.z * scale);
      made = root;
      setGroup(root);
    });
    return () => {
      alive = false;
      if (made) disposeMerged(made);
    };
    // `key` carries `placed` and `finished`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file, key]);
  return group ? <primitive object={group} /> : null;
}

export function StructurePreview({ file, placed, finishedOnly = false }: { file: string; placed: boolean[]; finishedOnly?: boolean }) {
  const [pickFinished, setFinished] = useState(false);
  const finished = finishedOnly || pickFinished;
  const built = useMemo(() => placed.filter(Boolean).length, [placed]);
  return (
    <section className={`by-preview${finishedOnly ? ' is-shelf' : ''}`} aria-label="Structure preview">
      <div className="by-preview-stage">
        <Canvas dpr={[1, 1.5]} camera={{ position: [2.6, 2.1, 2.9], fov: 34 }} gl={{ antialias: true, alpha: true }}>
          <hemisphereLight args={[0xffffff, 0x445066, 1.5]} />
          <directionalLight position={[3, 5, 4]} intensity={2} />
          <directionalLight position={[-4, 2, -2]} intensity={0.5} color="#cfe0ff" />
          <PreviewModel file={file} placed={placed} finished={finished} />
          <OrbitControls autoRotate autoRotateSpeed={1.4} enablePan={false} enableDamping target={[0, 0.7, 0]} minDistance={1.8} maxDistance={7} />
        </Canvas>
      </div>
      {finishedOnly ? null : (
      <div className="by-preview-bar" role="group" aria-label="Preview mode">
        <button type="button" className="by-seg" aria-pressed={!finished} onClick={() => setFinished(false)}>
          Now <span>{built}/{placed.length}</span>
        </button>
        <button type="button" className="by-seg" aria-pressed={finished} onClick={() => setFinished(true)}>
          Finished
        </button>
      </div>
      )}
    </section>
  );
}
