// @vitest-environment jsdom
//
// Viz smoke test (Task 10, spec §6 / §12). WebGL is NOT available in jsdom, so we
// do NOT attempt a full GPU mount of the war-room here — the cinematic render is
// verified live in the overlay (/dev-viz.html). What we CAN honestly assert in
// jsdom is the two non-GPU guards the mount path actually relies on:
//
//   1. `mountWarRoom` throws when its root element is missing. That guard runs
//      BEFORE `createRoot`/`<Canvas>`, so it exercises the real exported function
//      without touching WebGL.
//   2. The RAF pause rule: the Canvas drives its `frameloop` from `frameloopFor`,
//      which returns 'never' (RAF halted) exactly when the window is hidden. We
//      assert the helper directly AND through a simulated `document.hidden`, since
//      that is the precise decision the live component makes on `visibilitychange`.
//
// (A successful mount-into-a-real-root path needs a WebGL context and is covered
// live, not here — see the report's viz-smoke rationale.)

import { afterEach, describe, expect, it, vi } from 'vitest';
import { mountWarRoom, unmountWarRoom } from './mount';
import { activeFor, frameloopFor } from './WarRoom';

afterEach(() => {
  // Reset module-level mount singletons so each case starts clean.
  unmountWarRoom();
  vi.restoreAllMocks();
});

describe('mountWarRoom guard (jsdom, no WebGL)', () => {
  it('throws a clear error when the root element is absent', () => {
    // No element with this id exists in the empty jsdom document.
    expect(() => mountWarRoom('definitely-not-here')).toThrow(/definitely-not-here/);
  });

  it('does not throw merely by importing the war-room module graph', () => {
    // Loading mount.tsx pulls WarRoom → R3F/postprocessing/three. Constructing
    // those modules (THREE.Color palette, etc.) must not blow up under node/jsdom.
    expect(typeof mountWarRoom).toBe('function');
    expect(typeof frameloopFor).toBe('function');
  });
});

describe('frameloopFor — RAF pauses when hidden', () => {
  it('halts the render loop when the window is hidden', () => {
    expect(frameloopFor(true)).toBe('never');
  });

  it('runs the render loop when the window is visible', () => {
    expect(frameloopFor(false)).toBe('always');
  });

  it('reflects a simulated document.hidden === true as a paused loop', () => {
    // dev/browser harness, no daemon summon: the component derives
    // `active = activeFor(scene.summoned, document.hidden)`, then
    // `frameloop={frameloopFor(!active)}`.
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    const active = activeFor(undefined, document.hidden); // false → paused
    expect(active).toBe(false);
    expect(frameloopFor(!active)).toBe('never');
  });

  it('reflects a simulated document.hidden === false as a running loop', () => {
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    const active = activeFor(undefined, document.hidden); // true → running
    expect(active).toBe(true);
    expect(frameloopFor(!active)).toBe('always');
  });
});

describe('activeFor — the war-room wakes on daemon summon OR a visible page', () => {
  it('is active when the daemon summoned the overlay, even with a hidden page', () => {
    // The packaged app's overlay is a hidden native window: document.hidden stays
    // true, but the warden_hotkey summon (summoned=true) must wake the loop. This
    // is the exact regression that left the live war-room blank.
    expect(activeFor(true, true)).toBe(true);
    expect(frameloopFor(!activeFor(true, true))).toBe('always');
  });

  it('is active when the page is visible even without a summon (dev/browser)', () => {
    expect(activeFor(false, false)).toBe(true);
    expect(activeFor(undefined, false)).toBe(true);
  });

  it('pauses only when neither summoned nor visible', () => {
    expect(activeFor(false, true)).toBe(false);
    expect(activeFor(undefined, true)).toBe(false);
    expect(frameloopFor(!activeFor(false, true))).toBe('never');
  });
});
