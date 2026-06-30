import { useCallback, useRef, useState } from 'react';
import { useThree } from '@react-three/fiber';
import * as THREE from 'three';
import type { Vec3 } from '@/viz/shared/types/orbTypes';

export type DragApi = {
  /** Start dragging node `id` from its current world position (call on pointer-down). */
  begin: (id: string, startWorld: Vec3) => void;
  /** Live drag target; the dragged node's frame loop follows this instead of layout. */
  dragRef: React.MutableRefObject<{ id: string | null; pos: THREE.Vector3 }>;
  /** True through the click that fires on release, so the orb can skip select. */
  movedRef: React.MutableRefObject<boolean>;
  draggingId: string | null;
};

/**
 * Screen-plane node dragging. Pointer-down disables the camera controls and
 * projects the pointer onto a camera-facing plane through the node; each move
 * writes the live world position into `dragRef`. Release commits id -> [x,y,z]
 * (layout re-runs; links/labels/camera reconcile) and re-enables controls.
 */
export function useNodeDrag(
  onCommit: (id: string, pos: [number, number, number]) => void,
): DragApi {
  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const controls = useThree((s) => s.controls) as { enabled: boolean } | null;
  const dragRef = useRef<{ id: string | null; pos: THREE.Vector3 }>({
    id: null,
    pos: new THREE.Vector3(),
  });
  const movedRef = useRef(false);
  const plane = useRef(new THREE.Plane());
  const ndc = useRef(new THREE.Vector2());
  const ray = useRef(new THREE.Raycaster());
  const hit = useRef(new THREE.Vector3());
  const normal = useRef(new THREE.Vector3());
  const [draggingId, setDraggingId] = useState<string | null>(null);

  const onMove = useCallback(
    (e: PointerEvent) => {
      if (!dragRef.current.id) return;
      const rect = gl.domElement.getBoundingClientRect();
      ndc.current.set(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1,
      );
      ray.current.setFromCamera(ndc.current, camera);
      if (ray.current.ray.intersectPlane(plane.current, hit.current)) {
        dragRef.current.pos.copy(hit.current);
        movedRef.current = true;
      }
    },
    [camera, gl],
  );

  const onUp = useCallback(() => {
    const id = dragRef.current.id;
    if (id && movedRef.current) {
      onCommit(id, [dragRef.current.pos.x, dragRef.current.pos.y, dragRef.current.pos.z]);
    }
    dragRef.current.id = null;
    setDraggingId(null);
    if (controls) controls.enabled = true;
    document.body.style.cursor = '';
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', onUp);
    setTimeout(() => {
      movedRef.current = false;
    }, 0);
  }, [controls, onCommit, onMove]);

  const begin = useCallback(
    (id: string, startWorld: Vec3) => {
      dragRef.current.id = id;
      dragRef.current.pos.set(startWorld.x, startWorld.y, startWorld.z);
      movedRef.current = false;
      camera.getWorldDirection(normal.current);
      plane.current.setFromNormalAndCoplanarPoint(normal.current.negate(), dragRef.current.pos);
      if (controls) controls.enabled = false;
      document.body.style.cursor = 'grabbing';
      setDraggingId(id);
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
    },
    [camera, controls, onMove, onUp],
  );

  return { begin, dragRef, movedRef, draggingId };
}
