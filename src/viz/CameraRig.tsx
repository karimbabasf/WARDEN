// CameraRig.tsx — free orbit + cinematic focus.
//
// The old camera could only snap between a fixed overview and a fixed per-node
// pose (no drag at all), which is exactly why the scene "felt locked". This rig
// gives drei OrbitControls with inertia (drag to spin the constellation like a
// 3D model, scroll to dolly) AND a damped focus move: when an orb is selected we
// glide the orbit target onto it and pull the camera in, but we KEEP the user's
// current viewing angle — we only recenter + change distance, then hand control
// straight back. Clearing the selection glides back to the overview.

import { useEffect, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from './orbTypes';

const OVERVIEW_DIST = 9.4;
const MIN_DIST = 3;
const MAX_DIST = 17;

const goalTarget = new THREE.Vector3();
const offset = new THREE.Vector3();
const nextPos = new THREE.Vector3();

export function CameraRig({ selected }: { selected: LayoutNode | null }) {
  // OrbitControls instance — typed loosely to avoid importing three's controls type.
  const controls = useRef<any>(null);
  const targetGoal = useRef(new THREE.Vector3(0, 0, 0));
  const distGoal = useRef(OVERVIEW_DIST);
  const animating = useRef(false);

  useEffect(() => {
    if (selected) {
      targetGoal.current.set(selected.position.x, selected.position.y, selected.position.z);
      distGoal.current = THREE.MathUtils.clamp(2.6 + Math.max(0.6, selected.radius) * 3.4, MIN_DIST, MAX_DIST);
    } else {
      targetGoal.current.set(0, 0, 0);
      distGoal.current = OVERVIEW_DIST;
    }
    animating.current = true;
  }, [selected]);

  useFrame((state, dtRaw) => {
    const c = controls.current;
    if (!c) return;
    const dt = Math.min(dtRaw, 0.05);

    if (animating.current) {
      const k = 1 - Math.exp(-7 * dt);
      goalTarget.copy(targetGoal.current);
      c.target.lerp(goalTarget, k);

      // Preserve the current viewing direction; just recenter + change distance.
      offset.copy(state.camera.position).sub(c.target);
      const curDist = offset.length() || OVERVIEW_DIST;
      offset.normalize();
      const nextDist = THREE.MathUtils.lerp(curDist, distGoal.current, k);
      nextPos.copy(c.target).addScaledVector(offset, nextDist);
      state.camera.position.copy(nextPos);

      if (c.target.distanceToSquared(goalTarget) < 0.0004 && Math.abs(nextDist - distGoal.current) < 0.04) {
        animating.current = false;
      }
    }

    c.update();
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      enableDamping
      dampingFactor={0.085}
      rotateSpeed={0.85}
      zoomSpeed={0.95}
      enablePan={false}
      minDistance={MIN_DIST}
      maxDistance={MAX_DIST}
      // Keep the constellation upright-ish; allow looking from above/below but
      // never fully over the poles (avoids the disorienting flip).
      minPolarAngle={Math.PI * 0.16}
      maxPolarAngle={Math.PI * 0.84}
    />
  );
}

export default CameraRig;
