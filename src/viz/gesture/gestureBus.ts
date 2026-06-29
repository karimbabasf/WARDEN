// gestureBus.ts — the one-way input bus between the DOM-side hand tracker and the
// R3F-side CameraRig.
//
// Why a module singleton and not React state / context: this is TRANSIENT per-frame
// input. It must never trigger a re-render, and it has to cross the DOM↔R3F reconciler
// boundary that React context can't cross without an explicit bridge. There is exactly
// one producer (`useHandTracker`) and one consumer (`CameraRig`), so a plain shared
// object is the simplest correct thing.
//
// Why ACCUMULATED deltas (not a live value): the tracker runs on the webcam's clock
// (~30fps) while the render loop runs at ~60fps, so the producer ADDS each detection's
// rotation/zoom onto the pending totals and the consumer DRAINS (reads + zeroes) once
// per render frame — so every produced delta is applied exactly once, fps mismatch or not.
//
// Two channels: rotation (theta/phi, single-hand pinch-drag) and zoom (radius, two-hand
// pinch-spread). They are mutually exclusive per frame by construction (gestureOrbit
// emits one or the other), but both ride the same bus.

export interface GestureBus {
  /** True only while hand mode is on AND the overlay is active. Set by useHandTracker. */
  enabled: boolean;
  /** Accumulated, not-yet-applied azimuth (theta) rotation, radians. +ve = drag right. */
  pendingTheta: number;
  /** Accumulated, not-yet-applied polar (phi) rotation, radians. +ve = drag up. */
  pendingPhi: number;
  /** Accumulated, not-yet-applied dolly, in normalized hand-spread units. +ve = hands apart. */
  pendingRadius: number;
}

export const gestureBus: GestureBus = {
  enabled: false,
  pendingTheta: 0,
  pendingPhi: 0,
  pendingRadius: 0,
};

/** Producer: add one detection frame's pinch-drag rotation onto the pending totals. */
export function pushGestureRotation(dTheta: number, dPhi: number): void {
  gestureBus.pendingTheta += dTheta;
  gestureBus.pendingPhi += dPhi;
}

/** Producer: add one detection frame's two-hand spread change onto the pending dolly. */
export function pushGestureZoom(dRadius: number): void {
  gestureBus.pendingRadius += dRadius;
}

/** Consumer: read and clear all pending deltas in one shot (drain-once semantics). */
export function drainGesture(): { theta: number; phi: number; radius: number } {
  const theta = gestureBus.pendingTheta;
  const phi = gestureBus.pendingPhi;
  const radius = gestureBus.pendingRadius;
  gestureBus.pendingTheta = 0;
  gestureBus.pendingPhi = 0;
  gestureBus.pendingRadius = 0;
  return { theta, phi, radius };
}

/** Reset everything (called when hand mode turns off, so stale deltas can't linger). */
export function resetGestureBus(): void {
  gestureBus.enabled = false;
  gestureBus.pendingTheta = 0;
  gestureBus.pendingPhi = 0;
  gestureBus.pendingRadius = 0;
}
