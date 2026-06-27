// Reveal.tsx — the cinematic diagnosis "slam-in" (spec §6.5 role 1).
//
// THE jaw-drop moment. Played LIVE via `@remotion/player`'s <Player> (no render
// backend) the instant `diagnosis_ready` fires, driven by the REAL ranked
// findings handed in as props — never a fabricated count. Each ranked hole
// slams in on a spring, staggered one second apart, with a severity bar that
// fills to its real 1..5 weight and a harness badge whose colour is ALWAYS
// paired with a glyph + label (color-blind a11y, matching the war-room legend).
//
// Frame-accurate & deterministic: identical props → identical frames, so the
// post-M2 build-time pre-render (scripts/render-intro.mjs era) will match what
// the user sees live to the frame.

import { AbsoluteFill, Sequence, interpolate, spring, useCurrentFrame, useVideoConfig } from 'remotion';
import { FPS, INTRO_FRAMES, revealDuration } from './timing';
import { harnessTheme } from '@/viz/shared/theme/harnessTheme';
// Phosphor language — single source in ./palette (mirrors style.css tokens).
import { BG, GREEN, ACID, AMBER, DIM, MONO } from './palette';

// Harness identity (paired colour + glyph + label) comes from the SINGLE source
// in harnessTheme.ts. That module is pure — no React/Remotion import — so it
// stays inside this lazy reveal chunk without dragging anything onto the summon
// hot path.
const harnessBadge = harnessTheme;

export type RevealFinding = {
  title: string;
  severity: number; // real 1..5 severity weight
  harness: string; // snake_case: claude_code | codex | unknown
  est_cost?: number; // optional est token cost (omitted when unknown — honest)
};

export type RevealProps = {
  findings: RevealFinding[];
  diagnosisId: string;
};

// One ranked hole row — slams in from the left on a spring, severity bar wipes
// to its real weight, harness badge fades up. `local` is the frame WITHIN this
// row's Sequence so every row animates identically regardless of rank.
function HoleRow({ finding, rank }: { finding: RevealFinding; rank: number }) {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const badge = harnessBadge(finding.harness);
  const sev = Math.max(0, Math.min(5, finding.severity));

  // Spring slam-in: overshoot for punch, then settle.
  const slam = spring({ frame, fps, config: { damping: 12, stiffness: 180, mass: 0.7 } });
  const x = interpolate(slam, [0, 1], [-90, 0]);
  const opacity = interpolate(frame, [0, 6], [0, 1], { extrapolateRight: 'clamp' });

  // Severity bar wipes after the row has landed (frames 8..26).
  const barPct = interpolate(frame, [8, 26], [0, (sev / 5) * 100], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
  });
  // Heavier holes pulse hotter — weight on severity.
  const glow = 6 + sev * 4 + Math.sin(frame / 6) * (sev >= 4 ? 3 : 1);
  const barColor = sev >= 4 ? AMBER : sev >= 3 ? ACID : GREEN;

  return (
    <div
      style={{
        transform: `translateX(${x}px)`,
        opacity,
        display: 'grid',
        gridTemplateColumns: '64px 1fr 150px',
        alignItems: 'center',
        gap: 18,
        padding: '14px 18px',
        margin: '10px 0',
        border: `1px solid rgba(118,255,157,0.26)`,
        background: 'rgba(6,42,21,0.55)',
        boxShadow: `inset 0 0 26px rgba(118,255,157,0.06), 0 0 ${glow}px rgba(184,255,107,0.18)`,
      }}
    >
      {/* rank chip */}
      <div style={{ color: AMBER, fontSize: 30, fontWeight: 800, textShadow: `0 0 ${glow}px ${AMBER}` }}>
        #{rank + 1}
      </div>

      {/* title + severity bar */}
      <div style={{ minWidth: 0 }}>
        <div
          style={{
            color: ACID,
            fontSize: 22,
            letterSpacing: '0.04em',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            textShadow: '0 0 10px rgba(184,255,107,0.35)',
          }}
        >
          {finding.title}
        </div>
        <div
          style={{
            marginTop: 8,
            height: 12,
            border: `1px solid ${DIM}`,
            background: `linear-gradient(90deg, ${barColor} ${barPct}%, transparent ${barPct}%)`,
            boxShadow: `0 0 12px ${barColor}55`,
          }}
        />
        {finding.est_cost != null && (
          <div style={{ marginTop: 6, color: '#9dffc0', fontSize: 12, opacity: 0.85 }}>
            ~{Math.round(finding.est_cost).toLocaleString()} tokens
          </div>
        )}
      </div>

      {/* harness badge — colour ALWAYS with glyph + label */}
      <div style={{ justifySelf: 'end', display: 'inline-flex', alignItems: 'center', gap: 8 }}>
        <span style={{ color: badge.color, fontSize: 20, filter: `drop-shadow(0 0 6px ${badge.color})` }}>
          {badge.glyph}
        </span>
        <span style={{ color: badge.color, fontSize: 14, letterSpacing: '0.14em', textTransform: 'uppercase' }}>
          {badge.label}
        </span>
        <span style={{ color: DIM, fontSize: 13 }}>· sev {sev}</span>
      </div>
    </div>
  );
}

