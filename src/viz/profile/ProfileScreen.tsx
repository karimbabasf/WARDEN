// ProfileScreen.tsx — the full-page DOSSIER ("Profile by Proof") readout.
//
// FUNCTIONALITY-FIRST (per product owner): correct data wiring + a full-page
// layout that renders EVERYTHING the backend returns. Styling is deliberately
// minimal — existing phosphor tokens only; the cinematic pass is deferred.
//
// Data flow (the honest seam):
//   open / window change ─▶ get_efficiency_score + get_activity_heatmap (fast,
//                            deterministic) AND get_profile (may be null).
//   get_profile === null  ─▶ show "Build profile" → build_profile() runs the
//                            pipeline, emitting `dossier_progress` events that we
//                            stream into a live stage list; on resolve we render
//                            the returned Profile.
// Every number on screen traces to a real field in types.ts — no UI invention.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  PROFILE_WINDOWS,
  WINDOW_LABELS,
  type ActivityCell,
  type Claim,
  type DossierProgress,
  type EfficiencyScore,
  type EvidenceRef,
  type Leak,
  type Profile,
  type ProfileDimension,
  type ProfileWindow,
  type ProjectArchetype,
  type TraitTrend,
} from './types';
import { HEATMAP_LEVELS, heatmapFill, heatmapLevel, maxTokens } from './heatmap';

// ── tiny style helpers (tokens only; full page, scrollable) ───────────────────
const SHELL: CSSProperties = {
  position: 'fixed',
  inset: 0,
  zIndex: 9000, // above the war-room z-scale (max --z-controls: 50)
  background: 'var(--bg)',
  color: 'var(--ink)',
  fontFamily: 'var(--mono)',
  overflowY: 'auto',
  overflowX: 'hidden',
  padding: '0 0 80px',
};
const HEADER: CSSProperties = {
  position: 'sticky',
  top: 0,
  zIndex: 1,
  display: 'flex',
  alignItems: 'center',
  gap: 16,
  flexWrap: 'wrap',
  padding: '16px 24px',
  background: 'var(--panel-strong)',
  borderBottom: '1px solid var(--hair)',
  backdropFilter: 'blur(6px)',
};
const SECTION: CSSProperties = {
  padding: '20px 24px',
  borderBottom: '1px solid var(--hair)',
};
const H2: CSSProperties = {
  margin: '0 0 12px',
  fontSize: 13,
  letterSpacing: '0.14em',
  textTransform: 'uppercase',
  color: 'var(--acid)',
};
const SUBTLE: CSSProperties = { color: 'var(--ink-faint)', fontSize: 12 };

function pct(n: number): string {
  return `${Math.round((Number.isFinite(n) ? n : 0) * 100)}%`;
}

// ── window toggle ─────────────────────────────────────────────────────────────
function WindowToggle({
  value,
  onChange,
}: {
  value: ProfileWindow;
  onChange: (w: ProfileWindow) => void;
}) {
  return (
    <div style={{ display: 'flex', gap: 6 }} role="tablist" aria-label="time window">
      {PROFILE_WINDOWS.map((w) => {
        const active = w === value;
        return (
          <button
            key={w}
            role="tab"
            aria-selected={active}
            onClick={() => onChange(w)}
            style={{
              cursor: 'pointer',
              fontFamily: 'var(--mono)',
              fontSize: 12,
              padding: '6px 12px',
              borderRadius: 4,
              border: `1px solid ${active ? 'var(--green)' : 'var(--hair)'}`,
              background: active ? 'rgba(118,255,157,0.14)' : 'transparent',
              color: active ? 'var(--green)' : 'var(--ink-soft)',
            }}
          >
            {WINDOW_LABELS[w]}
          </button>
        );
      })}
    </div>
  );
}

