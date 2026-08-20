import { describe, expect, it } from 'vitest';
import { genieFrame, smootherstep } from './hudGenie';

const GEO = { width: 400, height: 300, neckLeft: 190, neckRight: 212 };

function points(clip: string): Array<[number, number]> {
  const inner = clip.slice('polygon('.length, -1);
  return inner.split(', ').map((p) => {
    const [x, y] = p.split(' ');
    return [parseFloat(x), parseFloat(y)] as [number, number];
  });
}

describe('smootherstep', () => {
  it('clamps and pins both ends', () => {
    expect(smootherstep(-3)).toBe(0);
    expect(smootherstep(0)).toBe(0);
    expect(smootherstep(1)).toBe(1);
    expect(smootherstep(9)).toBe(1);
    expect(smootherstep(0.5)).toBeCloseTo(0.5, 5);
  });
});

describe('genieFrame', () => {
  it('is the untouched panel rectangle at t = 0', () => {
    const f = genieFrame(0, GEO);
    const pts = points(f.clipPath);
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    expect(Math.min(...xs)).toBe(0);
    expect(Math.max(...xs)).toBe(400);
    expect(Math.min(...ys)).toBe(0);
    expect(Math.max(...ys)).toBe(300);
    expect(f.contentOpacity).toBe(1);
    expect(f.contentTransform).toContain('scale(1, 1)');
  });

  it('collapses onto the neck at t = 1', () => {
    const pts = points(genieFrame(1, GEO).clipPath);
    for (const [x, y] of pts) {
      expect(y).toBe(0);
      expect(x).toBeGreaterThanOrEqual(190);
      expect(x).toBeLessThanOrEqual(212);
    }
    expect(genieFrame(1, GEO).contentOpacity).toBe(0);
  });

  it('is a funnel mid-flight: the top is narrower than the bottom', () => {
    const pts = points(genieFrame(0.5, GEO).clipPath);
    // Right edge is the first half of the ring, sampled top to bottom.
    const right = pts.slice(0, pts.length / 2);
    const left = pts.slice(pts.length / 2).reverse();
    const widthAt = (i: number) => right[i][0] - left[i][0];
    expect(widthAt(0)).toBeLessThan(widthAt(right.length - 1));
    // and strictly monotonic: no row wider than the one below it
    for (let i = 1; i < right.length; i++) {
      expect(widthAt(i)).toBeGreaterThanOrEqual(widthAt(i - 1) - 0.01);
    }
  });

  it('shrinks the shape upward as it collapses', () => {
    const bottom = (t: number) => Math.max(...points(genieFrame(t, GEO).clipPath).map((p) => p[1]));
    expect(bottom(0)).toBe(300);
    expect(bottom(0.5)).toBeCloseTo(150, 0);
    expect(bottom(1)).toBe(0);
  });

  it('squeezes the content harder across than down, so it converges on the neck', () => {
    const t = genieFrame(0.5, GEO).contentTransform;
    const [sx, sy] = t.slice(t.indexOf('scale(') + 6, t.lastIndexOf(')')).split(', ').map(Number);
    expect(sx).toBeLessThan(sy);
    expect(sx).toBeGreaterThan(0);
  });

  it('holds the content opaque through the first half', () => {
    expect(genieFrame(0.3, GEO).contentOpacity).toBe(1);
    expect(genieFrame(0.62, GEO).contentOpacity).toBe(1);
    expect(genieFrame(0.85, GEO).contentOpacity).toBeLessThan(1);
  });

  it('leans the funnel when the icon sits outside the panel', () => {
    const off = { ...GEO, neckLeft: 470, neckRight: 492 };
    const pts = points(genieFrame(1, off).clipPath);
    for (const [x] of pts) expect(x).toBeGreaterThanOrEqual(470);
  });

  it('clamps a progress outside [0, 1]', () => {
    expect(genieFrame(-2, GEO).clipPath).toBe(genieFrame(0, GEO).clipPath);
    expect(genieFrame(4, GEO).clipPath).toBe(genieFrame(1, GEO).clipPath);
  });

  it('anchors the content origin at the neck centre', () => {
    expect(genieFrame(0.5, GEO).contentOrigin).toBe('201px 0px');
  });
});
