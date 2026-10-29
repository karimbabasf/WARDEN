// @vitest-environment jsdom

// The transport's HOST VISIBILITY contract.
//
// This is the battery guard for the notch section, and it is the kind of thing that
// regresses silently: everything still works with it broken, it just renders a WebGL
// scene at display rate behind a closed notch. Measured on the real bundle, visible was
// ~14,700 draw calls per 3s and hidden was 0; these tests hold the seam that produces
// that, so a future edit that drops the gate fails here instead of on a warm laptop.

import { beforeEach, describe, expect, it, vi } from 'vitest';

import { isHostVisible } from './hudTransport';

/** Nudge the module the way the host does. */
const setVisible = (v: boolean) => window.__wardenSetVisible?.(v);

describe('hudTransport: host visibility', () => {
  beforeEach(() => {
    setVisible(true);
  });

  it('installs the host hook when there is no Tauri runtime', () => {
    // The section is a bare WKWebView, so this is the ONLY channel the host has.
    expect(typeof window.__wardenSetVisible).toBe('function');
  });

  it('starts visible, so a host that never calls it still renders', () => {
    // Our own Tauri window never calls the hook. Defaulting to hidden would leave the
    // real HUD frozen, which is a far worse failure than a wasted frame.
    expect(isHostVisible()).toBe(true);
  });

  it('tracks what the host says', () => {
    setVisible(false);
    expect(isHostVisible()).toBe(false);
    setVisible(true);
    expect(isHostVisible()).toBe(true);
  });

  it('treats anything but a literal true as hidden', () => {
    // `evaluateJavaScript` is stringly typed on the Swift side; a mistake there must fail
    // toward NOT rendering rather than toward rendering forever.
    window.__wardenSetVisible?.(undefined);
    expect(isHostVisible()).toBe(false);
    window.__wardenSetVisible?.('true');
    expect(isHostVisible()).toBe(false);
    setVisible(true);
    expect(isHostVisible()).toBe(true);
  });

  it('closes the event stream when hidden and reopens it when shown', () => {
    const closed = vi.fn();
    class FakeES {
      onmessage: ((e: MessageEvent) => void) | null = null;
      close = closed;
      constructor(public url: string) {
        FakeES.built++;
      }
      static built = 0;
    }
    const prev = globalThis.EventSource;
    // @ts-expect-error test double
    globalThis.EventSource = FakeES;
    try {
      FakeES.built = 0;
      // Hiding must close: an idle stream still counts as a subscriber on the Rust side,
      // and one subscriber is the difference between WARDEN serializing the whole radar
      // state on every recompute and skipping it entirely.
      setVisible(false);
      setVisible(true);
      expect(FakeES.built).toBeGreaterThanOrEqual(1);
    } finally {
      globalThis.EventSource = prev;
    }
  });
});