// ── efficiency ────────────────────────────────────────────────────────────────
function EfficiencyPanel({ eff }: { eff: EfficiencyScore }) {
  // Show the headline on a 0–100 scale alongside the raw 0.00–1.00, NEVER alone:
  // the per-family bars sit right beneath it so the composite is always in context.
  const families = [...eff.families].sort((a, b) => b.weight - a.weight);
  return (
    <section style={SECTION}>
      <h2 style={H2}>Efficiency</h2>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 16, marginBottom: 8 }}>
        <div style={{ fontSize: 44, lineHeight: 1, color: 'var(--green)' }}>
          {Math.round(eff.headline * 100)}
          <span style={{ fontSize: 18, color: 'var(--ink-faint)' }}> / 100</span>
        </div>
        <div style={SUBTLE}>
          {eff.headline.toFixed(2)} · rubric {eff.rubric_version} · {eff.session_count} sessions
        </div>
      </div>
      <div style={{ display: 'grid', gap: 8, maxWidth: 720 }}>
        {families.map((f) => (
          <div key={f.key} style={{ display: 'grid', gridTemplateColumns: '180px 1fr 96px', gap: 10, alignItems: 'center' }}>
            <div style={{ fontSize: 12, color: 'var(--ink-soft)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={f.key}>
              {f.key}
            </div>
            <div style={{ height: 12, background: 'rgba(118,255,157,0.08)', borderRadius: 3, overflow: 'hidden', border: '1px solid var(--hair)' }}>
              <div
                style={{
                  height: '100%',
                  width: pct(f.sub_score),
                  background: 'var(--green)',
                  borderRadius: 3,
                }}
              />
            </div>
            <div style={{ fontSize: 11, color: 'var(--ink-faint)', textAlign: 'right' }}>
              {pct(f.sub_score)} · w{f.weight.toFixed(2)}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

// ── activity heatmap ──────────────────────────────────────────────────────────
function HeatmapPanel({ cells }: { cells: ActivityCell[] }) {
  const [hover, setHover] = useState<{ cell: ActivityCell; x: number; y: number } | null>(null);
  const max = useMemo(() => maxTokens(cells), [cells]);

  if (cells.length === 0) {
    return (
      <section style={SECTION}>
        <h2 style={H2}>Activity</h2>
        <div style={SUBTLE}>No activity in this window.</div>
      </section>
    );
  }

  // GitHub-style: columns of 7 (a week each), oldest → newest left to right.
  const sorted = [...cells].sort((a, b) => a.date.localeCompare(b.date));
  const cell = 13;
  const gap = 3;

  return (
    <section style={SECTION}>
      <h2 style={H2}>Activity</h2>
      <div style={{ position: 'relative', overflowX: 'auto' }}>
        <div
          style={{
            display: 'grid',
            gridTemplateRows: `repeat(7, ${cell}px)`,
            gridAutoFlow: 'column',
            gridAutoColumns: `${cell}px`,
            gap,
            width: 'max-content',
          }}
        >
          {sorted.map((c) => {
            const level = heatmapLevel(c.total_tokens, max);
            return (
              <div
                key={c.date}
                onMouseEnter={(e) => setHover({ cell: c, x: e.clientX, y: e.clientY })}
                onMouseMove={(e) => setHover({ cell: c, x: e.clientX, y: e.clientY })}
                onMouseLeave={() => setHover(null)}
                style={{
                  width: cell,
                  height: cell,
                  borderRadius: 2,
                  background: heatmapFill(level),
                  outline: '1px solid rgba(118,255,157,0.10)',
                }}
              />
            );
          })}
        </div>

        {/* legend */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 10, ...SUBTLE }}>
          <span>less</span>
          {Array.from({ length: HEATMAP_LEVELS }, (_, i) => (
            <span key={i} style={{ width: cell, height: cell, borderRadius: 2, background: heatmapFill(i), display: 'inline-block', outline: '1px solid rgba(118,255,157,0.10)' }} />
          ))}
          <span>more</span>
        </div>
      </div>

      {hover ? (
        <div
          style={{
            position: 'fixed',
            left: Math.min(hover.x + 14, window.innerWidth - 240),
            top: hover.y + 14,
            zIndex: 9100,
            pointerEvents: 'none',
            minWidth: 180,
            maxWidth: 240,
            padding: '8px 10px',
            background: 'var(--panel-strong)',
            border: '1px solid var(--hair-bright)',
            borderRadius: 6,
            boxShadow: 'var(--glow)',
            fontSize: 12,
          }}
        >
          <div style={{ color: 'var(--green)' }}>{hover.cell.date}</div>
          <div style={{ color: 'var(--ink-soft)' }}>
            {hover.cell.total_tokens.toLocaleString()} tokens · {hover.cell.session_count} sessions
          </div>
          {hover.cell.by_harness.length > 0 ? (
            <div style={{ marginTop: 4, display: 'grid', gap: 2 }}>
              {hover.cell.by_harness.map((h) => (
                <div key={h.harness} style={{ display: 'flex', justifyContent: 'space-between', gap: 10, ...SUBTLE }}>
                  <span style={{ color: 'var(--ink-soft)' }}>{h.harness}</span>
                  <span>{h.tokens.toLocaleString()} · {h.sessions}s</span>
                </div>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

// ── dimensions ────────────────────────────────────────────────────────────────
function StatusChip({ status }: { status: Claim['status'] }) {
  const asserted = status === 'asserted';
  return (
    <span
      style={{
        fontSize: 10,
        letterSpacing: '0.08em',
        textTransform: 'uppercase',
        padding: '1px 6px',
        borderRadius: 3,
        border: `1px solid ${asserted ? 'var(--green)' : 'var(--warn)'}`,
        color: asserted ? 'var(--green)' : 'var(--warn)',
      }}
    >
      {status}
    </span>
  );
}

function ClaimRow({ claim }: { claim: Claim }) {
  return (
    <li style={{ marginBottom: 8, listStyle: 'none' }}>
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8 }}>
        <StatusChip status={claim.status} />
        <span style={{ fontSize: 13, color: 'var(--ink)', lineHeight: 1.4 }}>{claim.text}</span>
      </div>
      <div style={{ ...SUBTLE, marginLeft: 4 }}>
        confidence {pct(claim.confidence)} · {claim.evidence.length} evidence
        {claim.evidence.length > 0 ? ` · ${evidenceLabel(claim.evidence[0])}` : ''}
      </div>
    </li>
  );
}

function evidenceLabel(ev: EvidenceRef): string {
  const where = ev.source_path ?? ev.session_id;
  const tail = where ? where.split('/').pop() ?? where : 'session';
  return ev.quote ? `“${truncate(ev.quote, 60)}”` : tail;
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

function DimensionPanel({ dim }: { dim: ProfileDimension }) {
  return (
    <div
      style={{
        padding: 14,
        border: '1px solid var(--hair)',
        borderRadius: 8,
        background: 'var(--panel)',
      }}
    >
      <div style={{ fontSize: 14, color: 'var(--green)', marginBottom: 4 }}>{dim.title}</div>
      <div style={{ fontSize: 13, color: 'var(--ink-soft)', lineHeight: 1.5, marginBottom: 10 }}>{dim.narrative}</div>
      {dim.claims.length > 0 ? (
        <ul style={{ margin: 0, padding: 0 }}>
          {dim.claims.map((c, i) => (
            <ClaimRow key={i} claim={c} />
          ))}
        </ul>
      ) : (
        <div style={SUBTLE}>No claims.</div>
      )}
    </div>
  );
}

// ── leaks ─────────────────────────────────────────────────────────────────────
function LeaksPanel({ leaks }: { leaks: Leak[] }) {
  return (
    <section style={SECTION}>
      <h2 style={H2}>Ranked leaks</h2>
      {leaks.length === 0 ? (
        <div style={SUBTLE}>No leaks surfaced.</div>
      ) : (
        <div style={{ display: 'grid', gap: 8, maxWidth: 820 }}>
          {[...leaks]
            .sort((a, b) => a.rank - b.rank)
            .map((l) => (
              <div
                key={`${l.rank}-${l.title}`}
                style={{ display: 'flex', gap: 12, alignItems: 'baseline', padding: '8px 10px', border: '1px solid var(--hair)', borderRadius: 6, background: 'var(--panel)' }}
              >
                <span style={{ color: 'var(--amber)', fontSize: 16, minWidth: 28 }}>#{l.rank}</span>
                <span style={{ flex: 1, fontSize: 13, color: 'var(--ink)' }}>{l.title}</span>
                <span style={SUBTLE}>
                  ~{l.est_cost_tokens.toLocaleString()} tok · ~{Math.round(l.est_cost_minutes)} min · {l.evidence.length} ev
                </span>
              </div>
            ))}
        </div>
      )}
    </section>
  );
}

// ── archetypes ────────────────────────────────────────────────────────────────
function ArchetypesPanel({ archetypes }: { archetypes: ProjectArchetype[] }) {
  return (
    <section style={SECTION}>
      <h2 style={H2}>Project archetypes</h2>
      {archetypes.length === 0 ? (
        <div style={SUBTLE}>No archetypes.</div>
      ) : (
        <div style={{ display: 'grid', gap: 10, gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))' }}>
          {archetypes.map((a) => (
            <div key={a.archetype} style={{ padding: 12, border: '1px solid var(--hair)', borderRadius: 8, background: 'var(--panel)' }}>
              <div style={{ color: 'var(--green)', fontSize: 13 }}>{a.archetype}</div>
              <div style={{ ...SUBTLE, marginBottom: 6 }}>{a.session_count} sessions</div>
              <div style={{ fontSize: 12, color: 'var(--ink-soft)', lineHeight: 1.4, marginBottom: 6 }}>{a.note}</div>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                {a.projects.map((p) => (
                  <span key={p} style={{ fontSize: 11, color: 'var(--ink-faint)', border: '1px solid var(--hair)', borderRadius: 3, padding: '1px 6px' }}>
                    {p.split('/').pop() || p}
                  </span>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

// ── trajectory ────────────────────────────────────────────────────────────────
const DIRECTION_GLYPH: Record<TraitTrend['direction'], { glyph: string; color: string }> = {
  improving: { glyph: '▲', color: 'var(--green)' },
  regressing: { glyph: '▼', color: 'var(--red)' },
  plateaued: { glyph: '▬', color: 'var(--warn)' },
  insufficient: { glyph: '·', color: 'var(--ink-faint)' },
};

function Sparkline({ points }: { points: TraitTrend['points'] }) {
  if (points.length < 2) return <span style={SUBTLE}>{points.length} point(s)</span>;
  const vals = points.map((p) => p.value);
  const lo = Math.min(...vals);
  const hi = Math.max(...vals);
  const span = hi - lo || 1;
  const w = 120;
  const h = 26;
  const step = w / (points.length - 1);
  const d = points
    .map((p, i) => {
      const x = i * step;
      const y = h - ((p.value - lo) / span) * h;
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');
  return (
    <svg width={w} height={h} style={{ display: 'block' }} aria-hidden>
      <path d={d} fill="none" stroke="var(--green)" strokeWidth={1.5} />
    </svg>
  );
}

function TrajectoryPanel({ trajectory }: { trajectory: TraitTrend[] }) {
  return (
    <section style={SECTION}>
      <h2 style={H2}>Trajectory</h2>
      {trajectory.length === 0 ? (
        <div style={SUBTLE}>No trajectory data.</div>
      ) : (
        <div style={{ display: 'grid', gap: 8, maxWidth: 720 }}>
          {trajectory.map((t) => {
            const d = DIRECTION_GLYPH[t.direction] ?? DIRECTION_GLYPH.insufficient;
            return (
              <div key={t.trait_key} style={{ display: 'grid', gridTemplateColumns: '180px 120px 1fr', gap: 12, alignItems: 'center', padding: '6px 10px', border: '1px solid var(--hair)', borderRadius: 6, background: 'var(--panel)' }}>
                <div style={{ fontSize: 12, color: 'var(--ink-soft)' }}>{t.trait_key}</div>
                <Sparkline points={t.points} />
                <div style={{ fontSize: 12, color: d.color }}>
                  {d.glyph} {t.direction} · {pct(t.confidence)}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

// ── the screen ────────────────────────────────────────────────────────────────
export function ProfileScreen({ onClose }: { onClose: () => void }) {
  const [win, setWin] = useState<ProfileWindow>('all-time');
  const [eff, setEff] = useState<EfficiencyScore | null>(null);
  const [cells, setCells] = useState<ActivityCell[]>([]);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [loading, setLoading] = useState(false);
  const [building, setBuilding] = useState(false);
  const [progress, setProgress] = useState<DossierProgress[]>([]);
  const [err, setErr] = useState<string | null>(null);

  // Guard against a stale window's response landing after a faster newer one.
  const reqSeq = useRef(0);

  const load = useCallback(async (w: ProfileWindow) => {
    const seq = ++reqSeq.current;
    setLoading(true);
    setErr(null);
    setProfile(null);
    setEff(null);
    setCells([]);
    setProgress([]);
    try {
      // Fast deterministic reads + the (maybe-null) cached profile, in parallel.
      const [effRes, cellRes, profRes] = await Promise.allSettled([
        invoke<EfficiencyScore>('get_efficiency_score', { window: w }),
        invoke<ActivityCell[]>('get_activity_heatmap', { window: w }),
        invoke<Profile | null>('get_profile', { window: w }),
      ]);
      if (seq !== reqSeq.current) return; // a newer window won; drop this result
      if (effRes.status === 'fulfilled') setEff(effRes.value);
      if (cellRes.status === 'fulfilled' && Array.isArray(cellRes.value)) setCells(cellRes.value);
      if (profRes.status === 'fulfilled') setProfile(profRes.value ?? null);

      // If EVERYTHING rejected we're almost certainly off the Tauri runtime
      // (browser preview) — surface a clear note instead of a blank page.
      if (effRes.status === 'rejected' && cellRes.status === 'rejected' && profRes.status === 'rejected') {
        setErr(`backend unavailable: ${String((effRes as PromiseRejectedResult).reason)}`);
      }
    } finally {
      if (seq === reqSeq.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(win);
  }, [win, load]);

  // Stream build progress from the backend while a build is in flight.
  useEffect(() => {
    if (!building) return;
    let un: undefined | (() => void);
    let cancelled = false;
    listen<DossierProgress>('dossier_progress', (e) => {
      setProgress((prev) => [...prev, e.payload]);
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
    setProgress([]);
    setErr(null);
    const w = win;
    try {
      const p = await invoke<Profile>('build_profile', { window: w });
      if (w === win) setProfile(p);
      // Refresh the deterministic panels too (efficiency may have been recomputed).
      void load(w);
    } catch (e) {
      setErr(`build failed: ${String(e)}`);
    } finally {
      setBuilding(false);
    }
  }, [win, load]);

  return (
    <div style={SHELL}>
      <header style={HEADER}>
        <button
          onClick={onClose}
          aria-label="close profile"
          style={{ cursor: 'pointer', fontFamily: 'var(--mono)', fontSize: 13, padding: '6px 12px', borderRadius: 4, border: '1px solid var(--hair)', background: 'transparent', color: 'var(--ink-soft)' }}
        >
          ← Close
        </button>
        <div style={{ fontSize: 14, letterSpacing: '0.16em', textTransform: 'uppercase', color: 'var(--green)' }}>
          DOSSIER · Profile by Proof
        </div>
        <div style={{ flex: 1 }} />
        <WindowToggle value={win} onChange={setWin} />
      </header>

      {err ? (
        <div style={{ ...SECTION, color: 'var(--warn)' }}>{err}</div>
      ) : null}

      {loading ? (
        <div style={{ ...SECTION, color: 'var(--ink-faint)' }}>Loading {WINDOW_LABELS[win]}…</div>
      ) : null}

      {/* Efficiency — always paired headline + family bars. */}
      {eff ? <EfficiencyPanel eff={eff} /> : null}

      {/* Activity heatmap. */}
      <HeatmapPanel cells={cells} />

      {/* Profile body: either the cached/built profile, or the build affordance. */}
      {profile ? (
        <>
          <section style={SECTION}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
              <h2 style={{ ...H2, margin: 0 }}>Dimensions</h2>
              {profile.detector_only ? (
                <span style={{ fontSize: 11, color: 'var(--warn)', border: '1px solid var(--warn)', borderRadius: 3, padding: '1px 6px' }}>
                  deterministic (no LLM) mode
                </span>
              ) : null}
              <span style={SUBTLE}>
                generated {profile.generated_at} · {profile.session_count} sessions · hash {profile.data_hash.slice(0, 8)}
              </span>
            </div>
            <div style={{ display: 'grid', gap: 12, gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))', marginTop: 12 }}>
              {profile.dimensions.map((d) => (
                <DimensionPanel key={d.key} dim={d} />
              ))}
            </div>
          </section>

          <LeaksPanel leaks={profile.ranked_leaks} />
          <ArchetypesPanel archetypes={profile.archetypes} />
          <TrajectoryPanel trajectory={profile.trajectory} />
        </>
      ) : (
        <section style={SECTION}>
          <h2 style={H2}>Profile</h2>
          {building ? (
            <div>
              <div style={{ color: 'var(--green)', marginBottom: 8 }}>Building profile for {WINDOW_LABELS[win]}…</div>
              <ol style={{ margin: 0, paddingLeft: 18, ...SUBTLE }}>
                {progress.map((p, i) => (
                  <li key={i} style={{ color: 'var(--ink-soft)' }}>
                    {p.stage} — {p.status}
                  </li>
                ))}
                {progress.length === 0 ? <li>starting…</li> : null}
              </ol>
            </div>
          ) : (
            <div>
              <div style={{ ...SUBTLE, marginBottom: 10 }}>
                No profile cached for {WINDOW_LABELS[win]} yet. Building runs the Diagnostician→synthesis
                pipeline over this window.
              </div>
              <button
                onClick={onBuild}
                disabled={!!err}
                style={{ cursor: 'pointer', fontFamily: 'var(--mono)', fontSize: 13, padding: '8px 16px', borderRadius: 4, border: '1px solid var(--green)', background: 'rgba(118,255,157,0.14)', color: 'var(--green)' }}
              >
                Build profile
              </button>
            </div>
          )}
        </section>
      )}
    </div>
  );
}

export default ProfileScreen;
