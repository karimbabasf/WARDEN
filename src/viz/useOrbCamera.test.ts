import { describe, expect, it } from 'vitest';
import {
  cameraTargetForFocus,
  cameraTargetForOrbitOverview,
  cameraTargetForOverview,
  cameraTargetForRadarOverview,
  radarCanvasCamera,
  radarHeroPose,
  RADAR_HERO_AZIMUTH,
  RADAR_HERO_ELEVATION,
  damp3,
} from './useOrbCamera';

describe('useOrbCamera helpers', () => {
  it('returns a stable overview target', () => {
    expect(cameraTargetForOverview()).toEqual({
      position: { x: 0, y: 0.4, z: 9.2 },
      lookAt: { x: 0, y: 0, z: 0 },
    });
  });

  it('returns the canonical OrbitControls overview pose used by double-click home', () => {
    expect(cameraTargetForOrbitOverview()).toEqual({
      position: { x: 0, y: 1, z: 12.6 },
      lookAt: { x: 0, y: 0, z: 0 },
    });
  });

  it('dives toward a selected node while looking at the node', () => {
    const focus = cameraTargetForFocus({ x: 2, y: 0.5, z: -1 }, 1.2);
    expect(focus.lookAt).toEqual({ x: 2, y: 0.5, z: -1 });
    expect(focus.position.z).toBeGreaterThan(focus.lookAt.z);
    expect(focus.position.x).toBeGreaterThan(2);
  });

  it('pulls the camera back for the radar overview (further than the habits overview)', () => {
    const radar = cameraTargetForRadarOverview();
    expect(radar.lookAt).toEqual({ x: 0, y: 0, z: 0 });
    expect(radar.position.z).toBeGreaterThan(0);
    // the radar forest spreads wider than a single habits cluster → pull back more
    expect(radar.position.z).toBeGreaterThanOrEqual(cameraTargetForOverview().position.z);
  });

  it('derives the radar standalone-canvas camera prop from the radar overview pose', () => {
    // The radar scene flies via the CameraRig (OrbitControls), but its standalone
    // <Canvas> still needs a sane INITIAL pose. We anchor that on the same
    // `cameraTargetForRadarOverview` pose so there is a single source of truth for
    // "where the radar opens" — wiring the overview export into the render path
    // instead of leaving it dead. The derived prop must sit at the overview z.
    const overview = cameraTargetForRadarOverview();
    const cam = radarCanvasCamera();
    expect(cam.position).toEqual([overview.position.x, overview.position.y, overview.position.z]);
    expect(cam.fov).toBeGreaterThan(0);
    expect(cam.far).toBeGreaterThan(cam.near);
  });

  it('composes a hero pose that looks at the fit target from `distance` away, off-axis', () => {
    const fit = { target: [1, 2, -3] as [number, number, number], distance: 20 };
    const pose = radarHeroPose(fit);
    // looks AT the centroid
    expect(pose.lookAt).toEqual({ x: 1, y: 2, z: -3 });
    // sits exactly `distance` from the target (spherical placement preserves radius)
    const dx = pose.position.x - 1;
    const dy = pose.position.y - 2;
    const dz = pose.position.z + 3;
    expect(Math.sqrt(dx * dx + dy * dy + dz * dz)).toBeCloseTo(20, 4);
    // three-quarter shot: OFF the front axis (x offset non-trivial) and ABOVE (y up)
    expect(Math.abs(dx)).toBeGreaterThan(0.5);
    expect(dy).toBeGreaterThan(0);
    // elevation matches the configured hero angle: y-offset = distance·sin(elevation)
    expect(dy).toBeCloseTo(20 * Math.sin(RADAR_HERO_ELEVATION), 4);
  });

  it('hero pose falls back to a sane distance when the fit distance is degenerate', () => {
    const pose = radarHeroPose({ target: [0, 0, 0], distance: 0 });
    const r = Math.hypot(pose.position.x, pose.position.y, pose.position.z);
    expect(r).toBeGreaterThan(0);
    expect(Number.isFinite(r)).toBe(true);
  });

  it('exposes hero angles that stay within the CameraRig polar clamps', () => {
    // The rig clamps polar (from +Y) to [0.16π, 0.84π]. Hero polar = π/2 − elevation.
    const polar = Math.PI / 2 - RADAR_HERO_ELEVATION;
    expect(polar).toBeGreaterThan(Math.PI * 0.16);
    expect(polar).toBeLessThan(Math.PI * 0.84);
    expect(RADAR_HERO_AZIMUTH).toBeGreaterThan(0);
  });

  it('damps vector components without overshooting', () => {
    const next = damp3({ x: 0, y: 0, z: 0 }, { x: 10, y: 5, z: -5 }, 8, 1 / 60);
    expect(next.x).toBeGreaterThan(0);
    expect(next.x).toBeLessThan(10);
    expect(next.z).toBeLessThan(0);
    expect(next.z).toBeGreaterThan(-5);
  });
});
