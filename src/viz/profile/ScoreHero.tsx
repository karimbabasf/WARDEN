// ScoreHero.tsx — §① THE SCORE. The dossier's one bold moment: the efficiency
// headline (0–100) plotted as a ring gauge, captioned by the generated verdict
// line, with the 7 rubric families as a compact secondary readout beside it.
//
// Honest-viz: the number is the real `headline * 100`; the ring arc and the
// family bars map to real sub_scores; the verdict is a deterministic reading
// (verdict.ts), never an LLM sentence. Motion (count-up + arc sweep) is gated on
// prefers-reduced-motion.

import { useEffect, useRef, useState } from 'react';
import type { EfficiencyScore } from './types';
import { familyLabel, strongestWeakest, verdictLine } from './verdict';

const PREFERS_REDUCED =
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Count 0 → target over ~1s, easeOutExpo. Instant under reduced-motion. */
function useCountUp(target: number, run: boolean): number {
  const [v, setV] = useState(PREFERS_REDUCED ? target : 0);
  const raf = useRef<number | null>(null);
  useEffect(() => {
    if (!run || PREFERS_REDUCED) {
      setV(target);
      return;
    }
    const start = performance.now();
    const dur = 1000;
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / dur);
      const eased = 1 - Math.pow(2, -10 * t);
      setV(Math.round(target * eased));
      if (t < 1) raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);
    return () => {
      if (raf.current != null) cancelAnimationFrame(raf.current);
    };
  }, [target, run]);
  return v;
}

const R = 52; // ring radius in the 120-box viewBox
const CIRC = 2 * Math.PI * R;

export function ScoreHero({ eff }: { eff: EfficiencyScore | null }) {
  const headline = eff && Number.isFinite(eff.headline) ? Math.max(0, Math.min(1, eff.headline)) : 0;
  const score = Math.round(headline * 100);
  const shown = useCountUp(score, !!eff);
  const arc = CIRC * (1 - headline);

  const ranked = eff ? strongestWeakest(eff.families) : null;
  const strongKey = ranked?.strongest.key;
  const weakKey = ranked?.weakest.key;

  // Families sorted by weight desc — the heaviest rubric families first.
  const families = eff ? [...eff.families].sort((a, b) => b.weight - a.weight) : [];

  const verdict = verdictLine(eff);

  return (
    <section className="hero" aria-label="Efficiency score">
      <div className="hero__grid">
        {/* the gauge — the eye's first anchor */}
        <div className="hero__gauge">
          <svg className="hero__ring" viewBox="0 0 120 120" aria-hidden>
            <circle className="hero__ring-track" cx="60" cy="60" r={R} />
            <circle
              className="hero__ring-arc"
              cx="60"
              cy="60"
              r={R}
              strokeDasharray={CIRC}
              strokeDashoffset={eff ? arc : CIRC}
            />
          </svg>
          <div className="hero__readout">
            <div className="hero__score" aria-hidden>
              {eff ? shown : '—'}
            </div>
            <div className="hero__outof">/ 100 EFFICIENCY</div>
          </div>
        </div>

        {/* verdict + compact family breakdown */}
        <div className="hero__side">
          <div className="hero__eyebrow">
            <b>①</b> THE SCORE
          </div>
          <p className="hero__verdict">{renderVerdict(verdict)}</p>
          <div className="hero__meta">
            {eff
              ? `rubric ${eff.rubric_version} · ${eff.session_count} session${eff.session_count === 1 ? '' : 's'} scored`
              : 'no scored sessions in this window yet'}
          </div>

          <div className="hero__families">
            {families.length === 0 ? (
              <div className="dossier__empty">The rubric fills in as WARDEN scores more of your sessions.</div>
            ) : (
              families.map((f) => {
                const pct = Math.round((Number.isFinite(f.sub_score) ? Math.max(0, Math.min(1, f.sub_score)) : 0) * 100);
                const cls =
                  f.key === strongKey ? 'famrow famrow--top' : f.key === weakKey ? 'famrow famrow--low' : 'famrow';
                return (
                  <div key={f.key} className={cls}>
                    <div className="famrow__label" title={familyLabel(f.key)}>
                      {familyLabel(f.key)}
                    </div>
                    <div className="famrow__track">
                      <div
                        className="famrow__fill"
                        style={{ transform: `scaleX(${eff ? pct / 100 : 0})` }}
                      />
                    </div>
                    <div className="famrow__pct">{pct}</div>
                  </div>
                );
              })
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

// Highlight the strongest/weakest phrases the verdict names, without re-deriving
// them: colorize the two known family segments if present, else plain text.
function renderVerdict(line: string) {
  const m = line.match(/^Strongest at (.+?) — you lose most to (.+?)\.$/);
  if (!m) return line;
  return (
    <>
      Strongest at <span className="strong">{m[1]}</span> — you lose most to <span className="lose">{m[2]}</span>.
    </>
  );
}

export default ScoreHero;
