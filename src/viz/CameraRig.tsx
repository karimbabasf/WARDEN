// CameraRig.tsx — free orbit + cinematic focus.
//
// The old camera could only snap between a fixed overview and a fixed per-node
// pose (no drag at all), which is exactly why the scene "felt locked". This rig
// gives drei OrbitControls with inertia (drag to spin the constellation like a
// 3D model, scroll to dolly) AND a damped focus move: when an orb is selected we
// glide the orbit target onto it and pull the camera in, KEEPING the user's
// current viewing angle (we only recenter + change distance). Crucially, we also
// REMEMBER the exact pose the user dove FROM, and restore it verbatim when they
// back out — so zooming out never leaves the camera tilted at a strange angle.

import { useEffect, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import type { LayoutNode } from './orbTypes';

// Pulled back from the old 9.4 so the (now more widely spaced) constellation
// opens with room to breathe instead of filling the frame.
const OVERVIEW_DIST = 12.6;
const MIN_DIST = 3;
const MAX_DIST = 24;

const dir = new THREE.Vector3();

export function CameraRig({ selected }: { selected: LayoutNode | null }) {
  // OrbitControls instance — typed loosely to avoid importing three's controls type.
  const controls = useRef<any>(null);
  // The pose we're animating toward — both the orbit target AND the camera position,
  // lerped together so focus-in and back-out are one consistent motion.
  const targetGoal = useRef(new THREE.Vector3(0, 0, 0));
  const posGoal = useRef(new THREE.Vector3(0, 1, OVERVIEW_DIST));
  const animating = useRef(false);
  const wasSelected = useRef(false);
  // The exact pose (camera position + orbit target) the user was viewing from BEFORE
  // diving into an orb. Captured on the overview→focus edge and restored on back-out.
  const homeTarget = useRef(new THREE.Vector3(0, 0, 0));
  const homePos = useRef<THREE.Vector3 | null>(null);

  useEffect(() => {
    const c = controls.current;
    if (selected) {
      // Capture the dive-from pose ONCE, on the null→selected edge, so a back-out
      // returns exactly here (orb→orb jumps keep the original home).
      if (!wasSelected.current && c) {
        homeTarget.current.copy(c.target);
        homePos.current = (homePos.current ?? new THREE.Vector3()).copy(c.object.position);
      }
      wasSelected.current = true;

      targetGoal.current.set(selected.position.x, selected.position.y, selected.position.z);
      const dist = THREE.MathUtils.clamp(2.6 + Math.max(0.6, selected.radius) * 3.4, MIN_DIST, MAX_DIST);
      // Glide in along the CURRENT viewing direction (preserve the user's angle):
      // recenter on the orb, sit `dist` back along the existing view ray.
      if (c) {
        dir.copy(c.object.position).sub(c.target);
        if (dir.lengthSq() < 1e-6) dir.set(0, 0, 1);
        dir.normalize();
      } else {
        dir.set(0, 0, 1);
      }
      posGoal.current.copy(targetGoal.current).addScaledVector(dir, dist);
    } else {
      wasSelected.current = false;
      // Restore the captured dive-from pose verbatim so backing out returns to the
      // exact angle + zoom the user left — never thrown off. Fallback to a canonical
      // overview only if nothing was ever captured (shouldn't happen in practice).
      if (homePos.current) {
        targetGoal.current.copy(homeTarget.current);
        posGoal.current.copy(homePos.current);
      } else {
        targetGoal.current.set(0, 0, 0);
        posGoal.current.set(0, 1, OVERVIEW_DIST);
      }
    }
    animating.current = true;
  }, [selected]);

  useFrame((_, dtRaw) => {
    const c = controls.current;
    if (!c) return;
    const dt = Math.min(dtRaw, 0.05);

    if (animating.current) {
      const k = 1 - Math.exp(-7 * dt);
      c.target.lerp(targetGoal.current, k);
      c.object.position.lerp(posGoal.current, k);

      if (
        c.target.distanceToSquared(targetGoal.current) < 0.0004 &&
        c.object.position.distanceToSquared(posGoal.current) < 0.0009
      ) {
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
      // Dolly toward whatever the cursor is over (and back out from it) instead of
      // always toward the orbit centre — the scroll zooms into the region you're
      // pointing at. OrbitControls keeps that world point pinned under the mouse.
      zoomToCursor
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
