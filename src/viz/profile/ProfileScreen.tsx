// ProfileScreen.tsx — the full-page DOSSIER ("Profile by Proof").
//
// A forensic operator dossier: who you are as an operator, and what you're
// fixing. Composed top→bottom (spec §4.3) as a real narrative, not a wall of
// identical boxes:
//
//   Header ─▶ ① THE SCORE (hero) ─▶ ② WHAT YOU'RE BREAKING (habits) ─▶
//   ③ WHERE YOU LOSE (leaks) ─▶ ④ WHO YOU ARE (dimensions) ─▶
//   ⑤ YOUR RANGE (archetypes + heatmap) ─▶ ⑥ TRAJECTORY
//
// Data flow (the honest seam), unchanged from the backend's contract:
//   open / window change ─▶ get_efficiency_score + get_activity_heatmap (fast,
//                            deterministic) AND get_profile (may be null).
//   get_profile === null  ─▶ COLD-START: an inviting "generate your dossier"
//                            moment. build_profile() runs the pipeline, emitting
//                            `dossier_progress` we render as an anticipatory
//                            staged reveal (not a raw <ol> of stage strings).
//   detector_only         ─▶ a one-line explainer (what it means + how to
//                            upgrade), not a bare badge.
//
// Every number traces to a real field in types.ts — no UI invention. All visual
// styling lives in the scoped ./dossier.css (disjoint from the war-room
// style.css); this file composes and wires.

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  PROFILE_WINDOWS,
  WINDOW_LABELS,
  type ActivityCell,
  type DossierProgress,
  type EfficiencyScore,
  type Profile,
  type ProfileWindow,
} from './types';
import './dossier.css';
import { ScoreHero } from './ScoreHero';
import { BreakingSection } from './BreakingSection';
import { LeaksSection } from './LeaksSection';
import { DimensionsSection } from './DimensionsSection';
import { RangeSection } from './RangeSection';
import { TrajectorySection } from './TrajectorySection';

// The build pipeline's stages, in the order they run — used to render an
// anticipatory reveal (a stage is 'done' once seen, the next is 'active').
// Mirrors src-tauri/src/dossier/build.rs emit() calls.
const BUILD_STAGES: { key: string; label: string }[] = [
  { key: 'aggregate', label: 'Reading your sessions' },
  { key: 'score', label: 'Scoring efficiency' },
  { key: 'summarize', label: 'Distilling evidence' },
  { key: 'synthesize', label: 'Writing your profile' },
  { key: 'persist', label: 'Sealing the dossier' },
];

// ── header pieces ─────────────────────────────────────────────────────────────
function WindowToggle({ value, onChange }: { value: ProfileWindow; onChange: (w: ProfileWindow) => void }) {
  return (
    <div className="dossier__windows" role="tablist" aria-label="time window">
      {PROFILE_WINDOWS.map((w) => (
        <button
          key={w}
          role="tab"
          type="button"
          aria-selected={w === value}
          className="dossier__win"
          onClick={() => onChange(w)}
        >
          {WINDOW_LABELS[w]}
        </button>
      ))}
    </div>
  );
}

function DetectorExplainer() {
  return (
    <div className="dossier__detector" role="note">
      <span className="g" aria-hidden>
        ◈
      </span>
      <span>
        Deterministic read — scored from detectors alone, no narrative model. Set{' '}
        <code>WARDEN_BRAIN_API_KEY</code> to have GLM synthesize the prose profile from the same evidence.
      </span>
    </div>
  );
}

