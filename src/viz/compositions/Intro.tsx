// Intro.tsx — the branded boot clip (spec §6.5 role 2).
//
// PRE-RENDER RESOLUTION (risk R-Rem): true build-time pre-render needs
// `@remotion/renderer`, which is explicitly post-M2. So for M2 the intro is
// played LIVE via the same <Player> as the reveal on FIRST summon — playing the
// composition live needs no render backend and is equivalent to the user. The
// post-M2 path (scripts/render-intro.mjs) will pre-render THIS SAME component to
// an mp4/webm for an instant-frame boot; until then, live playback is the boot.
//
// Pure phosphor brand beat: the WARDEN sigil powers on (scanline wipe + glow
// bloom), a tagline types under it, then it settles. Deterministic & short.

import { AbsoluteFill, interpolate, spring, useCurrentFrame, useVideoConfig } from 'remotion';
import { FPS } from './timing';
import { BG, GREEN, ACID, AMBER, MONO } from './palette';

/** Intro length in frames — a tight ~2.3s brand power-on. */
export const INTRO_DURATION = Math.round(FPS * 2.3);

const TAGLINE = 'the agent that watches your agents';

export function Intro() {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Sigil powers on: spring scale-in + a brightness bloom that overshoots then
  // settles to steady phosphor.
  const pop = spring({ frame, fps, config: { damping: 11, stiffness: 150, mass: 0.8 } });
  const scale = interpolate(pop, [0, 1], [0.7, 1]);
  const bloom = interpolate(frame, [0, 8, 20], [0, 1.6, 1], { extrapolateRight: 'clamp' });
  const sigilOpacity = interpolate(frame, [0, 6], [0, 1], { extrapolateRight: 'clamp' });

  // Scanline power-on wipe sweeping down across the sigil (0..18).
  const wipe = interpolate(frame, [0, 18], [0, 100], { extrapolateRight: 'clamp' });

  // Tagline types in char-by-char after the sigil lands.
  const typed = Math.max(0, Math.floor(interpolate(frame, [22, 22 + TAGLINE.length], [0, TAGLINE.length], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
  })));
  const caretOn = frame % 16 < 8;

  // Underline charge bar fills as the boot completes.
  const charge = interpolate(frame, [10, INTRO_DURATION - 6], [0, 100], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });
  const glow = 22 * bloom + Math.sin(frame / 7) * 4;

  return (
    <AbsoluteFill
      style={{
        background: `radial-gradient(circle at 50% 45%, #063518 0, ${BG} 50%, #000 100%)`,
        fontFamily: MONO,
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
      }}
    >
      <div style={{ position: 'relative', transform: `scale(${scale})`, opacity: sigilOpacity }}>
        <div
          style={{
            color: ACID,
            fontSize: 88,
            fontWeight: 800,
            letterSpacing: '0.28em',
            textShadow: `0 0 ${glow}px ${ACID}, 0 0 ${glow * 2}px rgba(118,255,157,0.4)`,
            filter: `brightness(${0.8 + bloom * 0.5})`,
          }}
        >
          WARDEN
        </div>
        {/* power-on scanline wipe */}
        <AbsoluteFill
          style={{
            pointerEvents: 'none',
            mixBlendMode: 'screen',
            background: `linear-gradient(180deg, rgba(184,255,107,0.0) ${wipe - 8}%, rgba(184,255,107,0.35) ${wipe}%, rgba(184,255,107,0.0) ${wipe + 8}%)`,
          }}
        />
      </div>

      {/* charge underline */}
      <div style={{ marginTop: 18, height: 3, width: 420 }}>
        <div style={{ height: '100%', width: `${charge}%`, background: AMBER, boxShadow: `0 0 14px ${AMBER}` }} />
      </div>

      {/* typed tagline */}
      <div style={{ marginTop: 22, color: GREEN, fontSize: 20, letterSpacing: '0.16em', textTransform: 'uppercase', textShadow: '0 0 10px rgba(118,255,157,0.3)' }}>
        {TAGLINE.slice(0, typed)}
        <span style={{ opacity: caretOn ? 1 : 0, color: ACID }}>▌</span>
      </div>

      {/* fixed scanline grain */}
      <AbsoluteFill
        style={{
          pointerEvents: 'none',
          mixBlendMode: 'screen',
          opacity: 0.4,
          background: 'repeating-linear-gradient(0deg, rgba(118,255,157,0.05) 0 1px, transparent 1px 4px)',
        }}
      />
    </AbsoluteFill>
  );
}

export default Intro;
