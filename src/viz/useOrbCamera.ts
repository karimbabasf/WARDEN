import { useFrame, useThree } from '@react-three/fiber';
import { useCallback, useRef } from 'react';
import * as THREE from 'three';
import type { Vec3 } from './orbTypes';

export type CameraTarget = {
  position: Vec3;
  lookAt: Vec3;
};

export function dampValue(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.lerp(current, target, 1 - Math.exp(-lambda * dt));
}

export function damp3(current: Vec3, target: Vec3, lambda: number, dt: number): Vec3 {
  return {
    x: dampValue(current.x, target.x, lambda, dt),
    y: dampValue(current.y, target.y, lambda, dt),
    z: dampValue(current.z, target.z, lambda, dt),
  };
}

export function cameraTargetForOverview(): CameraTarget {
  return {
    position: { x: 0, y: 0.4, z: 9.2 },
    lookAt: { x: 0, y: 0, z: 0 },
  };
}

export function cameraTargetForOrbitOverview(): CameraTarget {
  return {
    position: { x: 0, y: 1, z: 12.6 },
    lookAt: { x: 0, y: 0, z: 0 },
  };
}

/**
 * RADAR overview pose. The live agent forest spreads wider than a single Habits
 * cluster (multiple root planets on a ring, each with orbiting moons), so the
 * camera pulls back a touch further to frame the whole constellation.
 */
export function cameraTargetForRadarOverview(): CameraTarget {
  return {
    position: { x: 0, y: 2.6, z: 17.5 },
    lookAt: { x: 0, y: 0, z: 0 },
  };
}

/** The `<Canvas camera>` prop for the standalone radar scene. */
export type CanvasCameraProps = {
  position: [number, number, number];
  fov: number;
  near: number;
  far: number;
};

/**
 * Initial camera for the radar's standalone <Canvas> (the dev harness; in the live
 * app the radar body shares WarRoom's Canvas). The radar then FLIES via the
 * CameraRig (drei OrbitControls + damped focus-dive onto the selected globe), but
 * its opening pose is anchored here on the SAME `cameraTargetForRadarOverview`
 * pose, so "where the radar opens" has one source of truth and that overview export
 * is wired into the render path rather than left dead.
 */
export function radarCanvasCamera(): CanvasCameraProps {
  const { position } = cameraTargetForRadarOverview();
  return { position: [position.x, position.y, position.z], fov: 46, near: 0.1, far: 140 };
}

export function cameraTargetForFocus(position: Vec3, radius: number): CameraTarget {
  const distance = 2.2 + Math.max(0.8, radius) * 2.1;
  return {
    position: {
      x: position.x + distance * 0.34,
      y: position.y + distance * 0.2,
      z: position.z + distance,
    },
    lookAt: { ...position },
  };
}

export function useOrbCamera() {
  const { camera } = useThree();
  const target = useRef<CameraTarget>(cameraTargetForOverview());
  const lookAt = useRef(new THREE.Vector3(0, 0, 0));

  const reset = useCallback(() => {
    target.current = cameraTargetForOverview();
  }, []);

  const focus = useCallback((position: Vec3, radius: number) => {
    target.current = cameraTargetForFocus(position, radius);
  }, []);

  useFrame((_, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const nextPosition = damp3(
      { x: camera.position.x, y: camera.position.y, z: camera.position.z },
      target.current.position,
      4.8,
      dt,
    );
    camera.position.set(nextPosition.x, nextPosition.y, nextPosition.z);
    const nextLook = damp3(
      { x: lookAt.current.x, y: lookAt.current.y, z: lookAt.current.z },
      target.current.lookAt,
      5.2,
      dt,
    );
    lookAt.current.set(nextLook.x, nextLook.y, nextLook.z);
    camera.lookAt(lookAt.current);
  });

  return { focus, reset };
}
