// The property the camera rebuild rests on: a re-target does not restart the move.
//
// The old rig eased on a fixed clock, so re-aiming mid-flight reset t to 0 and the
// camera came to a dead stop before setting off again. One click produced three of
// those. These tests state the replacement property directly (velocity survives a
// re-target) rather than testing that some easing curve has the right shape.

import { describe, expect, it } from 'vitest';
import {
  POSE_RESPONSE,
  normalize3,
  poseOf,
  posePosition,
  poseSettled,
  poseSpring,
  poseSpringStep,
  type Pose,
} from './cameraPose';

const DT = 1 / 60;

const pose = (over: Partial<Pose> = {}): Pose => ({
  target: [0, 0, 0],
  dir: [0, 0, 1],
  distance: 12.6,
  ...over,
});

/** Run `frames` steps toward `goal`, returning the spring. */
function run(start: Pose, goal: Pose, frames: number, response = POSE_RESPONSE) {
  let s = poseSpring(start);
  for (let i = 0; i < frames; i++) s = poseSpringStep(s, goal, DT, response);
  return s;
}

describe('normalize3', () => {
  it('returns a unit vector', () => {
    const [x, y, z] = normalize3([3, 0, 4]);
    expect(Math.hypot(x, y, z)).toBeCloseTo(1, 12);
  });

  it('falls back rather than dividing by zero', () => {
    // Not hypothetical: the direction is read as (camera - pivot), and those coincide
    // for one frame whenever a pose is written before the controls mount.
    expect(normalize3([0, 0, 0])).toEqual([0, 0, 1]);
    expect(normalize3([NaN, 1, 0])).toEqual([0, 0, 1]);
    expect(normalize3([0, 0, 0], [1, 0, 0])).toEqual([1, 0, 0]);
  });
});

describe('poseSpringStep', () => {
  it('converges on the goal and holds there', () => {
    const goal = pose({ target: [2, -6, 0], distance: 5 });
    const s = run(pose(), goal, 120);
    const at = poseOf(s);
    expect(at.target[0]).toBeCloseTo(2, 2);
    expect(at.target[1]).toBeCloseTo(-6, 2);
    expect(at.distance).toBeCloseTo(5, 2);
    expect(poseSettled(s, goal)).toBe(true);
  });

  it('never overshoots at the default damping', () => {
    // A camera that overshoots its subject reads as a mistake. Critically damped is a
    // deliberate choice, not a default nobody looked at.
    const goal = pose({ distance: 5 });
    let s = poseSpring(pose({ distance: 12.6 }));
    for (let i = 0; i < 200; i++) {
      s = poseSpringStep(s, goal, DT);
      expect(poseOf(s).distance).toBeGreaterThanOrEqual(5 - 1e-6);
    }
  });

  it('CARRIES VELOCITY THROUGH A RE-TARGET', () => {
    // The regression, stated as a property. Re-aim a move that is already in flight
    // and the camera must keep moving; the old clock-based rig had it at a standstill
    // for the frame after the re-target and then re-accelerated from zero.
    const first = pose({ target: [4, 0, 0], distance: 5 });
    let s = run(pose(), first, 12); // mid-flight
    const speedBefore = Math.abs(s.tx.velocity);
    expect(speedBefore).toBeGreaterThan(0.5);

    const second = pose({ target: [6, 0, 0], distance: 5 });
    s = poseSpringStep(s, second, DT);
    // Still moving, and moving the same way. A restart would have zeroed this.
    expect(s.tx.velocity).toBeGreaterThan(speedBefore * 0.8);
  });

  it('re-targeting every single frame still arrives', () => {
    // What a live board does: `subtreeBounds` is recomputed on every emit and the
    // insets settle a frame or two after a rail opens. Under the old rig each of those
    // restarted the ease and the camera crawled; here they are absorbed.
    const goal = pose({ target: [2, -6, 0], distance: 5 });
    let s = poseSpring(pose());
    for (let i = 0; i < 120; i++) {
      // Jitter the goal by float noise the way a re-layout does.
      const noisy = pose({
        target: [2 + Math.sin(i) * 1e-9, -6, 0],
        distance: 5 + Math.cos(i) * 1e-9,
      });
      s = poseSpringStep(s, noisy, DT);
    }
    expect(poseSettled(s, goal)).toBe(true);
  });

  it('takes the near side of the sphere when the direction flips', () => {
    // A goal direction near the antipode of the current one would otherwise drag every
    // component through zero at once and whip the camera through the pivot.
    const goal = pose({ dir: [0, 0, -1] });
    let s = poseSpring(pose({ dir: [0, 0, 1] }));
    for (let i = 0; i < 90; i++) {
      s = poseSpringStep(s, goal, DT);
      const d = poseOf(s).dir;
      expect(Math.hypot(d[0], d[1], d[2])).toBeCloseTo(1, 6);
    }
  });

  it('settles from one enormous frame instead of exploding', () => {
    // A backgrounded webview hands back one huge dt. Implicit Euler is unconditionally
    // stable, and the rig relies on that rather than on frames arriving on time.
    const goal = pose({ target: [2, -6, 0], distance: 5 });
    const s = poseSpringStep(poseSpring(pose()), goal, 4.0);
    expect(Number.isFinite(poseOf(s).distance)).toBe(true);
    expect(poseOf(s).distance).toBeGreaterThan(0);
  });

  it('log-space dolly keeps the apparent zoom rate uniform', () => {
    // Overview to dive is a 2.5x ratio. In linear distance the back half of that
    // crawls; in log space equal times cover equal RATIOS, which is what reads as
    // smooth. Halfway through the settle the distance should be near the geometric
    // mean, not the arithmetic one.
    const goal = pose({ distance: 5 });
    const s = run(pose({ distance: 12.6 }), goal, Math.round(60 * POSE_RESPONSE * 0.5));
    const geometric = Math.sqrt(12.6 * 5); // 7.94
    const arithmetic = (12.6 + 5) / 2; // 8.8
    const at = poseOf(s).distance;
    expect(Math.abs(at - geometric)).toBeLessThan(Math.abs(at - arithmetic));
  });
});

describe('posePosition', () => {
  it('places the camera along the direction at the stated distance', () => {
    const p = posePosition(pose({ target: [1, 2, 3], dir: [0, 0, 1], distance: 5 }));
    expect(p).toEqual([1, 2, 8]);
  });

  it('normalizes a direction it was handed unnormalized', () => {
    const p = posePosition(pose({ target: [0, 0, 0], dir: [0, 0, 4], distance: 5 }));
    expect(p[2]).toBeCloseTo(5, 12);
  });
});
