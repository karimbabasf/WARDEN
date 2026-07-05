// BreakingSection.tsx — §② WHAT YOU'RE BREAKING. The Living-Habits catch→prove→
// erase loop, folded into the Dossier as the section with a pulse.
//
// The Dossier is its OWN React root (separate from the war-room island / bridge),
// so this section owns its habits wiring end-to-end: it subscribes to the
// `habits_refreshed` Tauri event directly and, on mount / Dossier-window change,
// calls `set_habits_window` mapping the Dossier window → the nearest habits
// window. Each active anti-pattern renders as a streak row —
// "N/K clean sessions · M more to erase" (M = max(0, K − credits)) — and flips to
// a satisfying erase-reward state once `fixed`.
//
// Honest-viz: credits/streakK/fixed come straight off the event (snake_case on
// the wire); nothing is fabricated. Empty window → an inviting, truthful note.

import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { BreakingHabit, EvidenceRef, HabitsRefreshed, HabitsWindow, ProfileWindow } from './types';
import { severityColor } from '../harnessTheme';
import { EvidenceList } from './EvidenceList';

/**
 * Map a Dossier window → the nearest Living-Habits window. The habits backend
 * has no 3mo bucket and its finest is 7d (today isn't a Dossier window), so:
 *   2wk → 7d · 30d → 30d · 3mo → 30d · 6mo → 6mo · all-time → all
 */
export function toHabitsWindow(w: ProfileWindow): HabitsWindow {
  switch (w) {
    case '2wk':
      return '7d';
    case '30d':
      return '30d';
    case '3mo':
      return '30d';
    case '6mo':
      return '6mo';
    case 'all-time':
    default:
      return 'all';
  }
}

const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0);
const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : d);

/** Normalize one raw wire OrbIssue → the lean BreakingHabit the section renders. */
export function normalizeHabit(raw: unknown): BreakingHabit {
  const i = (raw ?? {}) as Record<string, unknown>;
  const evidenceRaw = Array.isArray(i.evidence) ? (i.evidence as unknown[]) : [];
  const evidence: EvidenceRef[] = evidenceRaw.map((e) => {
    const o = (e ?? {}) as Record<string, unknown>;
    return {
      session_id: str(o.session_id ?? o.sessionId),
      turn_id: (o.turn_id ?? o.turnId ?? null) as string | null,
      event_id: (o.event_id ?? o.eventId ?? null) as string | null,
      quote: (o.quote ?? null) as string | null,
      source_path: (o.source_path ?? o.sourcePath ?? null) as string | null,
    };
  });
  return {
    id: str(i.id ?? i.pattern_id ?? i.patternId, 'habit'),
    patternId: str(i.pattern_id ?? i.patternId),
    title: str(i.title, 'Untitled pattern'),
    estCostTokens: num(i.est_cost_tokens ?? i.estCostTokens),
    estCostMinutes: num(i.est_cost_minutes ?? i.estCostMinutes),
    severity: num(i.severity),
    credits: num(i.credits),
    streakK: num(i.streak_k ?? i.streakK),
    fixed: i.fixed === true,
    evidence,
  };
}

function HabitRow({ h }: { h: BreakingHabit }) {
  const sev = severityColor(h.severity);
  const k = Math.max(0, Math.round(h.streakK));
  const credits = Math.max(0, Math.min(k || h.credits, Math.round(h.credits)));
  const remaining = Math.max(0, k - credits);
  const frac = k > 0 ? Math.max(0, Math.min(1, credits / k)) : 0;

  if (h.fixed) {
    return (
      <div className="habit habit--fixed" style={{ ['--sev' as string]: 'var(--green)' }}>
        <div className="habit__head">
          <span className="habit__title">{h.title}</span>
        </div>
        <div className="habit__cost">erased</div>
        <div className="habit__erased">
          <span className="check" aria-hidden>
            ✓
          </span>
          <span>Streak complete — {k > 0 ? `${k} clean sessions` : 'held clean'}. This habit is broken.</span>
        </div>
      </div>
    );
  }

  return (
    <div className="habit" style={{ ['--sev' as string]: sev }}>
      <span className="habit__pulse" aria-hidden />
      <div className="habit__head">
        <span className="habit__title">{h.title}</span>
      </div>
      <div className="habit__cost">
        ~{h.estCostTokens.toLocaleString()} tok · ~{Math.round(h.estCostMinutes)} min
      </div>
      <div className="habit__streakwrap">
        <div className="habit__bar" role="progressbar" aria-valuemin={0} aria-valuemax={k || 1} aria-valuenow={credits}>
          <div className="habit__bar-fill" style={{ transform: `scaleX(${frac})` }} />
          {k > 1 ? (
            <div className="habit__ticks" aria-hidden>
              {Array.from({ length: k }, (_, i) => (
                <span key={i} className="habit__tick" />
              ))}
            </div>
          ) : null}
        </div>
        <div className="habit__progress">
          <span className="clean">
            {credits}/{k || '?'} clean sessions
          </span>
          <span className="sep">·</span>
          <span className="more">{remaining > 0 ? `${remaining} more to erase` : 'ready to erase'}</span>
        </div>
      </div>
      {h.evidence.length > 0 ? <EvidenceList evidence={h.evidence} label="Caught in" /> : null}
    </div>
  );
}

export function BreakingSection({ window: win }: { window: ProfileWindow }) {
  const [habits, setHabits] = useState<BreakingHabit[]>([]);
  const [ready, setReady] = useState(false);

  const habitsWindow = useMemo(() => toHabitsWindow(win), [win]);

  // Subscribe once; the listener lives for the section's lifetime.
  useEffect(() => {
    let un: undefined | (() => void);
    let cancelled = false;
    listen<HabitsRefreshed>('habits_refreshed', (e) => {
      const issues = Array.isArray(e.payload?.issues) ? e.payload.issues : [];
      setHabits(issues.map(normalizeHabit));
      setReady(true);
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
  }, []);

  // On window change, ask the backend to (re)scan that window; the answer comes
  // back over `habits_refreshed`. Off the Tauri runtime this simply no-ops.
  useEffect(() => {
    setReady(false);
    invoke('set_habits_window', { window: habitsWindow }).catch(() => {
      // Browser preview / no backend — leave the empty state; not an error.
      setReady(true);
    });
  }, [habitsWindow]);

  // Fixed habits sink to the bottom (reward, then the live work); by severity within.
  const ordered = useMemo(
    () =>
      [...habits].sort((a, b) => {
        if (a.fixed !== b.fixed) return a.fixed ? 1 : -1;
        return b.severity - a.severity;
      }),
    [habits],
  );

  return (
    <section className="dossier__section" aria-label="What you're breaking">
      <div className="dossier__eyebrow">
        <span className="dossier__num">②</span>
        <h2 className="dossier__h">What you're breaking</h2>
        <span className="dossier__hint">catch → prove → erase</span>
      </div>
      {ordered.length === 0 ? (
        <div className="dossier__empty">
          {ready
            ? 'Nothing in remediation — WARDEN surfaces habits to break as it finds them.'
            : 'Scanning this window for anti-patterns…'}
        </div>
      ) : (
        <div className="breaking__list">
          {ordered.map((h) => (
            <HabitRow key={h.id} h={h} />
          ))}
        </div>
      )}
    </section>
  );
}

export default BreakingSection;
