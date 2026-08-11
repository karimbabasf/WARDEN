// The dive that arrived twice.
//
// `selected` is a node out of a layout the lead rebuilds on every `radar_state`
// emit, so the rig is handed a fresh object about once a second for a globe that has
// not moved. The selection effect re-ran on each one, and because it also clears
// `flyActive`, an emit landing mid-dive killed the focus fly and left the damped
// branch crawling toward the SELECT pose instead. The two poses sit at different
// distances, so the camera arrived, stopped, and then backed out again.
//
// Measured on a real dive before the guard: the camera reached z=5.023, the goal
// flipped from z=5.000 to z=5.615 the frame the fly ended, and it crawled outward
// for another 430ms (settle 1128ms). After: it converges on z=5 and holds (775ms).
//
// So the property under test is not "the key is stable", it is "a re-emitted board
// is not a new selection".

import { describe, expect, it } from 'vitest';
import { selectKey } from './CameraRig';

const node = (over: Partial<{ id: string; x: number; y: number; z: number; radius: number }> = {}) => {
  const { id = 'a1', x = 1.437, y = -6.5, z = 0, radius = 0.82 } = over;
  return { id, position: { x, y, z }, radius };
};

describe('selectKey', () => {
  it('is the same for two objects describing the same globe in the same place', () => {
    // Exactly what a fresh emit produces: a new object, identical values.
    expect(selectKey(node(), 334, 334)).toBe(selectKey(node(), 334, 334));
  });

  it('survives the float noise a re-layout can introduce', () => {
    // The layout is deterministic, but it is recomputed from re-parsed numbers, so
    // the guard must not be defeated by the last decimal place.
    expect(selectKey(node({ x: 1.4370001 }), 334, 334)).toBe(selectKey(node({ x: 1.437 }), 334, 334));
  });

  it('changes when a different globe is selected', () => {
    expect(selectKey(node({ id: 'a2' }), 334, 334)).not.toBe(selectKey(node(), 334, 334));
  });

  it('changes when the selected globe genuinely moves', () => {
    // A sibling finishing closes ranks and re-centres its parent, and the camera does
    // have to follow that. The guard must not swallow a real move.
    expect(selectKey(node({ x: 2.6 }), 334, 334)).not.toBe(selectKey(node(), 334, 334));
    expect(selectKey(node({ radius: 1.4 }), 334, 334)).not.toBe(selectKey(node(), 334, 334));
  });

  it('changes when the free channel changes', () => {
    // Selecting is what OPENS the right rail, so its width lands a commit later and
    // the pose has to be recomputed against the chrome that is now on screen.
    expect(selectKey(node(), 334, 0)).not.toBe(selectKey(node(), 334, 334));
    expect(selectKey(node(), 104, 334)).not.toBe(selectKey(node(), 334, 334));
  });

  it('distinguishes no selection from a selection, and tracks insets while empty', () => {
    expect(selectKey(null, 334, 0)).not.toBe(selectKey(node(), 334, 0));
    expect(selectKey(null, 334, 0)).toBe(selectKey(null, 334, 0));
    // Folding the rack with nothing selected still has to re-frame the board.
    expect(selectKey(null, 104, 0)).not.toBe(selectKey(null, 334, 0));
  });
});
