// @vitest-environment jsdom
//
// useLingering is what lets a cell leave the board visibly: a departed item stays
// mounted, flagged, for the length of its exit and then goes. The three things that
// matter are the order (live first, in their own order), the flag, and the prune.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { useLingering, type Lingering } from './hudLinger';

const EXIT = 200;
const id = (s: string) => s;

let latest: Lingering<string>[] = [];
let root: Root | null = null;
let container: HTMLDivElement | null = null;

function Probe({ items }: { items: string[] }) {
  latest = useLingering(items, id, EXIT);
  return null;
}

function render(items: string[]) {
  act(() => root!.render(<Probe items={items} />));
}

const flat = () => latest.map((l) => `${l.item}${l.leaving ? '-' : ''}`);

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
  vi.useRealTimers();
});

describe('useLingering', () => {
  it('passes a live list through untouched', () => {
    render(['a', 'b', 'c']);
    expect(flat()).toEqual(['a', 'b', 'c']);
  });

  it('keeps a departed item after the live ones, flagged, then prunes it', () => {
    render(['a', 'b', 'c']);
    render(['a', 'c']);
    expect(flat()).toEqual(['a', 'c', 'b-']);
    act(() => {
      vi.advanceTimersByTime(EXIT + 60);
    });
    expect(flat()).toEqual(['a', 'c']);
  });

  it('holds no slot for a ghost: a newcomer sits among the live items, ahead of it', () => {
    render(['a', 'b']);
    render(['a', 'd']);
    expect(flat()).toEqual(['a', 'd', 'b-']);
  });

  it('an item that comes back while leaving is simply live again', () => {
    render(['a', 'b']);
    render(['a']);
    expect(flat()).toEqual(['a', 'b-']);
    render(['a', 'b']);
    expect(flat()).toEqual(['a', 'b']);
  });

  it('prunes each ghost on its own clock', () => {
    render(['a', 'b', 'c']);
    render(['b', 'c']);
    act(() => {
      vi.advanceTimersByTime(EXIT / 2);
    });
    render(['c']);
    expect(flat()).toEqual(['c', 'a-', 'b-']);
    act(() => {
      vi.advanceTimersByTime(EXIT / 2 + 60);
    });
    expect(flat()).toEqual(['c', 'b-']);
    act(() => {
      vi.advanceTimersByTime(EXIT);
    });
    expect(flat()).toEqual(['c']);
  });

  it('does not re-render for a new array with the same members', () => {
    render(['a', 'b']);
    const before = latest;
    render(['a', 'b']);
    // A fresh input array yields a fresh output array (the memo keys on it), but no
    // ghost is created, so nothing is flagged and nothing is scheduled.
    expect(latest).not.toBe(before);
    expect(flat()).toEqual(['a', 'b']);
    expect(vi.getTimerCount()).toBe(0);
  });
});
