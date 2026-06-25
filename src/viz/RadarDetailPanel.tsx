// RadarDetailPanel.tsx — the click-through readout for one live agent (Tasks 19–21).
//
// Mounted by WarRoom as a right-dock glass panel (the `wd-detail` / `wd-inspector`
// look from the Habits inspector, NOT forked from it), opened when a radar globe is
// selected and the camera has dived in. Four honest sections:
//   1. Context gauge + composition  (Task 19)
//   2. Live activity feed           (Task 20)
//   3. Children roster              (Task 21)
//   4. Identity + cost              (Task 21)
//
// HONEST COMPOSITION is the load-bearing rule: the EXACT lens (cache-stable / fresh
// / output — anchored in the transcript's token accounting) is ALWAYS shown; the
// SEMANTIC lens (Preamble · Conversation · Tool-output · Thinking) is a local
// ESTIMATE, shown ONLY when `composition.estimated` is present and ALWAYS labeled
// "est."; when it is null the panel prints "—" and renders no semantic bar. An
// estimate is never dressed up as exact, and a missing one is never fabricated.

import type { CSSProperties } from 'react';
import type { RadarAgent } from './radarTypes';
import { radarSubtitle } from './radarTypes';
import { radarHarness, heatColor } from './radarTheme';

// ── small pure formatters ──────────────────────────────────────────────────────
function pct(fill: number): string {
  return `${Math.round(fill * 100)}%`;
}

/** Compact token magnitude: 172000 → "172k", 940 → "940". */
function tokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k`;
  return String(Math.round(n));
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

// ── one stacked composition bar ────────────────────────────────────────────────
type Segment = { key: string; label: string; value: number; color: string };

/**
 * A horizontal stacked bar whose segment widths are proportional to token counts.
 * Each segment carries `data-{attr}-seg="key"` so the honest contract is testable
 * and so screen-readers get a per-segment title (colour is never the only signal).
 */
function CompositionBar({ segments, segAttr }: { segments: Segment[]; segAttr: string }) {
  const total = segments.reduce((s, x) => s + Math.max(0, x.value), 0);
  return (
    <>
      <div className="wd-radar-bar" role="img">
        {segments.map((seg) => {
          const frac = total > 0 ? Math.max(0, seg.value) / total : 0;
          return (
            <span
              key={seg.key}
              className="wd-radar-bar-seg"
              data-seg={seg.key}
              {...{ [`data-${segAttr}-seg`]: seg.key }}
              title={`${seg.label}: ${tokens(seg.value)} (${Math.round(frac * 100)}%)`}
              style={{ width: `${frac * 100}%`, '--seg': seg.color } as CSSProperties}
            />
          );
        })}
      </div>
      <ul className="wd-radar-legend">
        {segments.map((seg) => (
          <li key={seg.key}>
            <span className="wd-radar-legend-dot" style={{ '--seg': seg.color } as CSSProperties} aria-hidden />
            <span className="wd-radar-legend-label">{seg.label}</span>
            <span className="wd-radar-legend-val">{tokens(seg.value)}</span>
          </li>
        ))}
      </ul>
    </>
  );
}

// ── Section 1: context gauge + composition (Task 19) ───────────────────────────
function ContextSection({ agent }: { agent: RadarAgent }) {
  const base = radarHarness(agent.harness).color;
  // gauge fill matches the globe: harness hue heated by fill (one honest signal).
  const heat = heatColor(base, agent.fillPct);
  const exact = agent.composition.exact;
  const est = agent.composition.estimated;

  const exactSegments: Segment[] = [
    { key: 'cacheRead', label: 'Cache-stable', value: exact.cacheRead, color: '#3a7d63' },
    { key: 'fresh', label: 'Fresh', value: exact.fresh, color: base },
    { key: 'output', label: 'Output', value: exact.output, color: '#ffd166' },
  ];

  const estSegments: Segment[] | null = est
    ? [
        { key: 'preamble', label: 'Preamble', value: est.preamble, color: '#5f8a6f' },
        { key: 'conversation', label: 'Conversation', value: est.conversation, color: base },
        { key: 'toolOutput', label: 'Tool-output', value: est.toolOutput, color: '#7bd3ff' },
        { key: 'thinking', label: 'Thinking', value: est.thinking, color: '#b98cff' },
      ]
    : null;

  return (
    <section className="wd-radar-section wd-radar-context">
      {/* heat-matched gauge: contextTokens / maxTokens, fill % */}
      <div className="wd-radar-gauge" style={{ '--heat': heat } as CSSProperties}>
        <div className="wd-radar-gauge-track">
          <div className="wd-radar-gauge-fill" style={{ width: `${Math.round(agent.fillPct * 100)}%` }} />
        </div>
        <div className="wd-radar-gauge-meta">
          <span className="wd-radar-gauge-pct">{pct(agent.fillPct)}</span>
          <span className="wd-radar-gauge-abs">
            {tokens(agent.contextTokens)} / {agent.maxTokens > 0 ? tokens(agent.maxTokens) : '∞'}
          </span>
        </div>
      </div>

      {/* EXACT lens — always shown (API-anchored) */}
      <div className="wd-radar-comp" data-composition="exact">
        <div className="wd-radar-comp-head">
          <span className="wd-card-kicker">Exact</span>
          <span className="wd-radar-comp-note">API-anchored</span>
        </div>
        <CompositionBar segments={exactSegments} segAttr="exact" />
      </div>

      {/* SEMANTIC lens — estimate, shown only when present, always labeled "est." */}
      <div className="wd-radar-comp" data-composition="estimated">
        <div className="wd-radar-comp-head">
          <span className="wd-card-kicker">Semantic</span>
          <span className="wd-radar-comp-note wd-radar-est">est.</span>
        </div>
        {estSegments ? (
          <CompositionBar segments={estSegments} segAttr="est" />
        ) : (
          <div className="wd-radar-empty" aria-label="No estimate available">
            —
          </div>
        )}
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
        {agent.role ? (
          <div>
            <dt>Role</dt>
            <dd>{agent.role}</dd>
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
};

export function RadarDetailPanel({ agent, children = [], onJumpTo, onClose }: RadarDetailPanelProps) {
  const theme = radarHarness(agent.harness);
  const heat = heatColor(theme.color, agent.fillPct);
  const title = agent.label || agent.nickname || agent.id;
  const subtitle = radarSubtitle(agent);

  return (
    <aside
      className="wd-detail wd-radar-detail"
      style={{ '--harness': theme.color, '--heat': heat } as CSSProperties}
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
        {onClose ? (
          <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close detail">
            ✕
          </button>
        ) : null}
      </div>

      <ContextSection agent={agent} />
      <ActivitySection agent={agent} />
      <RosterSection children={children} onJumpTo={onJumpTo} />
      <IdentitySection agent={agent} />
    </aside>
  );
}

export default RadarDetailPanel;
