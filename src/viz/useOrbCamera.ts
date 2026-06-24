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
