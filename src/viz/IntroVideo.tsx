// IntroVideo.tsx — the branded entry clip that replaces the Remotion phosphor-boot
// `Intro` composition. Plays MOBIUS-intro.mp4 ONCE on first summon, muted (so the
// macOS WKWebView always autoplays it), then fades the overlay out and retires it
// through `onEnded` — the same completion contract the old PlayerHost intro path
// honoured, so WarRoom's once-per-launch `showIntro` gating is left untouched.
//
// The asset lives in /public, served at the web root by Vite and bundled into the
// Tauri app. It is referenced by absolute URL (not imported) so the file never
// goes through the bundler.

import { useCallback, useEffect, useRef, useState } from 'react';

const SRC = '/MOBIUS-intro.mp4';

// Opacity fade once the clip finishes. Matches the app's ~600ms transition idiom.
const FADE_MS = 600;

// Absolute backstop: if neither 'ended' nor 'error' ever fires (codec/decode
// trouble), force-retire so the intro can NEVER permanently block the UI. Generous
// against the known ~13.5s clip.
const SAFETY_MS = 20_000;

const overlayStyle: React.CSSProperties = {
  position: 'absolute',
  inset: 0,
  // One step above --z-controls (50), the top of the chrome z-scale in style.css.
  // The entry clip is a HARD GATE, so it must sit over the HUD / nav / panels /
  // controls — not under them (which is what z-index 3 did, leaking the app on top).
  zIndex: 60,
  // Capture ALL pointer input so nothing in the app is usable until the clip is
  // done. The war room is mounted behind us (so the fade reveals it) but is fully
  // covered and inert until we retire. Clicks are swallowed, not skip-to-end.
  pointerEvents: 'auto',
  // Opaque --bg fill so the app is never visible through or around the video —
  // covers the pre-roll frame and any object-fit:cover crop edges.
  background: '#020403',
  transition: `opacity ${FADE_MS}ms ease`,
};

const videoStyle: React.CSSProperties = {
  width: '100%',
  height: '100%',
  objectFit: 'cover',
  display: 'block',
};

export type IntroVideoProps = {
  /** Fired once the clip has finished AND faded out — the parent then unmounts us. */
  onEnded?: () => void;
};

export default function IntroVideo({ onEnded }: IntroVideoProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [fading, setFading] = useState(false);
  const ending = useRef(false); // fade-out has begun
  const retired = useRef(false); // onEnded has fired

  const retire = useCallback(() => {
    if (retired.current) return;
    retired.current = true;
    onEnded?.();
  }, [onEnded]);

  // Start the fade exactly once (clip ended, errored, or safety tripped).
  const finish = useCallback(() => {
    if (ending.current) return;
    ending.current = true;
    setFading(true);
  }, []);

  useEffect(() => {
    const v = videoRef.current;
    if (v) {
      // Muted autoplay is universally permitted; set muted imperatively too
      // (React's `muted` prop is famously unreliable) and nudge play() in case
      // the autoPlay attribute is gated for any reason.
      v.muted = true;
      void v.play().catch(finish); // even muted play refused → don't strand the UI
    }
    const safety = setTimeout(finish, SAFETY_MS);
    return () => clearTimeout(safety);
  }, [finish]);

  // Retire after the fade — via transitionend OR a matched timeout, whichever
  // lands first (retire() dedupes), so a dropped transitionend can't leave the
  // faded-out overlay sitting on top forever.
  useEffect(() => {
    if (!fading) return;
    const t = setTimeout(retire, FADE_MS + 80);
    return () => clearTimeout(t);
  }, [fading, retire]);

  return (
    <div
      style={{ ...overlayStyle, opacity: fading ? 0 : 1 }}
      data-intro-video=""
      aria-hidden="true"
      onTransitionEnd={(e) => {
        if (fading && e.propertyName === 'opacity') retire();
      }}
    >
      <video
        ref={videoRef}
        src={SRC}
        style={videoStyle}
        autoPlay
        muted
        playsInline
        preload="auto"
        onEnded={finish}
        onError={finish}
      />
    </div>
  );
}
