// RadarDetailPanel.tsx — the click-through readout for one live agent (Tasks 19–21).
//
// Mounted by WarRoom as a right-dock glass panel (the `wd-detail` / `wd-inspector`
// look from the Habits inspector, NOT forked from it), opened when a radar globe is
// selected and the camera has dived in. Five honest sections:
//   0. Agent summary (hero)         what it is doing, what it has done, how long
//   1. Live context window          (Task 19)
//   2. Live activity feed           (Task 20)
//   3. Children roster              (Task 21)
//   4. Identity + cost              (Task 21)
//
// The context window is a static readout fed by live `radar_state`; rows come from
// the backend when available, and fall back to honest occupancy/free-space rows.

import type { CSSProperties, KeyboardEvent } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { RadarActivity, RadarAgent, RadarContextRow } from '@/viz/shared/types/radarTypes';
import { radarSubtitle, shortModel, formatTokens as tokens } from '@/viz/shared/types/radarTypes';
import { FilePreview } from '@/viz/shared/ui/FilePreview';
import { CompactControl } from '@/viz/shared/ui/CompactControl';
import { summarizeAgent } from './agentSummary';
import { radarHarness } from './radarTheme';

// ── small pure formatters ──────────────────────────────────────────────────────
function pct(fill: number): string {
  return `${Math.round(fill * 100)}%`;
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

/** Last path segment, for a short accessible name ("Reveal agent.rs in Finder"). */
function basename(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1] || path;
}

/**
 * Split a display path into the directory prefix and the filename.
 *
 * A path in a narrow rail always overflows, and plain `text-overflow: ellipsis`
 * eats the RIGHT end, which is the filename: the one part you actually needed.
 * Rendering the two spans separately lets the DIRECTORY absorb the truncation
 * while the filename stays whole, with no `direction: rtl` trick (which reorders
 * leading punctuation like "~/" and reads wrong).
 */
export function splitPath(path: string): { dir: string; base: string } {
  const cut = path.lastIndexOf('/');
  if (cut < 0) return { dir: '', base: path };
  return { dir: path.slice(0, cut + 1), base: path.slice(cut + 1) || path };
}

/**
 * Reveal a `~`-folded display path in Finder. Fire-and-forget: the caller is a
 * click handler, not an async flow, so a rejection (path moved/deleted since the
 * radar snapshot) is logged and swallowed rather than left as an unhandled
 * rejection or thrown into React's render cycle.
 */
function revealInFinder(path: string): void {
  invoke('reveal_path', { path }).catch((err) => {
    console.error('reveal_path failed', err);
  });
}

/**
 * Push a rename to the backend. Returns the CLEANED name Rust actually stored
 * (it trims and truncates), never the raw typed value, so the caller reconciles
 * to the source of truth instead of assuming the typed string stuck.
 */
function renameSession(agentId: string, name: string): Promise<string> {
  return invoke<string>('rename_session', { agentId, name });
}

/**
 * Live stopwatch readout for an in-flight action ("42s", "1m 23s", "2h 5m") so a
 * long-running call visibly ages rather than freezing at its first-seen elapsed.
 */
