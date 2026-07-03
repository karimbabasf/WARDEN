// RadarDetailPanel.tsx — the click-through readout for one live agent (Tasks 19–21).
//
// Mounted by WarRoom as a right-dock glass panel (the `wd-detail` / `wd-inspector`
// look from the Habits inspector, NOT forked from it), opened when a radar globe is
// selected and the camera has dived in. Four honest sections:
//   1. Live context window          (Task 19)
//   2. Live activity feed           (Task 20)
//   3. Children roster              (Task 21)
//   4. Identity + cost              (Task 21)
//
// The context window is a static readout fed by live `radar_state`; rows come from
// the backend when available, and fall back to honest occupancy/free-space rows.

import type { CSSProperties } from 'react';
import { useState } from 'react';
import type { RadarAgent, RadarContextRow } from './radarTypes';
import { radarSubtitle, radarOriginBadge } from './radarTypes';
import { radarHarness } from './radarTheme';

// ── small pure formatters ──────────────────────────────────────────────────────
function pct(fill: number): string {
  return `${Math.round(fill * 100)}%`;
}

/** Compact token magnitude: 172000 → "172k", 940 → "940". */
function tokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k`;
  return String(Math.round(n));
}

function rowPct(p: number): string {
  if (!Number.isFinite(p)) return '0.0%';
  return `${(Math.max(0, Math.min(1, p)) * 100).toFixed(1)}%`;
}

const STATUS_LABEL: Record<RadarAgent['status'], string> = {
  working: 'Working',
  idle: 'Idle',
  closed: 'Closed',
  terminated: 'Terminated',
};

/**
 * Relative-time stamp for the activity feed ("5m ago"), computed against `now`
 * (injectable so it is pure + testable). Tolerant of an unparseable/missing ts:
 * returns '' rather than ever rendering NaN — the feed simply omits the time then.
 */
export function relativeTime(ts: string, now: number = Date.now()): string {
  const t = Date.parse(ts);
  if (!Number.isFinite(t)) return '';
  const sec = Math.max(0, Math.round((now - t) / 1000));
  if (sec < 5) return 'just now';
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.floor(hr / 24)}d ago`;
}

/**
 * Uptime since `startedAt` ("5m", "2h 0m", "1d 3h"). Injectable `now` for tests.
 * Returns "—" for a missing/unparseable start — never NaN.
 */