// ── the anticipatory build reveal (cold-start) ────────────────────────────────
function BuildProgress({ seen }: { seen: Set<string> }) {
  // The first stage not yet seen is "active"; earlier ones are "done".
  const activeIdx = BUILD_STAGES.findIndex((s) => !seen.has(s.key));
  return (
    <div className="build">
      <div className="build__title">Building your dossier…</div>
      <div className="build__stages">
        {BUILD_STAGES.map((s, i) => {
          const done = seen.has(s.key) || (activeIdx >= 0 && i < activeIdx);
          const active = i === activeIdx;
          const cls = done ? 'bstage bstage--done' : active ? 'bstage bstage--active' : 'bstage';
          return (
            <div key={s.key} className={cls}>
              <span className="bstage__dot" aria-hidden />
              <span className="bstage__label">{s.label}</span>
              {done ? (
                <span className="bstage__check" aria-hidden>
                  ✓
                </span>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function ColdStart({
  win,
  building,
  seen,
  onBuild,
  disabled,
}: {
  win: ProfileWindow;
  building: boolean;
  seen: Set<string>;
  onBuild: () => void;
  disabled: boolean;
}) {
  return (
    <div className="cold">
      <div className="cold__inner">
        {building ? (
          <BuildProgress seen={seen} />
        ) : (
          <>
            <div className="cold__seal" aria-hidden>
              ◈
            </div>
            <div className="cold__title">No dossier yet for {WINDOW_LABELS[win]}</div>
            <p className="cold__sub">
              WARDEN reads every session in this window, scores how you drive your agents, and writes an
              evidence-cited profile of who you are as an operator. It takes a moment.
            </p>
            <button type="button" className="cold__cta" onClick={onBuild} disabled={disabled}>
              Generate your dossier
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// ── skeleton loader (calm placeholder, not a debug line) ──────────────────────
function Skeleton() {
  return (
    <div className="skeleton" aria-hidden>
      <div className="sk sk--hero" />
      <div className="sk sk--w60" />
      <div className="sk sk--w40" />
      <div className="sk" />
      <div className="sk sk--w60" />
    </div>
  );
}

// ── the screen ────────────────────────────────────────────────────────────────
export function ProfileScreen({ onClose }: { onClose: () => void }) {
  const [win, setWin] = useState<ProfileWindow>('all-time');
  const [eff, setEff] = useState<EfficiencyScore | null>(null);
  const [cells, setCells] = useState<ActivityCell[]>([]);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [loading, setLoading] = useState(true);
  const [building, setBuilding] = useState(false);
  const [seen, setSeen] = useState<Set<string>>(new Set());
  const [err, setErr] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);

  // Guard against a stale window's response landing after a faster newer one.
  const reqSeq = useRef(0);

  const load = useCallback(async (w: ProfileWindow) => {
    const seq = ++reqSeq.current;
    setLoading(true);
    setErr(null);
    setOffline(false);
    setProfile(null);
    setEff(null);
    setCells([]);
    try {
      const [effRes, cellRes, profRes] = await Promise.allSettled([
        invoke<EfficiencyScore>('get_efficiency_score', { window: w }),
        invoke<ActivityCell[]>('get_activity_heatmap', { window: w }),
        invoke<Profile | null>('get_profile', { window: w }),
      ]);
      if (seq !== reqSeq.current) return; // a newer window won; drop this result
      if (effRes.status === 'fulfilled') setEff(effRes.value);
      if (cellRes.status === 'fulfilled' && Array.isArray(cellRes.value)) setCells(cellRes.value);
      if (profRes.status === 'fulfilled') setProfile(profRes.value ?? null);

      // Everything rejected → we're off the Tauri runtime (browser preview).
      if (effRes.status === 'rejected' && cellRes.status === 'rejected' && profRes.status === 'rejected') {
        setOffline(true);
      }
    } finally {
      if (seq === reqSeq.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(win);
  }, [win, load]);

  // Stream build progress while a build is in flight → an ordered "seen" set the
  // reveal derives its done/active states from.
  useEffect(() => {
    if (!building) return;
    let un: undefined | (() => void);
    let cancelled = false;
    listen<DossierProgress>('dossier_progress', (e) => {
      const stage = e.payload?.stage;
      if (typeof stage === 'string') setSeen((prev) => new Set(prev).add(stage));
    })
      .then((f) => {
        if (cancelled) f();
        else un = f;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      un?.();
    };
  }, [building]);

  const onBuild = useCallback(async () => {
    setBuilding(true);
    setSeen(new Set());
    setErr(null);
    const w = win;
    try {
      const p = await invoke<Profile>('build_profile', { window: w });
      if (w === win) setProfile(p);
      void load(w); // refresh deterministic panels (efficiency may have recomputed)
    } catch (e) {
      setErr(`Couldn't build the dossier: ${String(e)}`);
    } finally {
      setBuilding(false);
    }
  }, [win, load]);

  // Prefer the built profile's embedded efficiency copy if the standalone fetch
  // lagged/failed, so THE SCORE never blanks while the profile has the numbers.
  const effForHero = eff ?? profile?.efficiency ?? null;

  return (
    <div className="dossier">
      <div className="dossier__inner">
        <header className="dossier__header">
          <div className="dossier__wordmark">
            <div className="dossier__title">
              DOSSIER <b>·</b> Profile by Proof
            </div>
            <div className="dossier__operator">
              {profile ? `generated ${profile.generated_at} · ${profile.session_count} sessions` : 'operator profile'}
            </div>
          </div>
          <div className="dossier__spacer" />
          <WindowToggle value={win} onChange={setWin} />
          <button type="button" className="dossier__close" onClick={onClose} aria-label="Close dossier">
            ✕
          </button>
        </header>

        {profile?.detector_only ? <DetectorExplainer /> : null}

        {offline ? (
          <div className="dossier__errline">
            Backend unavailable — this is a browser preview. Run the app (`pnpm tauri dev`) for live data.
          </div>
        ) : null}
        {err ? <div className="dossier__errline">{err}</div> : null}

        {/* Loading: a calm skeleton, not a debug list. */}
        {loading && !profile ? (
          <Skeleton />
        ) : profile ? (
          <>
            <ScoreHero eff={effForHero} />
            <BreakingSection window={win} />
            <LeaksSection leaks={profile.ranked_leaks} />
            <DimensionsSection dimensions={profile.dimensions} />
            <RangeSection archetypes={profile.archetypes} cells={cells} />
            <TrajectorySection trajectory={profile.trajectory} />
          </>
        ) : (
          // Cold-start: no cached profile for this window. Show the fast
          // deterministic score if we have it, then the generate moment.
          <>
            {effForHero ? <ScoreHero eff={effForHero} /> : null}
            <ColdStart win={win} building={building} seen={seen} onBuild={onBuild} disabled={offline} />
          </>
        )}
      </div>
    </div>
  );
}

export default ProfileScreen;
