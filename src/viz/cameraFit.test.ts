import { describe, expect, it } from 'vitest';
import { fitDistanceForBounds, FIT_OVERVIEW_DIST } from './cameraFit';

// fitDistanceForBounds is PURE camera-fit math (no WebGL): given the live node
// positions + the camera's vertical FOV + aspect, it returns the centroid to look
// at and the smallest camera-to-centroid distance that contains the whole bounding
// sphere in frame — accounting for the NARROWER of the vertical/horizontal FOV so
// a tall-but-narrow (portrait) window never clips the constellation on the sides.
//
// Distance law: for a bounding sphere of radius r, the distance at which the sphere
// exactly fills a symmetric frustum of half-angle a is r / sin(a). We frame against
// the smaller of the vertical (fov) and horizontal (derived from aspect) half-angles,
// then inflate the radius by `margin` (default > 1) so the fleet lands with breathing
// room, never flush to the edge.

const FOV = (46 * Math.PI) / 180; // the radar Canvas mounts a 46° vertical FOV

describe('fitDistanceForBounds', () => {
  it('empty input → sane overview default at the origin', () => {
    const fit = fitDistanceForBounds([], FOV, 16 / 9);
    expect(fit.target).toEqual([0, 0, 0]);
    expect(fit.distance).toBeCloseTo(FIT_OVERVIEW_DIST, 6);
  });

  it('a single point → look at it, sit the overview distance back (zero-radius sphere)', () => {
    const fit = fitDistanceForBounds([{ x: 2, y: -1, z: 3 }], FOV, 16 / 9);
    expect(fit.target[0]).toBeCloseTo(2, 6);
    expect(fit.target[1]).toBeCloseTo(-1, 6);
    expect(fit.target[2]).toBeCloseTo(3, 6);
    // a degenerate (radius 0) sphere can't drive a distance → fall back to overview.
    expect(fit.distance).toBeCloseTo(FIT_OVERVIEW_DIST, 6);
  });

  it('target is the centroid of the points', () => {
    const fit = fitDistanceForBounds(
      [
        { x: -4, y: 0, z: 0 },
        { x: 4, y: 0, z: 0 },
        { x: 0, y: 6, z: 0 },
        { x: 0, y: -6, z: 0 },
      ],
      FOV,
      16 / 9,
    );
    expect(fit.target[0]).toBeCloseTo(0, 6);
    expect(fit.target[1]).toBeCloseTo(0, 6);
    expect(fit.target[2]).toBeCloseTo(0, 6);
  });

  it('distance contains the bounding sphere for the narrower (vertical) axis on a wide frame', () => {
    // Two points 20 apart on X → centroid at origin, bounding radius 10. On a WIDE
    // frame (aspect > 1) the vertical FOV is the tighter constraint, so the distance
    // must satisfy the vertical half-angle: dist ≈ (r*margin)/sin(vfov/2).
    const r = 10;
    const margin = 1.15;
    const fit = fitDistanceForBounds(
      [
        { x: -10, y: 0, z: 0 },
        { x: 10, y: 0, z: 0 },
      ],
      FOV,
      16 / 9,
      margin,
    );
    const expected = (r * margin) / Math.sin(FOV / 2);
    expect(fit.distance).toBeCloseTo(expected, 4);
  });

  it('a TALL (portrait) frame is governed by the horizontal FOV → sits further back', () => {
    // Same sphere, but a portrait aspect (< 1) makes the HORIZONTAL half-angle the
    // narrower one, so the required distance is larger than the wide-frame case.
    const r = 10;
    const margin = 1;
    const wide = fitDistanceForBounds(
      [{ x: -10, y: 0, z: 0 }, { x: 10, y: 0, z: 0 }],
      FOV,
      16 / 9,
      margin,
    );
    const tall = fitDistanceForBounds(
      [{ x: -10, y: 0, z: 0 }, { x: 10, y: 0, z: 0 }],
      FOV,
      9 / 16,
      margin,
    );
    // Horizontal half-angle on the portrait frame:
    const hfovTall = 2 * Math.atan(Math.tan(FOV / 2) * (9 / 16));
    const expectedTall = (r * margin) / Math.sin(hfovTall / 2);
    expect(tall.distance).toBeCloseTo(expectedTall, 4);
    expect(tall.distance).toBeGreaterThan(wide.distance);
  });

  it('default margin leaves headroom (distance strictly exceeds the exact-fit distance)', () => {
    const pts = [{ x: -5, y: 0, z: 0 }, { x: 5, y: 0, z: 0 }];
    const exact = (5 / Math.sin(FOV / 2)); // margin 1, wide frame
    const fit = fitDistanceForBounds(pts, FOV, 16 / 9); // default margin
    expect(fit.distance).toBeGreaterThan(exact);
  });

  it('degenerate FOV / aspect never yields NaN or a non-positive distance', () => {
    const pts = [{ x: -3, y: 2, z: 1 }, { x: 3, y: -2, z: -1 }];
    for (const [fov, aspect] of [
      [0, 1],
      [FOV, 0],
      [Number.NaN, 16 / 9],
      [FOV, Number.NaN],
      [-1, -1],
    ] as const) {
      const fit = fitDistanceForBounds(pts, fov, aspect);
      expect(Number.isFinite(fit.distance)).toBe(true);
      expect(fit.distance).toBeGreaterThan(0);
    }
  });
});