export function uptime(startedAt: string, now: number = Date.now()): string {
  const t = Date.parse(startedAt);
  if (!Number.isFinite(t)) return '—';
  const sec = Math.max(0, Math.round((now - t) / 1000));
  const min = Math.floor(sec / 60);
  if (min < 1) return `${sec}s`;
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ${min % 60}m`;
  return `${Math.floor(hr / 24)}d ${hr % 24}h`;
}

/** Estimated cost → "$0.42", or "—" when null (never a fabricated figure). */
function cost(usd: number | null): string {
  if (usd == null || !Number.isFinite(usd)) return '—';
  return `$${usd.toFixed(2)}`;
}

/** Per-kind glyph + readable word (colour is never the only signal). */
const ACTIVITY_KIND: Record<string, { glyph: string; label: string }> = {
  tool: { glyph: '⚙', label: 'Tool' },
  message: { glyph: '✎', label: 'Message' },
  thinking: { glyph: '✶', label: 'Thinking' },
};
function activityKind(kind: string): { glyph: string; label: string } {
  return ACTIVITY_KIND[kind] ?? { glyph: '•', label: kind || 'Event' };
}

function fallbackContextRows(agent: RadarAgent): RadarContextRow[] {
  const max = Math.max(0, agent.maxTokens);
  const used = Math.max(0, agent.contextTokens);
  const usedPct = max > 0 ? Math.min(1, used / max) : 0;
  const rows: RadarContextRow[] = [
    {
      key: 'context',
      label: 'Context',
      tokens: used,
      percent: usedPct,
      count: null,
      muted: false,
    },
  ];
  if (max > 0) {
    rows.push({
      key: 'free_space',
      label: 'Free space',
      tokens: Math.max(0, max - used),
      percent: Math.max(0, 1 - usedPct),
      count: null,
      muted: true,
    });
  }
  return rows;
}

function contextRows(agent: RadarAgent): RadarContextRow[] {
  const rows = activeBreakdown(agent)?.rows ?? [];
  return rows.length > 0 ? rows : fallbackContextRows(agent);
}

function activeBreakdown(agent: RadarAgent) {
  const breakdown = agent.contextBreakdown;
  return breakdown && breakdown.rows.length > 0 ? breakdown : null;
}

// ── Section 1: screenshot-style context window (live, static readout) ─────────
function ContextSection({ agent }: { agent: RadarAgent }) {
  // Flat harness hue — the gauge's fill is shown by the BAR WIDTH below, not by
  // tinting the colour (colour no longer encodes load anywhere in the radar).
  const heat = radarHarness(agent.harness).color;
  const breakdown = activeBreakdown(agent);
  const used = breakdown?.usedTokens ?? agent.contextTokens;
  const max = breakdown?.maxTokens ?? agent.maxTokens;
  const fill = breakdown?.fillPct ?? agent.fillPct;
  const rows = contextRows(agent);

  return (
    <section className="wd-radar-section wd-context-window" data-context-window style={{ '--heat': heat } as CSSProperties}>
      <div className="wd-context-head">
        <span className="wd-context-head-label">Context window</span>
        <span className="wd-context-head-value">
          {tokens(used)} / {max > 0 ? tokens(max) : '∞'} ({pct(fill)})
        </span>
        <span className="wd-context-head-caret" aria-hidden>
          ˅
        </span>
      </div>
      <div className="wd-context-track" aria-hidden>
        <div className="wd-context-fill" style={{ width: `${Math.round(fill * 100)}%` }} />
      </div>
      <ul className="wd-context-rows">
        {rows.map((row) => (
          <li
            key={row.key}
            className={`wd-context-row${row.muted ? ' is-muted' : ''}`}
            data-context-row={row.key}
            style={{ '--row-fill': row.muted ? 'var(--ink-faint)' : heat } as CSSProperties}
          >
            <span className="wd-context-dot" aria-hidden />
            <span className="wd-context-label">{row.label}</span>
            <span className="wd-context-tokens">{tokens(row.tokens)}</span>
            <span className="wd-context-percent">{rowPct(row.percent)}</span>
            {row.count == null ? null : <span className="wd-context-count">{row.count}</span>}
          </li>
        ))}
      </ul>
      <div className="wd-context-source">
        <span>Live</span>
        <span>{radarHarness(agent.harness).label}</span>
      </div>
    </section>
  );
}

// ── Section 2: live activity feed (Task 20) ────────────────────────────────────
// No cap: the backend ships the agent's full action history and we render all of
// it. ~10 rows are visible at once and the feed scrolls (CSS .wd-radar-feed) so
// you can scroll back to the very first action.
function ActivitySection({ agent }: { agent: RadarAgent }) {
  // Newest-first. Sort by parsed ts desc; entries with an unparseable ts keep
  // their original order and sink to the end (stable, never throws on bad data).
  const ordered = agent.recentActivity
    .map((a, i) => ({ a, i, t: Date.parse(a.ts) }))
    .sort((x, y) => {
      const xt = Number.isFinite(x.t) ? x.t : -Infinity;
      const yt = Number.isFinite(y.t) ? y.t : -Infinity;
      return yt - xt || x.i - y.i;
    });

  return (
    <section className="wd-radar-section wd-radar-activity" data-section="activity">
      <div className="wd-card-kicker">Activity</div>
      {ordered.length === 0 ? (
        <div className="wd-radar-empty wd-radar-feed-empty">No recent activity</div>
      ) : (
        <ul className="wd-radar-feed">
          {ordered.map(({ a, i }) => {
            const k = activityKind(a.kind);
            const rel = relativeTime(a.ts);
            return (
              <li key={`${a.ts}-${i}`} className="wd-radar-feed-row" data-activity-row data-kind={a.kind}>
                <span className={`wd-radar-feed-glyph is-${a.kind}`} title={k.label} aria-hidden>
                  {k.glyph}
                </span>
                <span className="wd-radar-feed-label">
                  <span className="wd-radar-feed-kind">{k.label}</span>
                  {a.label}
                </span>
                {rel ? <time className="wd-radar-feed-time">{rel}</time> : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** A child's display name: role, else nickname, else label, else id. */
function childName(c: RadarAgent): string {
  return c.role || c.nickname || c.label || c.id;
}

// ── Section 3: children roster (Task 21) ───────────────────────────────────────
// `children` are the real subagents (parentId === this agent's id), passed in by
// WarRoom. A flat agent gets NO roster at all (honest-viz: never a fabricated or
// empty-but-present children list). Clicking a row flies the camera to that globe.
function RosterSection({ children, onJumpTo }: { children: RadarAgent[]; onJumpTo?: (id: string) => void }) {
  if (children.length === 0) return null;
  return (
    <section className="wd-radar-section wd-radar-roster" data-section="roster">
      <div className="wd-card-kicker">
        Children<span className="wd-radar-roster-count"> · {children.length}</span>
      </div>
      <ul className="wd-radar-roster-list">
        {children.map((c) => {
          const theme = radarHarness(c.harness);
          return (
            <li key={c.id} className="wd-radar-roster-row" data-roster-row data-child-id={c.id}>
              <button
                type="button"
                className="wd-radar-roster-btn"
                style={{ '--harness': theme.color } as CSSProperties}
                onClick={() => onJumpTo?.(c.id)}
                title={`Fly to ${childName(c)}`}
              >
                <span className="wd-radar-roster-glyph" aria-hidden>
                  {theme.glyph}
                </span>
                <span className="wd-radar-roster-name">{childName(c)}</span>
                <span className={`wd-radar-status is-${c.status}`}>{STATUS_LABEL[c.status]}</span>
                <span className="wd-radar-roster-fill">{pct(c.fillPct)}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

// ── Section 4: identity + cost (Task 21) ───────────────────────────────────────
function IdentitySection({ agent }: { agent: RadarAgent }) {
  const theme = radarHarness(agent.harness);
  return (
    <section className="wd-radar-section wd-radar-identity" data-section="identity">
      <div className="wd-card-kicker">Identity</div>
      <dl className="wd-radar-id-grid">
        <div>
          <dt>Harness</dt>
          <dd>
            <span aria-hidden>{theme.glyph}</span> {theme.label}
          </dd>
        </div>
        {radarOriginBadge(agent) ? (
          <div>
            <dt>Host</dt>
            <dd>{radarOriginBadge(agent)}</dd>
          </div>
        ) : null}
        {agent.role ? (
          <div>
            <dt>Role</dt>
            <dd>{agent.role}</dd>
          </div>
        ) : null}
        {agent.cwdPath ? (
          <div>
            <dt>Path</dt>
            <dd title={agent.cwdPath}>{agent.cwdPath}</dd>
          </div>
        ) : null}
        <div>
          <dt>Model</dt>
          <dd>{agent.model ?? '—'}</dd>
        </div>
        <div>
          <dt>Uptime</dt>
          <dd>{uptime(agent.startedAt)}</dd>
        </div>
        <div data-id="cost">
          <dt>Est. cost</dt>
          <dd>{cost(agent.estCostUsd)}</dd>
        </div>
      </dl>
    </section>
  );
}

export type RadarDetailPanelProps = {
  agent: RadarAgent;
  /** Real subagents of this agent (parentId === agent.id), supplied by WarRoom. */
  children?: RadarAgent[];
  /** Fly the camera to a child globe (select + focus). */
  onJumpTo?: (id: string) => void;
  onClose?: () => void;
  /**
   * Forcefully terminate this agent. What this does depends on the harness (see
   * `terminateAffordance` below): a Claude root kills its own process; a Claude
   * subagent has no process and the control is disabled; a Codex agent force-quits
   * the shared app-server and so closes EVERY Codex session — which is why that
   * variant requires an explicit confirm before it fires.
   */
  onTerminate?: (id: string) => void | Promise<void>;
};

/** What the Terminate control should be for a given agent — gated by what's actually
 *  killable on this machine, so we never show an enabled button that always errors.
 *   • claude root (depth 0)        → "Terminate", fires straight through.
 *   • claude subagent (depth > 0)  → DISABLED: it ends with its parent, no own process.
 *   • codex (any)                  → "Force-quit Codex (all sessions)", confirm-gated:
 *     it force-quits the shared `codex app-server`, closing every Codex conversation. */
export type TerminateAffordance = {
  /** Resting button label (paired with the ⏻ glyph). */
  label: string;
  /** false → render the button greyed out with `reason` as subtext. */
  enabled: boolean;
  /** true → first click arms a confirm step; only the confirm invokes. */
  confirm: boolean;
  /** Short why-line: the disabled reason, or the confirm warning. '' when neither. */
  reason: string;
};

export function terminateAffordance(agent: Pick<RadarAgent, 'harness' | 'depth' | 'parentId'>): TerminateAffordance {
  if (agent.harness === 'codex') {
    return {
      label: 'Force-quit Codex (all sessions)',
      enabled: true,
      confirm: true,
      reason: 'Codex shares one process — this closes every Codex session on this machine.',
    };
  }
  const isSubagent = agent.depth > 0 || agent.parentId != null;
  if (isSubagent) {
    return {
      label: 'Terminate',
      enabled: false,
      confirm: false,
      reason: 'Subagents end with their parent — no separate process to terminate.',
    };
  }
  return { label: 'Terminate', enabled: true, confirm: false, reason: '' };
}

export function RadarDetailPanel({ agent, children = [], onJumpTo, onClose, onTerminate }: RadarDetailPanelProps) {
  const theme = radarHarness(agent.harness);
  const title = agent.label || agent.nickname || agent.id;
  const subtitle = radarSubtitle(agent);

  // What the kill control is for THIS agent (label / enabled / confirm-gated / reason).
  const term = terminateAffordance(agent);

  // Local guard so a double-click can't fire two kills; surfaces a backend refusal
  // (e.g. a subagent has no process to kill) inline instead of swallowing it.
  const [terminating, setTerminating] = useState(false);
  const [termError, setTermError] = useState<string | null>(null);
  // Codex force-quit is destructive across ALL its sessions, so the first click only
  // ARMS a confirm; the second click actually fires. Resets if the user cancels.
  const [armed, setArmed] = useState(false);

  const fire = async () => {
    if (terminating || !onTerminate) return;
    setArmed(false);
    setTerminating(true);
    setTermError(null);
    try {
      await onTerminate(agent.id);
    } catch (e) {
      // Backend rejection (Codex EPERM, a Claude subagent, or no Tauri in the QA
      // harness) bubbles here verbatim — surface it inline, re-enable, never swallow.
      setTermError(typeof e === 'string' ? e : (e as Error)?.message ?? 'Termination failed');
      setTerminating(false);
    }
    // On success the agent's globe disappears and this panel unmounts, so there is no
    // need to reset `terminating` — leaving it true keeps the button inert until then.
  };

  const handleTerminate = () => {
    if (terminating || !term.enabled) return;
    if (term.confirm && !armed) {
      setArmed(true); // first click arms the confirm; the Confirm button calls `fire`.
      return;
    }
    void fire();
  };

  // Accent is the flat harness hue — colour no longer encodes fill (that's the
  // globe's SIZE channel). CSS resolves `--heat` → `--harness` via its fallback.
  return (
    <aside
      className="wd-detail wd-radar-detail"
      style={{ '--harness': theme.color } as CSSProperties}
      aria-label={`Agent ${title}`}
    >
      <div className="wd-detail-head">
        <div>
          <div className="wd-card-kicker">
            <span className="wd-card-glyph" aria-hidden>
              {theme.glyph}
            </span>
            {theme.label}
            <span className={`wd-radar-status is-${agent.status}`}> · {STATUS_LABEL[agent.status]}</span>
          </div>
          <h2 className="wd-detail-title">{title}</h2>
          {subtitle ? <div className="wd-detail-sub">{subtitle}</div> : null}
        </div>
        <div className="wd-detail-actions">
          {onTerminate ? (
            armed ? (
              // Codex confirm state — force-quit closes ALL Codex sessions, so make the
              // user say so explicitly. Confirm fires; Cancel disarms. (No dedicated CSS:
              // styled inline from the same palette tokens style.css uses, since the
              // confirm UI is radar-local.)
              <div
                className="wd-detail-actions"
                role="group"
                aria-label="Confirm force-quit"
                style={{ gap: 8 }}
              >
                <span
                  style={{
                    color: 'var(--warn)',
                    fontSize: 10,
                    letterSpacing: '0.06em',
                    textTransform: 'uppercase',
                  }}
                >
                  Force-quit ALL Codex sessions?
                </span>
                <button
                  className="wd-detail-terminate"
                  type="button"
                  onClick={() => void fire()}
                  disabled={terminating}
                  aria-label="Confirm — force-quit every Codex session on this machine"
                >
                  <span aria-hidden>⏻</span> {terminating ? 'Quitting…' : 'Confirm'}
                </button>
                <button
                  className="wd-detail-close"
                  type="button"
                  onClick={() => setArmed(false)}
                  disabled={terminating}
                  aria-label="Cancel force-quit"
                  style={{ width: 'auto', padding: '0 8px', fontSize: 10, letterSpacing: '0.06em', textTransform: 'uppercase' }}
                >
                  Cancel
                </button>
              </div>
            ) : (
              <button
                className="wd-detail-terminate"
                type="button"
                onClick={handleTerminate}
                disabled={terminating || !term.enabled}
                aria-label={term.enabled ? term.label : `${term.label} (unavailable) — ${term.reason}`}
                title={term.reason || term.label}
              >
                <span aria-hidden>⏻</span> {terminating ? 'Terminating…' : term.label}
              </button>
            )
          ) : null}
          {onClose ? (
            <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close detail">
              ✕
            </button>
          ) : null}
        </div>
      </div>
      {/* Why the kill control is disabled (a subagent has no own process). Shown only
          when there's a reason AND the button isn't enabled — never under a live button.
          Muted/info tone (NOT the red error tone) and styled inline from palette tokens,
          since this note is radar-local with no dedicated CSS rule. */}
      {onTerminate && !term.enabled && term.reason ? (
        <div
          className="wd-detail-term-note"
          role="note"
          style={{
            margin: '8px 0 0',
            padding: '6px 9px',
            borderLeft: '2px solid var(--hair)',
            background: 'rgba(118, 255, 157, 0.04)',
            color: 'var(--ink-faint)',
            fontSize: 11,
            letterSpacing: '0.03em',
            lineHeight: 1.5,
          }}
        >
          {term.reason}
        </div>
      ) : null}
      {termError ? (
        <div className="wd-detail-term-error" role="alert">
          {termError}
        </div>
      ) : null}

      <ContextSection agent={agent} />
      <ActivitySection agent={agent} />
      <RosterSection children={children} onJumpTo={onJumpTo} />
      <IdentitySection agent={agent} />
    </aside>
  );
}

export default RadarDetailPanel;