// Animated scanline sweep + grain overlay — the green-phosphor CRT language.
function Scanlines({ frame }: { frame: number }) {
  const sweepY = interpolate(frame % 90, [0, 90], [-10, 110]);
  return (
    <AbsoluteFill style={{ pointerEvents: 'none', mixBlendMode: 'screen' }}>
      <AbsoluteFill
        style={{
          background: 'repeating-linear-gradient(0deg, rgba(118,255,157,0.06) 0 1px, transparent 1px 4px)',
          opacity: 0.5,
        }}
      />
      <AbsoluteFill
        style={{
          background: `linear-gradient(180deg, transparent ${sweepY - 6}%, rgba(184,255,107,0.10) ${sweepY}%, transparent ${sweepY + 6}%)`,
        }}
      />
    </AbsoluteFill>
  );
}

/**
 * The full reveal. `durationInFrames` is `revealDuration(findings.length)` so the
 * <Player> plays exactly long enough for every real hole to land plus the intro
 * runway. Honest: renders ONLY the findings passed in (no count is invented).
 */
export function Reveal({ findings, diagnosisId }: RevealProps) {
  const frame = useCurrentFrame();
  const list = Array.isArray(findings) ? findings : [];

  // Title slam: scale-overshoot + amber underline wipe in the intro runway.
  const titleScale = interpolate(frame, [0, 10, 18], [0.6, 1.08, 1], { extrapolateRight: 'clamp' });
  const titleOpacity = interpolate(frame, [0, 8], [0, 1], { extrapolateRight: 'clamp' });
  const underline = interpolate(frame, [12, INTRO_FRAMES], [0, 100], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });
  const verdictGlow = 18 + Math.sin(frame / 8) * 8;

  const harnessCount = new Set(list.map(f => f.harness)).size;

  return (
    <AbsoluteFill
      style={{
        background: `radial-gradient(circle at 50% 0%, #063518 0, ${BG} 46%, #000 100%)`,
        fontFamily: MONO,
        color: GREEN,
        padding: '54px 60px',
      }}
    >
      {/* header */}
      <div style={{ transform: `scale(${titleScale})`, transformOrigin: 'left center', opacity: titleOpacity }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 16 }}>
          <span style={{ color: ACID, fontSize: 40, fontWeight: 800, letterSpacing: '0.18em', textShadow: `0 0 ${verdictGlow}px ${ACID}` }}>
            WARDEN
          </span>
          <span style={{ color: AMBER, fontSize: 26, letterSpacing: '0.22em', textShadow: `0 0 ${verdictGlow}px ${AMBER}` }}>
            VERIFIED DIAGNOSIS
          </span>
        </div>
        <div style={{ height: 3, marginTop: 10, width: `${underline}%`, background: AMBER, boxShadow: `0 0 14px ${AMBER}` }} />
        <div style={{ marginTop: 10, color: '#70a980', fontSize: 14, letterSpacing: '0.14em' }}>
          {list.length} HOLE{list.length === 1 ? '' : 'S'} · {harnessCount} HARNESS{harnessCount === 1 ? '' : 'ES'} · {diagnosisId.slice(0, 12)}
        </div>
      </div>

      {/* ranked holes — each in its own Sequence, staggered one second apart */}
      <div style={{ marginTop: 26 }}>
        {list.map((f, i) => (
          <Sequence key={i} from={INTRO_FRAMES + i * FPS} durationInFrames={Math.max(1, revealDuration(list.length) - (INTRO_FRAMES + i * FPS))} layout="none">
            <HoleRow finding={f} rank={i} />
          </Sequence>
        ))}
        {list.length === 0 && (
          <div style={{ marginTop: 30, color: '#70a980', fontSize: 18, opacity: interpolate(frame, [10, 26], [0, 1], { extrapolateRight: 'clamp' }) }}>
            No confirmed holes this run — the war room held.
          </div>
        )}
      </div>

      <Scanlines frame={frame} />
    </AbsoluteFill>
  );
}

export default Reveal;
