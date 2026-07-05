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
 *
 * This fixed pose is now only a FALLBACK for callers/tests without live bounds — the
 * live camera frames the real fleet via `fitDistanceForBounds` + `radarHeroPose`
 * (see CameraRig), so summon lands on an auto-fit, composed ¾ shot rather than a
 * one-size constant. Kept as the deterministic baseline (and the standalone dev
 * <Canvas>'s opening pose) so "where the radar opens" still has a stable source.
 */
export function cameraTargetForRadarOverview(): CameraTarget {
  return {
    // Opens a touch wider (17.5 → 21) so a busy multi-folder fleet lands closer to
    // fully framed on summon; a lone-orchestrator swarm still reads fine, and the
    // wheel covers the rest either way (MAX_DIST raised in CameraRig).
    position: { x: 0, y: 3, z: 21 },
    lookAt: { x: 0, y: 0, z: 0 },
  };
}

// ── the composed RADAR "hero" angle ────────────────────────────────────────────
// A deliberate three-quarter shot so a summon lands on an INTENTIONAL, cinematic
// frame — never an axis-aligned "looking straight down −Z" default that reads as a
// flat wall of globes. Azimuth swings the camera off the front axis (front-right),
// elevation tilts it gently down onto the fleet. Both sit comfortably inside the
// CameraRig polar clamps (MIN_POLAR ≈ 0.16π .. MAX_POLAR ≈ 0.84π): the resulting
// polar angle is π/2 − elevation ≈ 66°, well within range.
export const RADAR_HERO_AZIMUTH = 0.62; // ≈ 35° off the +Z front axis (front-right)
export const RADAR_HERO_ELEVATION = 0.42; // ≈ 24° above the horizon, looking down a touch

/**
 * Compose a camera pose from an auto-fit result (`{ target, distance }` — see
 * `fitDistanceForBounds`) and a hero azimuth/elevation. The camera is placed on the
 * sphere of `distance` around `target` at the given angles; it looks AT `target`.
 *
 * Spherical placement (azimuth `az` about +Y from the +Z axis, elevation `el` above
 * the XZ plane): offset = distance · (cosEl·sinAz, sinEl, cosEl·cosAz). At az=el=0
 * this is +Z (the classic "looking down −Z" overview), so non-zero angles rotate the
 * shot to the composed three-quarter view. Pure + exported so the framing is
 * unit-tested without WebGL — the CameraRig just eases toward what this returns.
 */
export function radarHeroPose(
  fit: { target: [number, number, number]; distance: number },
  azimuth: number = RADAR_HERO_AZIMUTH,
  elevation: number = RADAR_HERO_ELEVATION,
): CameraTarget {
  const [tx, ty, tz] = fit.target;
  const d = Number.isFinite(fit.distance) && fit.distance > 0 ? fit.distance : 21;
  const az = Number.isFinite(azimuth) ? azimuth : RADAR_HERO_AZIMUTH;
  const el = Number.isFinite(elevation) ? elevation : RADAR_HERO_ELEVATION;
  const cosEl = Math.cos(el);
  return {
    position: {
      x: tx + d * cosEl * Math.sin(az),
      y: ty + d * Math.sin(el),
      z: tz + d * cosEl * Math.cos(az),
    },
    lookAt: { x: tx, y: ty, z: tz },
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
