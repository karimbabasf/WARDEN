// TrajectorySection.tsx — §⑥ TRAJECTORY. How the operator's traits trend over
// time: a sparkline + a direction glyph per trait. The important part is the
// GRACEFUL sparse state — the by-proof gate marks a trait 'insufficient' when it
// has fewer than 3 buckets × 3 samples (common in demo data), and instead of a
// broken-looking empty muted row we show an honest "still gathering" line with a
// progress hint.
//
// Honest-viz: direction and points are the backend's real values; 'insufficient'
// is rendered as what it is (not enough history), never faked into a trend.

import type { TraitPoint, TraitTrend } from './types';
import { familyLabel } from './verdict';

const DIRECTION: Record<TraitTrend['direction'], { glyph: string; color: string; word: string }> = {
  improving: { glyph: '▲', color: 'var(--green)', word: 'improving' },
  regressing: { glyph: '▼', color: 'var(--red)', word: 'regressing' },
  plateaued: { glyph: '▬', color: 'var(--warn)', word: 'holding steady' },
  insufficient: { glyph: '·', color: 'var(--ink-faint)', word: 'gathering' },
};

// Buckets needed before the backend can call a direction (by-proof: 3 buckets).
const BUCKETS_TARGET = 3;

function Sparkline({ points }: { points: TraitPoint[] }) {
  const vals = points.map((p) => p.value).filter((v) => Number.isFinite(v));
  if (vals.length < 2) return <span className="trend__conf">{vals.length} point(s)</span>;
  const lo = Math.min(...vals);
  const hi = Math.max(...vals);
  const span = hi - lo || 1;
  const w = 120;
  const h = 26;
  const step = w / (vals.length - 1);
  const d = vals
    .map((v, i) => {
      const x = i * step;
      const y = h - ((v - lo) / span) * h;
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');
  const last = vals[vals.length - 1];
  const lastY = h - ((last - lo) / span) * h;
  return (
    <svg className="trend__spark" width={w} height={h} aria-hidden>
      <path d={d} fill="none" stroke="var(--green)" strokeWidth={1.5} />
      <circle cx={w} cy={lastY} r={2.2} fill="var(--acid)" />
    </svg>
  );
}

function TrendRow({ t }: { t: TraitTrend }) {
  const dir = DIRECTION[t.direction] ?? DIRECTION.insufficient;
  const label = familyLabel(t.trait_key) || t.trait_key;

  // Graceful sparse state: not a broken empty row, but an honest "still gathering".
  if (t.direction === 'insufficient') {
    const have = Math.min(BUCKETS_TARGET, t.points.length);
    return (
      <div className="trend trend--sparse">
        <div className="trend__gather">
          <span className="trait">{label}</span>
          <span className="msg">still gathering — needs {BUCKETS_TARGET}+ weeks of history; here's what we have.</span>
          <span className="trend__dots" aria-label={`${have} of ${BUCKETS_TARGET} periods collected`}>
            {Array.from({ length: BUCKETS_TARGET }, (_, i) => (
              <span key={i} className={`trend__dot ${i < have ? 'trend__dot--on' : ''}`} />
            ))}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="trend">
      <div className="trend__trait">{label}</div>
      <Sparkline points={t.points} />
      <div className="trend__dir" style={{ color: dir.color }}>
        <span className="g" aria-hidden>
          {dir.glyph}
        </span>
        <span>{dir.word}</span>
        <span className="trend__conf">· {Math.round((Number.isFinite(t.confidence) ? t.confidence : 0) * 100)}%</span>
      </div>
    </div>
  );
}

export function TrajectorySection({ trajectory }: { trajectory: TraitTrend[] }) {
  const traj = trajectory ?? [];
  return (
    <section className="dossier__section" aria-label="Trajectory">
      <div className="dossier__eyebrow">
        <span className="dossier__num">⑥</span>
        <h2 className="dossier__h">Trajectory</h2>
        <span className="dossier__hint">which way you're trending</span>
      </div>
      {traj.length === 0 ? (
        <div className="dossier__empty">
          Trajectory needs a few weeks of history to plot. WARDEN starts charting it as your sessions accumulate.
        </div>
      ) : (
        <div className="traj">
          {traj.map((t) => (
            <TrendRow key={t.trait_key} t={t} />
          ))}
        </div>
      )}
    </section>
  );
}

export default TrajectorySection;
