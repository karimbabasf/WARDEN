// useHandTracker.ts — the DOM-side half of hand mode.
//
// Owns the whole webcam → MediaPipe → gesture pipeline, but ONLY while `enabled` is
// true. On enable it lazily loads the (locally vendored, offline) HandLandmarker model,
// opens the camera, and runs a requestAnimationFrame detection loop that: recognises
// pinches (createPinchTracker), turns a pinch-drag into an orbit delta (updateOrbitGesture),
// pushes that delta onto the gestureBus for CameraRig to drain, and draws the stylised
// hand HUD. On disable / unmount / overlay-hide it cancels the loop AND stops every
// camera track — the webcam light goes dark, nothing watches you until you ask again.
//
// The model is cached at module scope so re-toggling is instant (no 18 MB reload); the
// camera STREAM is never cached — it is always fully released on teardown (privacy).

import { useEffect, useRef, useState, type RefObject } from 'react';
import type { HandLandmarker as HandLandmarkerT, HandLandmarkerResult } from '@mediapipe/tasks-vision';
import { createPinchTracker, type PinchTracker } from './pinchRecognition';
import { createOrbitGestureState, updateOrbitGesture, type OrbitGestureState } from './gestureOrbit';
import type { HandLandmark, Pinch } from './types';
import { gestureBus, pushGestureRotation, pushGestureZoom, resetGestureBus } from './gestureBus';

export type HandTrackerStatus = 'off' | 'loading' | 'active' | 'denied' | 'no-camera' | 'error';

// Local asset URLs vendored into public/ (see the M-gesture asset step). Same contract
// as the source orbit-snap project, so the model + wasm load fully offline.
const WASM_PATH = '/mediapipe/wasm';
const MODEL_PATH = '/models/hand_landmarker.task';

// Detection confidence — ported verbatim from orbit-snap so the feel matches.
const DETECTION_CONFIDENCE = 0.58;

// Standard MediaPipe 21-landmark hand skeleton (for the HUD only).
const HAND_BONES: ReadonlyArray<readonly [number, number]> = [
  [0, 1], [1, 2], [2, 3], [3, 4], // thumb
  [0, 5], [5, 6], [6, 7], [7, 8], // index
  [5, 9], [9, 10], [10, 11], [11, 12], // middle
  [9, 13], [13, 14], [14, 15], [15, 16], // ring
  [13, 17], [17, 18], [18, 19], [19, 20], // pinky
  [0, 17], // palm base
];
const THUMB_TIP = 4;
const INDEX_TIP = 8;

// Phosphor palette (mirrors style.css tokens; canvas can't read CSS vars cheaply).
const C_BONE = 'rgba(118, 255, 157, 0.5)'; // --green, dimmed
const C_DOT = '#76ff9d'; // --green
const C_PINCH = '#ffd166'; // --warn (amber) — the active thumb/index + pinch ring

// Module-scoped model cache — survives toggles so re-enabling is instant. The MediaPipe
// package is dynamically imported on first use, so nothing camera-related sits in the
// initial bundle — true to "camera off until asked" (zero cost until hand mode).
let sharedLandmarker: HandLandmarkerT | null = null;
let loadPromise: Promise<HandLandmarkerT> | null = null;

async function loadLandmarker(): Promise<HandLandmarkerT> {
  if (sharedLandmarker) return sharedLandmarker;
  if (!loadPromise) {
    loadPromise = (async () => {
      const { FilesetResolver, HandLandmarker } = await import('@mediapipe/tasks-vision');
      const fileset = await FilesetResolver.forVisionTasks(WASM_PATH);
      const make = (delegate: 'GPU' | 'CPU'): Promise<HandLandmarkerT> =>
        HandLandmarker.createFromOptions(fileset, {
          baseOptions: { modelAssetPath: MODEL_PATH, delegate },
          runningMode: 'VIDEO',
          numHands: 2,
          minHandDetectionConfidence: DETECTION_CONFIDENCE,
          minHandPresenceConfidence: DETECTION_CONFIDENCE,
          minTrackingConfidence: DETECTION_CONFIDENCE,
        });
      let lm: HandLandmarkerT;
      try {
        lm = await make('GPU'); // fast path
      } catch {
        lm = await make('CPU'); // headless / weak-GPU fallback
      }
      sharedLandmarker = lm;
      return lm;
    })();
    // On failure, clear the promise so a later toggle can retry the load.
    loadPromise.catch(() => {
      loadPromise = null;
    });
  }
  return loadPromise;
}

