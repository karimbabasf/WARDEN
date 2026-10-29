// BrickyardPanel.tsx: the right-hand readout for one bench. What is being built, how
// far along it is, the instruction page open on the bench, and the build log, where
// every row is a real tool call and a click finds the brick it placed.

import { useEffect, useMemo, useRef, useState } from 'react';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { harnessColor } from '@/viz/shared/theme/harnessColors';
import type { BenchRig, BenchStats, BrickPick } from './bench';
import type { BuildStep } from './buildLog';
import { nameOfAgent } from './names';
import { StructurePreview } from './StructurePreview';

const PREVIEW_MS = 1500;

const ACTION_LABEL: Record<string, string> = {
  place: 'placed',
  pull: 'pulled',
  browse: 'read',
  page: 'page',
  spawn: 'helper',
  ask: 'asked',
};

function clock(ts: string): string {
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

/** Mirrors the bench's open right-hand page into the panel. */
function PageMirror({ rig, page }: { rig: BenchRig; page: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  // Keyed on the canvas itself: the page renders after the step changes, so keying on
  // the step alone left the mirror one page behind the book.
  const src = rig.pageCanvas();
  useEffect(() => {
    const dst = ref.current;
    if (!src || !dst) return;
    dst.width = src.width;
    dst.height = src.height;
    dst.getContext('2d')?.drawImage(src, 0, 0);
  }, [src]);
  return <canvas ref={ref} className="by-page" aria-label={`Instruction page ${page + 1}`} />;
}

/** A finished set off the shelf, on a big turntable, with who built it. */
function ShelfCard({ rig, pick, crew, onBack }: { rig: BenchRig; pick: BrickPick; crew: RadarAgent[]; onBack: () => void }) {
  const info = rig.shelfInfo(pick.setIndex);
  const count = info?.bricks ?? 0;
  const all = useMemo(() => new Array(count).fill(true), [count]);
  if (!info) return null;
  const names = new Map(crew.map((a) => [a.id, nameOfAgent(a)]));
  return (
    <section className="by-shelfcard" aria-label="Finished set">
      <div className="by-shelfcard-head">
        <span className="by-shelfcard-kind">On the shelf</span>
        <button type="button" className="by-back" onClick={onBack}>
          Back
        </button>
      </div>
      <div className="by-set">
        <span className="by-set-num">{info.entry.number}</span>
        <span className="by-set-name">{info.entry.name}</span>
        {info.entry.year ? <span className="by-set-year">{info.entry.year}</span> : null}
      </div>
      <StructurePreview file={info.file} placed={all} finishedOnly />
      <div className="by-shelf">
        {info.bricks} bricks, built by{' '}
        {info.builders
          .slice(0, 3)
          .map((b) => `${names.get(b.actor) ?? 'a helper'} (${b.bricks})`)
          .join(', ')}
      </div>
    </section>
  );
}

function StepCard({ pick }: { pick: BrickPick }) {
  const s = pick.placed;
  return (
    <section className="by-card" aria-label="Selected brick">
      <div className="by-card-head">
        <span className="by-card-kind">brick {pick.brick + 1}</span>
        <span className="by-card-set">
          {pick.set.number} {pick.set.name}
        </span>
      </div>
      {s ? (
        <>
          <div className="by-card-tool">{s.tool}</div>
          <div className="by-card-label">{s.label}</div>
          {s.target ? <div className="by-card-target">{s.target}</div> : null}
          <div className="by-card-meta">
            placed {clock(s.ts)} · call #{s.seq}
          </div>
        </>
      ) : (
        <div className="by-card-label">Not placed yet.</div>
      )}
      {pick.pulled ? (
        <div className="by-card-pulled">
          pulled {clock(pick.pulled.ts)}: {pick.pulled.label}
        </div>
      ) : null}
    </section>
  );
}

export function BrickyardPanel({
  rig,
  lead,
  helpers,
  stats,
  pick,
  onPickStep,
  onClose,
}: {
  rig: BenchRig;
  lead: RadarAgent;
  helpers: RadarAgent[];
  stats: BenchStats | null;
  pick: BrickPick | null;
  onPickStep: (p: BrickPick | null) => void;
  onClose: () => void;
}) {
  const hc = harnessColor(lead.harness);
  const [modelView, setModelView] = useState(false);
  const [scrub, setScrub] = useState<number | null>(null);
  const placed = stats?.standing ?? 0;

  useEffect(() => {
    rig.setGhost(modelView);
    if (!modelView) {
      setScrub(null);
      rig.scrub(null);
    }
    return () => {
      rig.setGhost(false);
      rig.scrub(null);
    };
  }, [rig, modelView]);
  useEffect(() => {
    if (modelView) rig.scrub(scrub);
  }, [rig, modelView, scrub, placed]);

  const choose = (actor: string, step: BuildStep) => {
    const p = rig.brickForStep(actor, step.seq);
    if (p) {
      rig.highlight(p.brick);
      onPickStep(p);
    }
  };

  // The preview rebakes the whole set, so it is throttled: at most one rebake every
  // PREVIEW_MS however fast bricks land, and the first one as soon as there is a set.
  const [preview, setPreview] = useState(() => rig.previewState());
  const lastBake = useRef(0);
  useEffect(() => {
    const wait = preview === null ? 0 : Math.max(0, lastBake.current + PREVIEW_MS - performance.now());
    const t = window.setTimeout(() => {
      lastBake.current = performance.now();
      setPreview(rig.previewState());
    }, wait);
    return () => window.clearTimeout(t);
    // `preview` is read for the first-bake case only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rig, stats?.standing, stats?.setIndex]);

  const pct = stats ? Math.round((stats.standing / Math.max(1, stats.bricks)) * 100) : 0;

  return (
    <div className="by-panel" style={{ ['--harness' as string]: hc.hue }}>
      <header className="by-head">
        <div className="by-head-id">
          <span className="by-glyph" aria-label={hc.label}>
            {hc.glyph}
          </span>
          <div>
            <h2 className="by-title">{nameOfAgent(lead)}</h2>
            <div className={`by-status is-${lead.status}`}>{lead.status}</div>
          </div>
        </div>
        <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close bench">
          ×
        </button>
      </header>

      {stats ? (
        <section className="by-build" aria-label="Build progress">
          <div className="by-set">
            <span className="by-set-num">{stats.set.number}</span>
            <span className="by-set-name">{stats.set.name}</span>
            {stats.set.year ? <span className="by-set-year">{stats.set.year}</span> : null}
          </div>
          <div className="by-meter" role="meter" aria-valuemin={0} aria-valuemax={stats.bricks} aria-valuenow={stats.standing}>
            <span style={{ width: `${pct}%` }} />
          </div>
          <dl className="by-figures">
            <div>
              <dt>bricks</dt>
              <dd>
                {stats.standing}
                <small>/{stats.bricks}</small>
              </dd>
            </div>
            <div>
              <dt>step</dt>
              <dd>
                {Math.min(stats.page + 1, stats.steps)}
                <small>/{stats.steps}</small>
              </dd>
            </div>
            <div>
              <dt>placed</dt>
              <dd>{stats.placedTotal}</dd>
            </div>
            <div>
              <dt>pulled</dt>
              <dd>{stats.pulledTotal}</dd>
            </div>
          </dl>
          {stats.shelf.length > 0 ? <div className="by-shelf">shelf: {stats.shelf.map((s) => s.name).join(', ')}</div> : null}
        </section>
      ) : (
        <section className="by-build by-loading">Opening the box...</section>
      )}

      {pick?.shelved ? (
        <ShelfCard rig={rig} pick={pick} crew={[lead, ...helpers]} onBack={() => onPickStep(null)} />
      ) : preview ? (
        <StructurePreview file={preview.file} placed={preview.placed} />
      ) : null}

      {helpers.length > 0 ? (
        <section className="by-helpers" aria-label="Helpers">
          {helpers.map((h) => (
            <span key={h.id} className={`by-helper is-${h.status}`}>
              {nameOfAgent(h)}
            </span>
          ))}
        </section>
      ) : null}

      {stats ? <PageMirror rig={rig} page={stats.page} /> : null}

      <section className="by-model" aria-label="Model view">
        <button
          type="button"
          className={`by-toggle${modelView ? ' is-on' : ''}`}
          aria-pressed={modelView}
          onClick={() => setModelView((v) => !v)}
        >
          {modelView ? 'Hide bricks to come' : 'Show bricks to come'}
        </button>
        {modelView && placed > 0 ? (
          <label className="by-scrub">
            <span>replay</span>
            <input type="range" min={0} max={placed} value={scrub ?? placed} onChange={(e) => setScrub(Number(e.target.value))} />
            <span className="by-scrub-n">{scrub ?? placed}</span>
          </label>
        ) : null}
      </section>

      {pick ? <StepCard pick={pick} /> : null}

      <section className="by-log" aria-label="Build log">
        <h3 className="by-log-title">Build log</h3>
        {stats && stats.recent.length > 0 ? (
          <ol>
            {stats.recent.map(({ actor, step, kind }) => (
              <li key={`${actor}:${step.seq}`} className={`by-row is-${kind}`}>
                <button type="button" disabled={kind !== 'place'} onClick={() => choose(actor, step)}>
                  <span className="by-row-kind">{ACTION_LABEL[kind] ?? kind}</span>
                  <span className="by-row-label">{step.label || step.tool}</span>
                  <span className="by-row-time">{clock(step.ts)}</span>
                </button>
              </li>
            ))}
          </ol>
        ) : (
          <p className="by-empty">No tool calls yet. The builder is reading the book.</p>
        )}
      </section>
    </div>
  );
}
