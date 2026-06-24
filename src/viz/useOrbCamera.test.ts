import { describe, expect, it } from 'vitest';
import {
  cameraTargetForFocus,
  cameraTargetForOverview,
  cameraTargetForRadarOverview,
  radarCanvasCamera,
  damp3,
} from './useOrbCamera';

describe('useOrbCamera helpers', () => {
  it('returns a stable overview target', () => {
    expect(cameraTargetForOverview()).toEqual({
      position: { x: 0, y: 0.4, z: 9.2 },
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

  it('damps vector components without overshooting', () => {
    const next = damp3({ x: 0, y: 0, z: 0 }, { x: 10, y: 5, z: -5 }, 8, 1 / 60);
    expect(next.x).toBeGreaterThan(0);
    expect(next.x).toBeLessThan(10);
    expect(next.z).toBeLessThan(0);
    expect(next.z).toBeGreaterThan(-5);
  });
});

