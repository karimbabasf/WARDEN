import { describe, it, expect, vi, afterEach } from 'vitest';
import { recordCanvas, recorderAvailable } from './recorder';

// The recorder is the EXPORT FALLBACK (recap). It must feature-detect
// `MediaRecorder` and reject CLEANLY when the runtime can't capture a canvas —
// the node vitest env has neither `MediaRecorder` nor a real canvas, which is
// exactly the unsupported path we assert here. No real recording is exercised
// (that is verified live in the overlay).
afterEach(() => {
  vi.unstubAllGlobals();
});

describe('recorder feature detection', () => {
  it('reports unavailable when MediaRecorder is absent (node/jsdom)', () => {
    expect(typeof (globalThis as any).MediaRecorder).toBe('undefined');
    expect(recorderAvailable()).toBe(false);
  });

  it('rejects cleanly (no throw) when MediaRecorder is unavailable', async () => {
    const fakeCanvas = {} as HTMLCanvasElement;
    await expect(recordCanvas(fakeCanvas, 500)).rejects.toThrow(/MediaRecorder|unsupported|unavailable/i);
  });

  it('rejects when the canvas cannot be captured even if MediaRecorder exists', async () => {
    // MediaRecorder present, but the canvas lacks captureStream → still a clean reject.
    vi.stubGlobal('MediaRecorder', class {} as unknown as typeof MediaRecorder);
    const canvasNoCapture = {} as HTMLCanvasElement;
    await expect(recordCanvas(canvasNoCapture, 500)).rejects.toThrow(/captureStream|unsupported|canvas/i);
  });

  it('reports available only when MediaRecorder exists', () => {
    vi.stubGlobal('MediaRecorder', class {} as unknown as typeof MediaRecorder);
    expect(recorderAvailable()).toBe(true);
  });
});