function elapsedClock(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ${String(sec % 60).padStart(2, '0')}s`;
  const hr = Math.floor(min / 60);
  return `${hr}h ${min % 60}m`;
}

/** Ticks once a second while `active`, so a mounted hero re-renders its elapsed
 * time without polling the backend. Idle (no action) skips the interval entirely. */
function useTick(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [active, intervalMs]);
  return now;
}

/** Per-kind glyph + readable word (colour is never the only signal). */
const ACTIVITY_KIND: Record<string, { glyph: string; label: string }> = {
  read: { glyph: '▤', label: 'Read' },
  write: { glyph: '◆', label: 'Write' },
  search: { glyph: '⌕', label: 'Search' },
  run: { glyph: '»', label: 'Run' },
  tool: { glyph: '◈', label: 'Tool' },
  message: { glyph: '≡', label: 'Message' },
  thinking: { glyph: '◌', label: 'Thinking' },
};
function activityKind(kind: string): { glyph: string; label: string } {
  return ACTIVITY_KIND[kind] ?? { glyph: '•', label: kind || 'Event' };
}

/** Coarse age ("45s", "12m", "2h", "3d"): changes at most once a minute past the first
 * minute, so a per-second tick never churns the line it sits on. */
function coarseAge(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  return `${Math.floor(hr / 24)}d`;
}

/** The one identity clause the header does NOT carry: root vs subagent, and the team
 * role. Model + harness are repeated in short so the summary reads on its own. */
function identityLine(agent: RadarAgent, harnessLabel: string): string {
  const parts = [(agent.depth ?? 0) > 0 ? 'Subagent' : 'Root agent', harnessLabel];
  const model = shortModel(agent.model);
  if (model) parts.push(model);
  let line = parts.join(' · ');
  const team = agent.team;
  if (team) {
    line += team.isLead
      ? ` · leads ${team.name}`
      : ` · in ${team.name}${team.memberName ? ` as ${team.memberName}` : ''}`;
  }
  return line;
}

// ── The agent summary: four things, at a glance, that do NOT rewrite every second ──
//
// This replaces a single CHURNING headline. The old section WAS the in-flight verb, and
// it flipped between "Editing X", "Reading Y" and "Last ran Z" several times a second as
// the tool call turned over, so the parts that never change (what this agent is, how
// much it has done) were drowned by the one part that changes constantly. Now the block
// answers, in order, the four questions people open the panel to ask:
//
//   Is    what this agent is        (root vs subagent, harness, model, team), stays put
//   Done  what it has done so far    (exact counts + files over its whole feed), only grows
//   Doing what it is doing now       (its one live line, the ONLY ticking part)
//   Next  what it will do next       (a grounded line from role + state, never a guess)
//
// The block MEMOISES on `summary.key`, a value signature of the underlying data, so the
// 750ms radar poll (which hands a fresh agent object every time) and the 1s clock tick
// do not rebuild it. Only the live stopwatch re-renders, aged in the panel from `now`.
// Everything rendered is a count or a label off the real feed (see `summarizeAgent`):
// no intent, no paraphrase, no invented progress.
function AgentSummarySection({ agent }: { agent: RadarAgent }) {
  const [open, setOpen] = useState(false);
  const theme = radarHarness(agent.harness);

  // Memoise by VALUE, never by the agent object: the poll replaces the object every
  // 750ms with equal data, so an identity memo would rebuild on every tick. `memoKey`
  // busts only when something real changed (a new row, a status flip, a new live call).
  const memoKey = [
    agent.id,
    agent.status,
    agent.recentActivity?.length ?? 0,
    agent.recentActivity?.[0]?.ts ?? '',
    agent.currentAction?.kind ?? '',
    agent.currentAction?.startedAt ?? '',
    agent.childCount ?? 0,
    agent.depth ?? 0,
  ].join('|');
  // eslint-disable-next-line react-hooks/exhaustive-deps -- memoKey is the value signature of `agent`
  const summary = useMemo(() => summarizeAgent(agent), [memoKey]);

  // The single live number, aged against a ticking `now`. Tick only while there is
  // something to age (an in-flight call, a "since", or an uptime), so a fully static
  // summary costs no interval. A per-second tick re-renders this section, but the
  // memoised body is unchanged, so only the clock text moves.
  const now = useTick(summary.clockBaseMs != null || summary.startedAtMs != null);
  const clockMs = summary.clockBaseMs != null ? Math.max(0, now - summary.clockBaseMs) : summary.clockFixedMs;

  // Collapse the viewer whenever the live target changes, otherwise the panel keeps
  // showing the previous file under a new heading.
  const target = summary.target;
  const prevTargetRef = useRef(target);
  if (prevTargetRef.current !== target) {
    prevTargetRef.current = target;
    if (open) setOpen(false);
  }

  const doingGlyph = summary.doingKind ? activityKind(summary.doingKind) : null;
  const ranFor = summary.startedAtMs != null ? coarseAge(now - summary.startedAtMs) : null;
  const cost = agent.estCostUsd != null && agent.estCostUsd > 0 ? `$${agent.estCostUsd.toFixed(2)}` : null;
  const doneStat = [ranFor, `${summary.actions} action${summary.actions === 1 ? '' : 's'}`, cost]
    .filter(Boolean)
    .join(' · ');

  return (
    <section
      className={`wd-radar-section wd-agent-summary${summary.inFlight ? ' is-live' : ''}`}
      data-section="agent-summary"
      data-current-action={summary.inFlight ? 'active' : 'idle'}
      data-kind={summary.doingKind ?? undefined}
    >
      <div className="wd-card-kicker">Agent summary</div>

      {/* 1. What it IS. The one identity clause the header does not carry. */}
      <p className="wd-summary-row wd-summary-is">
        <span className="wd-summary-tag">Is</span>
        <span className="wd-summary-val">{identityLine(agent, theme.label)}</span>
      </p>

      {/* 2. What it has DONE since it began: exact counts over the whole feed. */}
      <div className="wd-summary-row wd-summary-done">
        <span className="wd-summary-tag">Done</span>
        <div className="wd-summary-done-body">
          <span className="wd-summary-val">{summary.actions > 0 ? doneStat : ranFor ? `${ranFor} · nothing recorded yet` : 'Nothing recorded yet'}</span>
          {summary.tally.length > 0 ? (
            <ul className="wd-summary-tally" data-summary-tally>
              {summary.tally.map((t) => (
                <li key={t.kind} className="wd-summary-tally-item" data-kind={t.kind}>
                  <span className="wd-summary-tally-glyph" aria-hidden>
                    {activityKind(t.kind).glyph}
                  </span>
                  {t.label}
                </li>
              ))}
            </ul>
          ) : null}
          {summary.files.length > 0 ? (
            <div className="wd-summary-files" data-summary-files>
              <span className="wd-summary-files-label">Touched</span>
              <ul className="wd-summary-files-list">
                {summary.files.map((f) => (
                  <li key={f}>
                    <button
                      type="button"
                      className="wd-summary-file"
                      title={f}
                      onClick={() => revealInFinder(f)}
                      aria-label={`Reveal ${basename(f)} in Finder`}
                    >
                      {basename(f)}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      </div>

      {/* 3. What it is DOING now: the one live line, the only part that ticks. */}
      <div className="wd-summary-row wd-summary-doing" data-inflight={summary.inFlight ? 'yes' : 'no'}>
        <span className="wd-summary-tag">Doing</span>
        <div className="wd-summary-doing-body">
          <div className="wd-summary-doing-main">
            {doingGlyph ? (
              <span className={`wd-summary-glyph is-${summary.doingKind}`} aria-hidden>
                {doingGlyph.glyph}
              </span>
            ) : null}
            <span className="wd-summary-doing-verb">{summary.doing}</span>
            {clockMs != null ? (
              <span className="wd-summary-clock">
                <span className="wd-summary-clock-value">{elapsedClock(clockMs)}</span>
                <span className="wd-summary-clock-label">{summary.clockLabel}</span>
              </span>
            ) : null}
          </div>
          {summary.lastAction ? <div className="wd-summary-doing-last">{summary.lastAction}</div> : null}
          {target ? (
            <div className="wd-summary-target-row">
              {/* Two distinct verbs, so neither is a mystery-meat icon: OPEN reads the
                  file inside WARDEN, REVEAL hands it to Finder. */}
              <button
                type="button"
                className="wd-summary-target"
                onClick={() => setOpen((o) => !o)}
                aria-expanded={open}
                aria-label={`${open ? 'Hide' : 'View'} ${basename(target)}`}
              >
                <span className="wd-summary-target-glyph" aria-hidden>
                  {open ? '▾' : '▸'}
                </span>
                <span className="wd-path" title={target}>
                  <span className="wd-path-dir">{splitPath(target).dir}</span>
                  <span className="wd-path-base">{splitPath(target).base}</span>
                </span>
              </button>
              <button
                type="button"
                className="wd-icon-btn"
                onClick={() => revealInFinder(target)}
                aria-label={`Reveal ${basename(target)} in Finder`}
                title="Reveal in Finder"
              >
                ⌖
              </button>
            </div>
          ) : null}
        </div>
      </div>

      {/* 4. What it will do NEXT: a grounded line from role + state, or nothing. */}
      {summary.next ? (
        <p className="wd-summary-row wd-summary-next">
          <span className="wd-summary-tag">Next</span>
          <span className="wd-summary-val">{summary.next}</span>
        </p>
      ) : null}

      {open && target ? <FilePreview path={target} onClose={() => setOpen(false)} /> : null}
    </section>
  );
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

  // The head carried a caret glyph but was a plain div, so it advertised an
  // expander that did not exist. It is now a real disclosure button, and the
  // exact API-anchored token split lives behind it: a number worth having, but
  // not worth spending four permanent rows of a narrow rail on.
  const [open, setOpen] = useState(false);
  const exact = agent.composition.exact;
  const hot = fill >= 0.85;

  return (
    <section
      className={`wd-radar-section wd-context-window${hot ? ' is-hot' : ''}`}
      data-context-window
      style={{ '--heat': heat } as CSSProperties}
    >
      <button
        type="button"
        className="wd-context-head"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="wd-context-head-label">Context window</span>
        <span className="wd-context-head-value">
          {tokens(used)} / {max > 0 ? tokens(max) : '∞'} ({pct(fill)})
        </span>
        <span className="wd-context-head-caret" aria-hidden>
          {open ? '▾' : '▸'}
        </span>
      </button>
      <div className="wd-context-track" aria-hidden>
        <div className="wd-context-fill" style={{ width: `${Math.round(fill * 100)}%` }} />
      </div>
      {open ? (
        <dl className="wd-context-exact">
          <div>
            <dt>Cache read</dt>
            <dd>{tokens(exact.cacheRead)}</dd>
          </div>
          <div>
            <dt>Fresh input</dt>
            <dd>{tokens(exact.fresh)}</dd>
          </div>
          {/* Cache writes cost MORE than fresh input, so they are worth seeing
              separately rather than folded into it. */}
          <div>
            <dt>Cache write</dt>
            <dd>{tokens(exact.cacheWrite)}</dd>
          </div>
          <div>
            <dt>Output</dt>
            <dd>{tokens(exact.output)}</dd>
          </div>
        </dl>
      ) : null}
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
      {/* The ARM switch belongs ON the meter: the number you are reacting to and
          the action you take about it should not be in two different places. */}
      <CompactControl agentId={agent.id} />

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
/**
 * One feed row, expandable.
 *
 * Collapsed it is a single truncated line, which is what makes a 200-row feed
 * skimmable. Expanded it gives the whole label WRAPPED (a shell command is often
 * the interesting part and truncation hides exactly the tail you want), the exact
 * timestamp, and the two file verbs.
 *
 * Only ONE row is open at a time (the open key lives on the section, not the row),
 * so the feed cannot accordion into an unreadable stack.
 */
function ActivityRow({
  row,
  rowKey,
  expandedKey,
  onToggle,
}: {
  row: RadarActivity;
  rowKey: string;
  expandedKey: string | null;
  onToggle: (key: string | null) => void;
}) {
  const k = activityKind(row.kind);
  const rel = relativeTime(row.ts);
  const open = expandedKey === rowKey;
  const [viewing, setViewing] = useState(false);

  return (
    <li className={`wd-radar-feed-row${open ? ' is-open' : ''}`} data-activity-row data-kind={row.kind}>
      <button
        type="button"
        className="wd-radar-feed-main"
        aria-expanded={open}
        onClick={() => {
          onToggle(open ? null : rowKey);
          if (open) setViewing(false);
        }}
      >
        <span className={`wd-radar-feed-glyph is-${row.kind}`} title={k.label} aria-hidden>
          {k.glyph}
        </span>
        <span className="wd-radar-feed-label">
          <span className="wd-radar-feed-kind">{k.label}</span>
          {row.label}
        </span>
        {rel ? <time className="wd-radar-feed-time">{rel}</time> : null}
        <span className="wd-radar-feed-caret" aria-hidden>
          {open ? '▾' : '▸'}
        </span>
      </button>

      {open ? (
        <div className="wd-radar-feed-detail">
          <p className="wd-radar-feed-full">{row.label || k.label}</p>
          <div className="wd-radar-feed-meta">
            {row.ts ? <span>{row.ts}</span> : null}
            {row.target ? (
              <>
                <button type="button" className="wd-mini-btn" onClick={() => setViewing((v) => !v)}>
                  {viewing ? 'Hide file' : 'View file'}
                </button>
                <button
                  type="button"
                  className="wd-mini-btn"
                  onClick={() => revealInFinder(row.target as string)}
                  aria-label={`Reveal ${basename(row.target)} in Finder`}
                >
                  Reveal
                </button>
              </>
            ) : (
              <span className="wd-radar-feed-nofile">No single file for this row</span>
            )}
          </div>
          {viewing && row.target ? <FilePreview path={row.target} onClose={() => setViewing(false)} /> : null}
        </div>
      ) : null}
    </li>
  );
}

function ActivitySection({ agent }: { agent: RadarAgent }) {
  const [expanded, setExpanded] = useState<string | null>(null);
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
          {ordered.map(({ a, i }) => (
            <ActivityRow key={`${a.ts}-${i}`} row={a} rowKey={`${a.ts}-${i}`} expandedKey={expanded} onToggle={setExpanded} />
          ))}
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
  const team = agent.team ?? null;
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
        {team ? (
          <div data-id="team">
            <dt>Team</dt>
            <dd>
              {team.name}
              {team.memberType ? ` · ${team.memberType}` : ''}
              {team.isLead ? ' · lead' : ''}
            </dd>
          </div>
        ) : null}
        {team?.memberName ? (
          <div data-id="team-member">
            <dt>Member</dt>
            <dd>{team.memberName}</dd>
          </div>
        ) : null}
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

/**
 * The agent's name, editable in place.
 *
 * Renaming used to mean hand-editing a transcript, so this is the whole point of the
 * naming work rather than a convenience. Four behaviours are load-bearing:
 *
 * 1. While the input is focused it OWNS its value. The radar polls `get_radar_state` on
 *    an interval, and a frame landing mid-edit must not overwrite what is being typed.
 * 2. The committed value is the string Rust RETURNED, not the string typed: the backend
 *    trims and truncates, so reconciling to its answer keeps the panel honest.
 * 3. A rejected rename restores the exact previous name and says why, instead of leaving
 *    a silently failed edit on screen.
 * 4. No rename affordance at all renders when `agentId` is empty: there is nothing to
 *    resolve the rename against.
 *
 * `committed` is compared against `heading` at RENDER time (not via an effect keyed on
 * `editing`), so ending an edit can never itself race the optimistic value away before
 * the backend's answer lands; only a genuinely new `heading` prop (a fresh poll) clears it.
 */
function EditableTitle({ agentId, heading }: { agentId: string; heading: string }) {
  const canRename = agentId.length > 0;

  const [committed, setCommitted] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  // Guards a commit firing twice for one edit: Enter and Escape both end by removing
  // focus from the input, which can also fire a native blur.
  const settledRef = useRef(true);

  const prevAgentIdRef = useRef(agentId);
  const prevHeadingRef = useRef(heading);
  if (prevAgentIdRef.current !== agentId) {
    // Switched to a different agent: none of this state belongs to it anymore.
    prevAgentIdRef.current = agentId;
    prevHeadingRef.current = heading;
    setCommitted(null);
    setEditing(false);
    setError(null);
  } else if (prevHeadingRef.current !== heading) {
    // A fresh poll brought a new backend-derived heading: defer to it.
    prevHeadingRef.current = heading;
    setCommitted(null);
  }

  const shown = committed ?? heading;

  function begin() {
    if (!canRename) return;
    settledRef.current = false;
    setDraft(shown);
    setError(null);
    setEditing(true);
  }

  function cancel() {
    if (settledRef.current) return;
    settledRef.current = true;
    setEditing(false);
  }

  function commit() {
    if (settledRef.current) return;
    settledRef.current = true;
    const next = draft.trim();
    const previous = shown;
    setEditing(false);
    if (!next || next === previous) return;
    // Optimistic: show the typed value immediately, then reconcile to whatever Rust
    // actually stored (it trims and truncates) once the call resolves.
    setCommitted(next);
    setError(null);
    renameSession(agentId, next)
      .then((cleaned) => setCommitted(cleaned))
      .catch((err: unknown) => {
        setCommitted(previous);
        setError(typeof err === 'string' ? err : 'rename failed');
      });
  }

  if (!editing) {
    return (
      <div className="wd-detail-title-row">
        <h2 className="wd-detail-title">{shown}</h2>
        {canRename ? (
          <button
            className="wd-detail-rename"
            type="button"
            onClick={begin}
            aria-label={`Rename ${shown}`}
            title="Rename"
          >
            {/* The word, not a pencil glyph. It only appears on header hover, and a
                label is unambiguous where an icon has to be learned. */}
            Rename
          </button>
        ) : null}
        {error ? (
          <span className="wd-detail-rename-error" role="alert">
            {error}
          </span>
        ) : null}
      </div>
    );
  }

  return (
    <div className="wd-detail-title-row">
      <input
        className="wd-detail-title-input"
        aria-label="Session name"
        value={draft}
        autoFocus
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            commit();
          } else if (e.key === 'Escape') {
            e.preventDefault();
            cancel();
          }
        }}
      />
    </div>
  );
}

export function RadarDetailPanel({ agent, children = [], onJumpTo, onClose }: RadarDetailPanelProps) {
  const theme = radarHarness(agent.harness);
  // The radar's OWN derived heading (task label, else nickname, else the raw id):
  // kept separate from `agent.title`, the harness's own session name, so neither
  // overwrites the other.
  const heading = agent.label || agent.nickname || agent.id;
  const sessionTitle = agent.title ?? null;
  const subtitle = radarSubtitle(agent);

  // Accent is the flat harness hue — colour no longer encodes fill (that's the
  // globe's SIZE channel). CSS resolves `--heat` → `--harness` via its fallback.
  return (
    <aside
      className="wd-detail wd-radar-detail"
      style={{ '--harness': theme.color } as CSSProperties}
      aria-label={`Agent ${heading}`}
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
          <EditableTitle agentId={agent.id} heading={heading} />
          {sessionTitle ? <div className="wd-detail-session-name">{sessionTitle}</div> : null}
          {subtitle ? <div className="wd-detail-sub">{subtitle}</div> : null}
        </div>
        {onClose ? (
          <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close detail">
            ×
          </button>
        ) : null}
      </div>

      <AgentSummarySection agent={agent} />
      <ContextSection agent={agent} />
      <ActivitySection agent={agent} />
      <RosterSection children={children} onJumpTo={onJumpTo} />
      <IdentitySection agent={agent} />
    </aside>
  );
}

export default RadarDetailPanel;
