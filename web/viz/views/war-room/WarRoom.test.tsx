// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import {
  activeFor,
  isDiscoveryHomeDoubleClickAllowed,
  railInsetsFrom,
  RADAR_VISIBLE_PULL_MS,
  useSettledFocus,
} from './WarRoom';
import { frameloopFor, BLUR_SETTLE_MS } from '@/viz/shared/scene/frameloop';

describe('activeFor', () => {
  it('pauses only on minimize', () => {
    expect(activeFor(true, false, true)).toBe(false);
    expect(activeFor(false, false, true)).toBe(false);
  });

  it('stays active while summoned even if the page reports hidden', () => {
    expect(activeFor(true, true, false)).toBe(true);
  });

  it('keys off page visibility when not summoned (dev/browser)', () => {
    expect(activeFor(false, false, false)).toBe(true);
    expect(activeFor(false, true, false)).toBe(false);
  });
});

describe('isDiscoveryHomeDoubleClickAllowed', () => {
  it('is blocked while an agent is selected or the camera is focused in', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: 'a1', focusDepth: 0, eventTarget: null })).toBe(false);
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 2, eventTarget: null })).toBe(false);
  });

  it('is allowed on the empty void', () => {
    const div = document.createElement('div');
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: div })).toBe(true);
  });

  it('is blocked when the double-click lands on a control', () => {
    const btn = document.createElement('button');
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: btn })).toBe(false);
  });
});

// The laggy zoom. The insets feed CameraRig's pose effects as DEPENDENCIES, so every
// distinct value published during a selection restarts the 700ms fly from wherever
// the camera had reached. The dock slides its 16px in over 220ms, so a rect-based
// read published a different number on each of the three staggered measures the old
// code took, and the dive stalled and re-eased twice on the way in.
describe('railInsetsFrom', () => {
  const RAIL = { offsetLeft: 14, offsetWidth: 320 };
  // Same dock, sampled at three points of its 16px slide. Layout geometry does not
  // move, so all three are the same element in the same place.
  const DOCK = { offsetLeft: 1106, offsetWidth: 320 };

  it('reserves the rail on the left and the dock on the right', () => {
    expect(railInsetsFrom(RAIL, DOCK, true, 1440)).toEqual({ left: 334, right: 334 });
  });

  it('publishes ONE value across the dock slide, so the camera fly is never restarted', () => {
    // Whatever moment of the transition it is called at, the answer is identical:
    // the transform is not in the numbers at all.
    const readings = [0, 1, 2].map(() => railInsetsFrom(RAIL, DOCK, true, 1440));
    expect(new Set(readings.map((r) => `${r.left}/${r.right}`)).size).toBe(1);
  });

  it('reserves nothing on the right while the dock is closed', () => {
    expect(railInsetsFrom(RAIL, DOCK, false, 1440).right).toBe(0);
  });

  it('treats a rail hidden by the narrow-window media query as zero width', () => {
    // A fixed element reports offsetParent === null, so width is the only honest
    // test of whether it is on screen.
    expect(railInsetsFrom({ offsetLeft: 0, offsetWidth: 0 }, DOCK, true, 900).left).toBe(0);
    expect(railInsetsFrom(null, null, true, 900)).toEqual({ left: 0, right: 0 });
  });

  it('never reports a negative inset for a dock wider than the window', () => {
    expect(railInsetsFrom(RAIL, { offsetLeft: 1600, offsetWidth: 320 }, true, 1440).right).toBe(0);
  });
});

describe('RADAR_VISIBLE_PULL_MS', () => {
  it('is a light polling cadence, not a busy loop', () => {
    expect(RADAR_VISIBLE_PULL_MS).toBeGreaterThan(0);
    expect(RADAR_VISIBLE_PULL_MS).toBeLessThan(5000);
  });
});

// The stall Karim reported as "clicking the app tab and clicking out glitches for a
// hot second": every focus edge swapped the Canvas frameloop, and ordinary clicking
// produces those edges in pairs.
describe('useSettledFocus', () => {
  let root: Root | null = null;
  let container: HTMLDivElement | null = null;
  const seen: boolean[] = [];

  function Probe({ raw }: { raw: boolean }) {
    seen.push(useSettledFocus(raw));
    return null;
  }

  function mount(raw: boolean) {
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => root!.render(<Probe raw={raw} />));
  }

  function update(raw: boolean) {
    act(() => root!.render(<Probe raw={raw} />));
  }

  const latest = () => seen[seen.length - 1];

  afterEach(() => {
    act(() => root?.unmount());
    container?.remove();
    root = null;
    container = null;
    seen.length = 0;
    vi.useRealTimers();
  });

  it('holds focus through a blur that is immediately taken back', () => {
    vi.useFakeTimers();
    mount(true);
    expect(latest()).toBe(true);

    // A click that passes through the window: blur, then focus a few frames later.
    update(false);
    act(() => vi.advanceTimersByTime(BLUR_SETTLE_MS / 4));
    expect(latest()).toBe(true);
    update(true);
    act(() => vi.advanceTimersByTime(BLUR_SETTLE_MS * 4));

    expect(latest()).toBe(true);
    // ...and the loop mode never left 'always', so nothing was torn down at all.
    expect(seen.every((f) => frameloopFor(false, f) === 'always')).toBe(true);
  });

  it('does drop to the paced rate once the blur really holds', () => {
    vi.useFakeTimers();
    mount(true);
    update(false);
    act(() => vi.advanceTimersByTime(BLUR_SETTLE_MS + 20));
    expect(latest()).toBe(false);
    expect(frameloopFor(false, latest())).toBe('demand');
  });

  it('comes back instantly, with no hold on the way up', () => {
    vi.useFakeTimers();
    mount(false);
    update(false);
    act(() => vi.advanceTimersByTime(BLUR_SETTLE_MS + 20));
    expect(latest()).toBe(false);

    update(true);
    expect(latest()).toBe(true); // no timer advanced
  });

  it('settles fast enough to stay a rounding error against a blurred session', () => {
    expect(BLUR_SETTLE_MS).toBeGreaterThanOrEqual(150);
    expect(BLUR_SETTLE_MS).toBeLessThanOrEqual(1000);
  });
});
