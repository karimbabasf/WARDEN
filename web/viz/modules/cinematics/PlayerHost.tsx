// PlayerHost.tsx — the LAZY Remotion <Player> overlay host.
//
// This is the ONLY module that imports `@remotion/player` and the compositions,
// so it forms a single lazy Vite chunk. WarRoom.tsx reaches it exclusively via
// React.lazy(), so Remotion never lands in the main bundle and never runs on the
// ⌘⌥⌃M summon hot path (risk R-Bundle / R-Rem). It mounts ABOVE the R3F
// canvas inside #war-room-root and plays:
//   • Reveal — on phase==='reveal', driven by the REAL findings handed in
//
// (The branded boot intro is no longer a Remotion clip: WarRoom now plays the
// pre-rendered MOBIUS-intro.mp4 via <IntroVideo>. The Intro composition is kept
// for the render-intro pre-render stub, so PlayerHost is reveal-only.)
//
// Reveal plays LIVE through <Player> (autoPlay, no controls), which needs no
// render backend — equivalent to a pre-render from the user's seat. The 'ended'
// event is wired via the PlayerRef (Remotion has no onEnded prop) so the host
// can notify the parent when the clip finishes.

import { useEffect, useMemo, useRef } from 'react';
import { Player, type PlayerRef } from '@remotion/player';
import { Reveal, revealDuration, FPS, type RevealFinding } from './compositions';

export type PlayerKind = 'reveal';

export type PlayerHostProps = {
  kind: PlayerKind;
  findings: RevealFinding[];
  diagnosisId: string;
  /** Fired when the clip's own play-through completes (Player 'ended' event). */
  onEnded?: () => void;
};

// The overlay fills #war-room-root, sits above the canvas, and never eats
// pointer events (the terminal below stays interactive).
const overlayStyle: React.CSSProperties = {
  position: 'absolute',
  inset: 0,
  zIndex: 3,
  pointerEvents: 'none',
};

const playerStyle: React.CSSProperties = {
  width: '100%',
  height: '100%',
  background: 'transparent',
};

export default function PlayerHost({ kind, findings, diagnosisId, onEnded }: PlayerHostProps) {
  const ref = useRef<PlayerRef>(null);

  const durationInFrames = useMemo(
    () => revealDuration(findings.length),
    [findings.length],
  );

  // Memoise inputProps so <Player> doesn't see a new object every render.
  const inputProps = useMemo(
    () => ({ findings, diagnosisId }),
    [findings, diagnosisId],
  );

  // Remotion exposes completion via the 'ended' event on the PlayerRef, not a
  // prop — subscribe so the parent can retire the overlay when the clip ends.
  useEffect(() => {
    const p = ref.current;
    if (!p || !onEnded) return;
    const handler = () => onEnded();
    p.addEventListener('ended', handler);
    return () => p.removeEventListener('ended', handler);
  }, [onEnded]);

  return (
    <div style={overlayStyle} aria-hidden="false" data-player={kind}>
      <Player
        ref={ref}
        component={Reveal as React.FC}
        inputProps={inputProps}
        durationInFrames={durationInFrames}
        compositionWidth={1280}
        compositionHeight={720}
        fps={FPS}
        style={playerStyle}
        autoPlay
        loop={false}
        controls={false}
        clickToPlay={false}
        doubleClickToFullscreen={false}
        showVolumeControls={false}
        spaceKeyToPlayOrPause={false}
        numberOfSharedAudioTags={0}
        acknowledgeRemotionLicense
      />
    </div>
  );
}
