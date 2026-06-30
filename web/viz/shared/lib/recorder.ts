// recorder.ts — the RECAP export fallback (spec §6.5 role 3).
//
// `@remotion/renderer` (true headless render) is explicitly post-M2 (risk
// R-Rem). For M2 the shareable recap is produced by capturing the LIVE war-room
// <canvas> with the browser-native `MediaRecorder` — zero render backend, no
// extra dependency. It is strictly a FALLBACK: it must feature-detect and
// reject CLEANLY on any runtime that can't capture a canvas (older webviews,
// the node/jsdom test env) rather than throwing or hanging.
//
// Output is a single webm Blob the caller can download or hand to a share sheet.

/** The webm codecs we try, best-first; the first one MediaRecorder accepts wins. */
const WEBM_MIME_CANDIDATES = [
  'video/webm;codecs=vp9',
  'video/webm;codecs=vp8',
  'video/webm',
];

/**
 * True only when the runtime can actually capture a canvas to webm — i.e. both
 * `MediaRecorder` exists AND it can emit one of our webm mime types. Used to
 * gate the "export recap" affordance so it is never offered when it can't work.
 */
export function recorderAvailable(): boolean {
  const MR = (globalThis as { MediaRecorder?: typeof MediaRecorder }).MediaRecorder;
  if (typeof MR === 'undefined') return false;
  // Some engines expose MediaRecorder but not isTypeSupported; treat the bare
  // presence as "maybe" (true) and let recordCanvas's mime probe be the real
  // gate — but if isTypeSupported exists, require at least one webm type.
  if (typeof MR.isTypeSupported === 'function') {
    return WEBM_MIME_CANDIDATES.some(m => MR.isTypeSupported(m));
  }
  return true;
}

function pickMimeType(MR: typeof MediaRecorder): string | undefined {
  if (typeof MR.isTypeSupported !== 'function') return undefined; // let MR default
  return WEBM_MIME_CANDIDATES.find(m => MR.isTypeSupported(m));
}

/**
 * Record `canvas` for `ms` milliseconds and resolve with a webm `Blob`.
 *
 * Rejects cleanly (a plain `Error`, never an unhandled throw) when:
 *   • `MediaRecorder` is unavailable (feature-detect miss),
 *   • the canvas can't be captured (`captureStream` missing / throws),
 *   • the recorder errors mid-capture.
 * The promise always settles — a watchdog stops the recorder even if the
 * browser never fires `onstop`.
 */
export function recordCanvas(canvas: HTMLCanvasElement, ms: number): Promise<Blob> {
  return new Promise<Blob>((resolve, reject) => {
    const MR = (globalThis as { MediaRecorder?: typeof MediaRecorder }).MediaRecorder;
    if (typeof MR === 'undefined') {
      reject(new Error('recordCanvas: MediaRecorder unavailable in this runtime'));
      return;
    }

    // `captureStream` is the canvas→MediaStream bridge; guard it explicitly so a
    // bare object (test env) or an older webview rejects instead of throwing.
    const capture = (canvas as HTMLCanvasElement & {
      captureStream?: (fps?: number) => MediaStream;
    }).captureStream;
    if (typeof capture !== 'function') {
      reject(new Error('recordCanvas: canvas.captureStream unsupported'));
      return;
    }

    let stream: MediaStream;
    try {
      stream = capture.call(canvas, 30);
    } catch (err) {
      reject(new Error(`recordCanvas: captureStream failed (${String(err)})`));
      return;
    }

    let recorder: MediaRecorder;
    try {
      const mimeType = pickMimeType(MR);
      recorder = new MR(stream, mimeType ? { mimeType } : undefined);
    } catch (err) {
      reject(new Error(`recordCanvas: MediaRecorder construction failed (${String(err)})`));
      return;
    }

    const chunks: BlobPart[] = [];
    let settled = false;
    let watchdog: ReturnType<typeof setTimeout> | undefined;

    const stop = () => {
      try {
        if (recorder.state !== 'inactive') recorder.stop();
      } catch {
        /* already stopped */
      }
    };

    recorder.ondataavailable = ev => {
      if (ev.data && ev.data.size > 0) chunks.push(ev.data);
    };
    recorder.onerror = () => {
      if (settled) return;
      settled = true;
      if (watchdog) clearTimeout(watchdog);
      stop();
      reject(new Error('recordCanvas: MediaRecorder error during capture'));
    };
    recorder.onstop = () => {
      if (settled) return;
      settled = true;
      if (watchdog) clearTimeout(watchdog);
      resolve(new Blob(chunks, { type: recorder.mimeType || 'video/webm' }));
    };

    try {
      recorder.start();
    } catch (err) {
      settled = true;
      reject(new Error(`recordCanvas: MediaRecorder.start failed (${String(err)})`));
      return;
    }

    // Stop after `ms`; a tiny grace lets the final dataavailable flush.
    setTimeout(stop, Math.max(0, ms));
    watchdog = setTimeout(() => {
      if (settled) return;
      stop();
      // Give onstop a tick; if it never comes, resolve with whatever we have.
      setTimeout(() => {
        if (settled) return;
        settled = true;
        resolve(new Blob(chunks, { type: 'video/webm' }));
      }, 250);
    }, Math.max(0, ms) + 1500);
  });
}