// Draw the stylised hand skeleton into the HUD canvas. Mirrored on X so it reads like a
// mirror (move your right hand right → the skeleton goes right). Cosmetic only — the
// orbit math uses raw coords; camera direction is reconciled by the signs in CameraRig.
function drawHud(canvas: HTMLCanvasElement, hands: HandLandmark[][], pinches: Pinch[]): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  if (hands.length === 0) return;

  const px = (x: number) => (1 - x) * w; // mirror X
  const py = (y: number) => y * h;

  hands.forEach((lms, hi) => {
    if (!lms || lms.length < 21) return;
    const pinching = pinches.some((p) => p.id === hi);

    // bones
    ctx.lineWidth = 2;
    ctx.strokeStyle = C_BONE;
    ctx.beginPath();
    for (const [a, b] of HAND_BONES) {
      ctx.moveTo(px(lms[a].x), py(lms[a].y));
      ctx.lineTo(px(lms[b].x), py(lms[b].y));
    }
    ctx.stroke();

    // landmark dots
    for (let i = 0; i < lms.length; i++) {
      const hot = pinching && (i === THUMB_TIP || i === INDEX_TIP);
      ctx.fillStyle = hot ? C_PINCH : C_DOT;
      ctx.beginPath();
      ctx.arc(px(lms[i].x), py(lms[i].y), hot ? 5 : 2.6, 0, Math.PI * 2);
      ctx.fill();
    }

    // pinch ring at the thumb/index midpoint
    if (pinching) {
      const mx = (px(lms[THUMB_TIP].x) + px(lms[INDEX_TIP].x)) / 2;
      const my = (py(lms[THUMB_TIP].y) + py(lms[INDEX_TIP].y)) / 2;
      ctx.lineWidth = 2;
      ctx.strokeStyle = C_PINCH;
      ctx.beginPath();
      ctx.arc(mx, my, 13, 0, Math.PI * 2);
      ctx.stroke();
    }
  });

  // Two-hand zoom axis: a dashed line linking the two pinch points signals "spread to
  // zoom / pinch to come back" while both hands are pinching.
  if (pinches.length >= 2) {
    ctx.strokeStyle = C_PINCH;
    ctx.lineWidth = 1.5;
    ctx.setLineDash([5, 5]);
    ctx.beginPath();
    ctx.moveTo(px(pinches[0].point.x), py(pinches[0].point.y));
    ctx.lineTo(px(pinches[1].point.x), py(pinches[1].point.y));
    ctx.stroke();
    ctx.setLineDash([]);
  }
}

export function useHandTracker(
  enabled: boolean,
  videoRef: RefObject<HTMLVideoElement | null>,
  canvasRef: RefObject<HTMLCanvasElement | null>,
): HandTrackerStatus {
  const [status, setStatus] = useState<HandTrackerStatus>('off');
  const rafRef = useRef<number | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const pinchTrackerRef = useRef<PinchTracker | null>(null);
  const orbitStateRef = useRef<OrbitGestureState | null>(null);
  const lastVideoTimeRef = useRef<number>(-1);

  useEffect(() => {
    if (!enabled) {
      setStatus('off');
      return;
    }
    let cancelled = false;

    const loop = () => {
      rafRef.current = requestAnimationFrame(loop);
      const video = videoRef.current;
      const lm = sharedLandmarker;
      const tracker = pinchTrackerRef.current;
      const orbit = orbitStateRef.current;
      if (!video || !lm || !tracker || !orbit || video.readyState < 2) return;

      // Only run detection on a genuinely new camera frame — MediaPipe throws on a
      // repeated timestamp, and there's nothing to compute if the frame didn't change.
      const t = video.currentTime;
      if (t === lastVideoTimeRef.current) return;
      lastVideoTimeRef.current = t;

      let result: HandLandmarkerResult;
      try {
        result = lm.detectForVideo(video, performance.now());
      } catch {
        return;
      }
      const hands = (result.landmarks ?? []) as HandLandmark[][];
      const pinches = tracker.recognize(hands);
      const out = updateOrbitGesture(orbit, pinches);
      if (out.dragging && (out.azimuthDelta !== 0 || out.elevationDelta !== 0)) {
        pushGestureRotation(out.azimuthDelta, out.elevationDelta);
      }
      if (out.radiusDelta !== 0) {
        pushGestureZoom(out.radiusDelta);
      }
      const canvas = canvasRef.current;
      if (canvas) drawHud(canvas, hands, pinches);
    };

    async function start() {
      setStatus('loading');
      try {
        await loadLandmarker();
      } catch {
        if (!cancelled) setStatus('error');
        return;
      }
      if (cancelled) return;

      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: 'user', width: 640, height: 480 },
          audio: false,
        });
      } catch (err) {
        if (cancelled) return;
        const name = (err as DOMException)?.name;
        setStatus(name === 'NotAllowedError' ? 'denied' : name === 'NotFoundError' ? 'no-camera' : 'error');
        return;
      }
      if (cancelled) {
        stream.getTracks().forEach((tr) => tr.stop());
        return;
      }
      streamRef.current = stream;
      const video = videoRef.current;
      if (!video) {
        stream.getTracks().forEach((tr) => tr.stop());
        return;
      }
      video.srcObject = stream;
      try {
        await video.play();
      } catch {
        /* autoplay can reject silently; the loop tolerates a not-yet-playing video */
      }

      // Fresh per-session gesture state (no drag carried over from a prior session).
      pinchTrackerRef.current = createPinchTracker();
      orbitStateRef.current = createOrbitGestureState();
      lastVideoTimeRef.current = -1;
      gestureBus.enabled = true;
      setStatus('active');
      rafRef.current = requestAnimationFrame(loop);
    }

    void start();

    return () => {
      cancelled = true;
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
      const video = videoRef.current;
      if (video) {
        try {
          video.pause();
        } catch {
          /* ignore */
        }
        video.srcObject = null;
      }
      if (streamRef.current) {
        streamRef.current.getTracks().forEach((tr) => tr.stop()); // release camera — light off
        streamRef.current = null;
      }
      pinchTrackerRef.current = null;
      orbitStateRef.current = null;
      resetGestureBus();
      const canvas = canvasRef.current;
      const cx = canvas?.getContext('2d');
      if (canvas && cx) cx.clearRect(0, 0, canvas.width, canvas.height);
    };
  }, [enabled, videoRef, canvasRef]);

  return status;
}
