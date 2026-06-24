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
const ACTIVITY_CAP = 8;

function ActivitySection({ agent }: { agent: RadarAgent }) {
  // Newest-first. Sort by parsed ts desc; entries with an unparseable ts keep
  // their original order and sink to the end (stable, never throws on bad data).
  const ordered = agent.recentActivity
    .map((a, i) => ({ a, i, t: Date.parse(a.ts) }))
    .sort((x, y) => {
      const xt = Number.isFinite(x.t) ? x.t : -Infinity;
      const yt = Number.isFinite(y.t) ? y.t : -Infinity;
      return yt - xt || x.i - y.i;
    })
    .slice(0, ACTIVITY_CAP);

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

export type RadarDetailPanelProps = {
  agent: RadarAgent;
  onClose?: () => void;
};

export function RadarDetailPanel({ agent, onClose }: RadarDetailPanelProps) {
  const theme = radarHarness(agent.harness);
  const heat = heatColor(theme.color, agent.fillPct);
  const title = agent.label || agent.nickname || agent.id;

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
        </div>
        {onClose ? (
          <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close detail">
            ✕
          </button>
        ) : null}
      </div>

      <ContextSection agent={agent} />
      <ActivitySection agent={agent} />
    </aside>
  );
}

export default RadarDetailPanel;
