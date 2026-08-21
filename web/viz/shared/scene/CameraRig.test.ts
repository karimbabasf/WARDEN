// The two keys that decide when the camera is allowed to re-aim.
//
// HISTORY. `selectKey` was written to stop a re-emitted board from reading as a new
// selection: `selected` is a node out of a layout the lead rebuilds on every
// `radar_state` emit, so the rig is handed a fresh object about once a second for a
// globe that has not moved.
//
// It used to carry the RAIL INSETS as well, because the pose was computed once per
// commit and the right rail opens one commit after the selection that opened it: the
// first pose was against chrome that was not on screen yet. That is gone. The rig now
// resolves the pose every frame against whatever the insets currently are, so a rail
// arriving bends the move in flight instead of restarting it, and the key has no
// business knowing about chrome.
//
// `focusKey` is the same discipline applied to `focusBounds`, which never had it: the
// old key interpolated raw floats, `subtreeBounds` came back differing in the twelfth
// decimal on every emit, and the camera re-flew a 700ms ease once a second for as long
// as you stayed dived in.

import { describe, expect, it } from 'vitest';
import { focusKey, selectKey } from './CameraRig';

const node = (over: Partial<{ id: string; x: number; y: number; z: number; radius: number }> = {}) => {
  const { id = 'a1', x = 1.437, y = -6.5, z = 0, radius = 0.82 } = over;
  return { id, position: { x, y, z }, radius };
};

describe('selectKey', () => {
  it('is the same for two objects describing the same globe in the same place', () => {
    // Exactly what a fresh emit produces: a new object, identical values.
    expect(selectKey(node())).toBe(selectKey(node()));
  });

  it('survives the float noise a re-layout can introduce', () => {
    // The layout is deterministic, but it is recomputed from re-parsed numbers, so
    // the guard must not be defeated by the last decimal place.
    expect(selectKey(node({ x: 1.4370001 }))).toBe(selectKey(node({ x: 1.437 })));
  });

  it('changes when a different globe is selected', () => {
    expect(selectKey(node({ id: 'a2' }))).not.toBe(selectKey(node()));
  });

  it('changes when the selected globe genuinely moves', () => {
    // A sibling finishing closes ranks and re-centres its parent, and the camera does
    // have to follow that. The guard must not swallow a real move.
    expect(selectKey(node({ x: 2.6 }))).not.toBe(selectKey(node()));
    expect(selectKey(node({ radius: 1.4 }))).not.toBe(selectKey(node()));
  });

  it('distinguishes no selection from a selection', () => {
    expect(selectKey(null)).not.toBe(selectKey(node()));
    expect(selectKey(null)).toBe(selectKey(null));
  });
});

describe('focusKey', () => {
  const bounds = (over: Partial<{ x: number; y: number; z: number; radius: number }> = {}) => {
    const { x = 1.437, y = -6.5, z = 0, radius = 2.41 } = over;
    return { center: [x, y, z] as [number, number, number], radius };
  };

  it('is the same for a subtree recomputed from a fresh layout', () => {
    // The regression this exists for: an emit landing mid-dive used to restart the
    // whole 700ms ease because the twelfth decimal moved.
    expect(focusKey(bounds({ x: 1.4370000000001 }))).toBe(focusKey(bounds()));
    expect(focusKey(bounds({ radius: 2.410000000001 }))).toBe(focusKey(bounds()));
  });

  it('changes when the subtree genuinely moves or grows', () => {
    // A subagent spawning grows the subtree, and the camera has to widen for it.
    expect(focusKey(bounds({ radius: 3.6 }))).not.toBe(focusKey(bounds()));
    expect(focusKey(bounds({ x: 3.9 }))).not.toBe(focusKey(bounds()));
  });

  it('distinguishes no focus from a focus', () => {
    expect(focusKey(null)).not.toBe(focusKey(bounds()));
    expect(focusKey(null)).toBe(focusKey(null));
  });
});
