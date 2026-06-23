// Recap.tsx — the shareable hole recap (spec §6.5 role 3, composition side).
//
// A compact, loop-friendly summary of the confirmed holes — the visual that
// `recorder.ts` captures to webm via MediaRecorder (the M2 export fallback),
// and that the post-M2 `@remotion/renderer` path will render headlessly. Same
// honest contract as the reveal: it summarises the REAL findings handed in.
//
// Where the Reveal is the cinematic slam-in, the Recap is the calmer "here is
// what we found" card: a tallied header + a tight ranked list that wipes in,
// designed to read in a 3–6s clip.

import { AbsoluteFill, interpolate, spring, useCurrentFrame, useVideoConfig } from 'remotion';
import { FPS } from './timing';
import type { RevealFinding } from './Reveal';
import { harnessTheme } from '../harnessTheme';
// Phosphor language — single source in ./palette (mirrors style.css tokens).
import { BG, GREEN, ACID, AMBER, DIM, MONO } from './palette';

// Harness identity from the SINGLE source (harnessTheme.ts) — pure, so it stays
// in this lazy chunk without pulling anything onto the summon hot path.
const badge = harnessTheme;

export type RecapProps = {
  findings: RevealFinding[];
  diagnosisId: string;
};

/** Recap length scales gently with finding count, floored for share-ability. */
export function recapDuration(n: number): number {
  const safe = Number.isFinite(n) ? Math.max(0, Math.min(64, Math.floor(n))) : 0;
  return Math.round(FPS * 1.0) + safe * Math.round(FPS * 0.5);
}

function RecapRow({ finding, rank }: { finding: RevealFinding; rank: number }) {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const b = badge(finding.harness);
  const sev = Math.max(0, Math.min(5, finding.severity));
  const enter = spring({ frame, fps, config: { damping: 16, stiffness: 140 } });
  const opacity = interpolate(enter, [0, 1], [0, 1]);
  const x = interpolate(enter, [0, 1], [40, 0]);
  const pct = interpolate(frame, [4, 16], [0, (sev / 5) * 100], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });
  const barColor = sev >= 4 ? AMBER : sev >= 3 ? ACID : GREEN;

  return (
    <div style={{ opacity, transform: `translateX(${x}px)`, display: 'grid', gridTemplateColumns: '40px 1fr 130px', gap: 14, alignItems: 'center', padding: '8px 0' }}>
      <span style={{ color: AMBER, fontSize: 20, fontWeight: 800 }}>#{rank + 1}</span>
      <div style={{ minWidth: 0 }}>
        <div style={{ color: ACID, fontSize: 17, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{finding.title}</div>
        <div style={{ marginTop: 5, height: 8, border: `1px solid ${DIM}`, background: `linear-gradient(90deg, ${barColor} ${pct}%, transparent ${pct}%)` }} />
      </div>
      <span style={{ justifySelf: 'end', display: 'inline-flex', alignItems: 'center', gap: 6, color: b.color, fontSize: 13, letterSpacing: '0.12em', textTransform: 'uppercase' }}>
        <span style={{ filter: `drop-shadow(0 0 5px ${b.color})` }}>{b.glyph}</span>
        {b.label}
      </span>
    </div>
  );
}

export function Recap({ findings, diagnosisId }: RecapProps) {
  const frame = useCurrentFrame();
  const list = Array.isArray(findings) ? findings : [];
  const headerOpacity = interpolate(frame, [0, 8], [0, 1], { extrapolateRight: 'clamp' });
  const worst = list.reduce((m, f) => Math.max(m, f.severity), 0);

  return (
    <AbsoluteFill style={{ background: `radial-gradient(circle at 50% 0%, #063518 0, ${BG} 48%, #000 100%)`, fontFamily: MONO, color: GREEN, padding: '40px 48px' }}>
      <div style={{ opacity: headerOpacity }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 12 }}>
          <span style={{ color: ACID, fontSize: 30, fontWeight: 800, letterSpacing: '0.2em' }}>WARDEN</span>
          <span style={{ color: '#70a980', fontSize: 16, letterSpacing: '0.16em' }}>HOLE RECAP</span>
        </div>
        <div style={{ marginTop: 6, color: AMBER, fontSize: 14, letterSpacing: '0.14em' }}>
          {list.length} HOLE{list.length === 1 ? '' : 'S'} · WORST SEV {worst} · {diagnosisId.slice(0, 12)}
        </div>
      </div>
      <div style={{ marginTop: 18 }}>
        {list.slice(0, 8).map((f, i) => (
          <RecapRow key={i} finding={f} rank={i} />
        ))}
      </div>
    </AbsoluteFill>
  );
}

export default Recap;
