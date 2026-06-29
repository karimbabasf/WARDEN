// HandMode.tsx — the on-screen surface for hand mode.
//
// Renders the toggle (button + 'H' shortcut), the "camera live" indicator, the hidden
// decoding <video> MediaPipe reads from, and the corner HUD canvas the tracker draws the
// stylised hand into. All of hand mode's React state lives here, so WarRoom only has to
// mount <HandMode active={active} />. Camera work is delegated to useHandTracker, which
// runs ONLY while `enabled` (= the user's toggle AND the overlay being active).

import { useCallback, useEffect, useRef, useState } from 'react';
import { useHandTracker, type HandTrackerStatus } from './useHandTracker';
import './handMode.css';

const STATUS_LABEL: Record<HandTrackerStatus, string> = {
  off: 'Hand mode',
  loading: 'Starting camera…',
  active: 'Hand mode · on',
  denied: 'Camera blocked',
  'no-camera': 'No camera found',
  error: 'Camera error',
};

function isTypingTarget(t: EventTarget | null): boolean {
  if (typeof Element === 'undefined' || !(t instanceof Element)) return false;
  return t.closest('input, textarea, select, [contenteditable="true"]') !== null;
}

export function HandMode({ active }: { active: boolean }) {
  // `handMode` is the user's INTENT (what the toggle says); `enabled` also requires the
  // overlay to be active, so minimising the window releases the camera.
  const [handMode, setHandMode] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const enabled = handMode && active;
  const status = useHandTracker(enabled, videoRef, canvasRef);

  // A failed camera flips the toggle back off (so it never sits "on" with nothing
  // running) and surfaces a brief reason instead.
  useEffect(() => {
    if (status === 'denied' || status === 'no-camera' || status === 'error') {
      setHandMode(false);
      setNotice(STATUS_LABEL[status]);
      const id = window.setTimeout(() => setNotice(null), 4500);
      return () => window.clearTimeout(id);
    }
  }, [status]);

  const toggle = useCallback(() => {
    setNotice(null);
    setHandMode((on) => !on);
  }, []);

  // 'H' toggles hand mode (ignored while typing in a field or with a modifier held, so
  // it never collides with shortcuts or text entry).
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      if (ev.key !== 'h' && ev.key !== 'H') return;
      if (isTypingTarget(ev.target)) return;
      ev.preventDefault();
      toggle();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [toggle]);

  const liveCamera = status === 'active';

  return (
    <>
      {/* Hidden but DECODING — MediaPipe reads frames off this element. Kept tiny +
          transparent rather than display:none (which can halt frame production). It only
          receives a stream while hand mode is on; there is no camera access otherwise. */}
      <video ref={videoRef} className="wd-hand-video" playsInline muted aria-hidden />

      {/* Stylised hand HUD — mounted only while hand mode is on; pointer-events:none so it
          never intercepts clicks on the war-room beneath it. */}
      {handMode && <canvas ref={canvasRef} className="wd-hand-hud" width={360} height={420} aria-hidden />}

      <div className={`wd-hand-dock${handMode ? ' is-on' : ''}`}>
        {notice && <span className="wd-hand-notice" role="status">{notice}</span>}
        {liveCamera && (
          <span className="wd-hand-live" title="Your camera is on">
            <span className="wd-hand-live-dot" aria-hidden />
            camera live
          </span>
        )}
        <button
          type="button"
          className={`wd-hand-toggle${handMode ? ' is-on' : ''}`}
          aria-pressed={handMode}
          title="Hand mode (H) — pinch and drag to orbit the war-room"
          onClick={toggle}
        >
          <svg className="wd-hand-glyph" viewBox="0 0 24 24" aria-hidden>
            <path
              d="M7 11V6.5a1.3 1.3 0 0 1 2.6 0V10m0 0V4.8a1.3 1.3 0 0 1 2.6 0V10m0 0V5.4a1.3 1.3 0 0 1 2.6 0V11m0-1.2a1.3 1.3 0 0 1 2.6 0V14c0 3.3-2.4 6-6 6h-1c-1.7 0-2.7-.6-3.8-1.9L4.6 14.7c-.8-1-.5-2 .4-2.5.7-.4 1.5-.2 2 .4L8 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
          <span className="wd-hand-label">{STATUS_LABEL[handMode ? status : 'off']}</span>
        </button>
      </div>
    </>
  );
}

export default HandMode;
