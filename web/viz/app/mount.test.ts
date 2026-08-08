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
import { RADAR_VISIBLE_PULL_MS, activeFor } from '@/viz/views/war-room/WarRoom';
import { backgroundTickDue, BACKGROUND_FPS, frameloopFor } from '@/viz/shared/scene/frameloop';

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

describe('activeFor — animate unless minimized (blur is irrelevant)', () => {
  it('a summoned overlay stays active when blurred / on another screen', () => {
    expect(activeFor(true, false)).toBe(true);
    // even if the page-visibility flag is stale-true right after a native show
    expect(activeFor(true, true)).toBe(true);
  });

  it('pauses only when minimized', () => {
    expect(activeFor(true, false, true)).toBe(false);
    expect(frameloopFor(!activeFor(true, false, true))).toBe('never');
  });

  it('a visible dev/browser page (no summon) is active, and pauses when tab-hidden', () => {
    expect(activeFor(false, false)).toBe(true);
    expect(activeFor(undefined, false)).toBe(true);
    expect(activeFor(false, true)).toBe(false);
  });

  it('minimize overrides everything', () => {
    expect(activeFor(false, false, true)).toBe(false);
    expect(activeFor(undefined, false, true)).toBe(false);
  });
});

describe('RADAR visible-tab refresh', () => {
  it('keeps the fallback pull under one second', () => {
    expect(RADAR_VISIBLE_PULL_MS).toBeLessThanOrEqual(1000);
  });
});

// The CPU fix. WARDEN sits open behind the editor all day, so "visible but blurred" is
// its normal state, and rendering a rotating constellation at display rate there cost
// about half a core for frames nobody looked at. `document.hidden` cannot catch it: on
// macOS it only goes true on minimize/hide, never when the window is merely behind
// another. Focus is therefore a SEPARATE axis, and it changes render rate only.
describe('frameloopFor: an unfocused window renders paced, not at display rate', () => {
  it('renders at full rate only while focused', () => {
    expect(frameloopFor(false, true)).toBe('always');
  });

  it('drops a visible but blurred window to demand', () => {
    expect(frameloopFor(false, false)).toBe('demand');
  });

  it('still stops entirely when hidden, focused or not', () => {
    expect(frameloopFor(true, true)).toBe('never');
    expect(frameloopFor(true, false)).toBe('never');
  });

  it('defaults to focused so existing call sites keep full rate', () => {
    expect(frameloopFor(false)).toBe('always');
    expect(frameloopFor(true)).toBe('never');
  });

  it('never returns always for a blurred window (the regression that burned a core)', () => {
    for (const summoned of [true, false, undefined]) {
      const active = activeFor(summoned, false, false);
      expect(frameloopFor(!active, false)).not.toBe('always');
    }
  });

  // The bound that matters is the LOWER one. A blurred window is the one the user
  // stares at while working in another app, and the first version of this fix paced it
  // at 8fps, which reads as a stutter rather than as a saving. 30 is the floor for
  // motion that still looks continuous; the upper bound keeps it a fraction of a
  // 120Hz display rate, which is what made the unpaced version expensive.
  it('paces the background low enough to matter but high enough to look continuous', () => {
    expect(BACKGROUND_FPS).toBeGreaterThanOrEqual(30);
    expect(BACKGROUND_FPS).toBeLessThanOrEqual(60);
  });

  it('fires the first paced tick immediately, then on the period', () => {
    expect(backgroundTickDue(Number.NaN, 30)).toBe(true);
    expect(backgroundTickDue(1000 / 30, 30)).toBe(true);
    expect(backgroundTickDue(0, 30)).toBe(false);
    expect(backgroundTickDue(5, 30)).toBe(false);
  });

  // The judder guard: at 30fps on a 60Hz panel the period is exactly two refreshes, so
  // a strict `>=` loses to float jitter and the tick slips a whole refresh. A tick that
  // is a hair early must still count, or the rate alternates 30/20/30/20.
  it('accepts a tick that lands a hair before the nominal period', () => {
    const period = 1000 / 30;
    expect(backgroundTickDue(period - 0.001, 30)).toBe(true);
    // Two refreshes on a 60Hz panel, which is the real cadence this has to catch.
    expect(backgroundTickDue(2 * (1000 / 60), 30)).toBe(true);
    // ...but one refresh short is genuinely early and must not fire.
    expect(backgroundTickDue(1000 / 60, 30)).toBe(false);
  });
});
