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

import type { CSSProperties, KeyboardEvent } from 'react';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { RadarActivity, RadarAgent, RadarContextRow } from '@/viz/shared/types/radarTypes';
import { radarSubtitle, formatTokens as tokens } from '@/viz/shared/types/radarTypes';
import { awaitingLine } from '@/viz/shared/lib/awaitingCopy';
import { FilePreview } from '@/viz/shared/ui/FilePreview';
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
  awaiting: 'Waiting on you',
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
 * Where an agent's terminal window is, as the backend resolved it.
 * `reachable` is the only thing the button keys off; a false always carries a
 * `reason` written for the operator.
 */
export type TerminalTarget = {
  reachable: boolean;
  app: string | null;
  viaAgentId: string | null;
  viaLabel: string | null;
  reason: string | null;
};

function normalizeTarget(raw: any): TerminalTarget | null {
  if (!raw || typeof raw !== 'object') return null;
  return {
    reachable: raw.reachable === true,
    app: typeof raw.app === 'string' ? raw.app : null,
    viaAgentId: typeof raw.viaAgentId === 'string' ? raw.viaAgentId : null,
    viaLabel: typeof raw.viaLabel === 'string' ? raw.viaLabel : null,
    reason: typeof raw.reason === 'string' ? raw.reason : null,
  };
}

/**
 * "Take me there": raise the terminal window this agent is running in.
 *
 * The radar tells you which agent needs you; this is the step that was still
 * manual, hunting the right tab across a dozen open windows.
 *
 * Three behaviours are load-bearing:
 *
 * 1. **The button never claims what the backend cannot do.** It renders only for
 *    a target the backend already resolved as reachable. Every other case shows
 *    the backend's own one-line reason instead, so an IDE-hosted session, a
 *    closed window and an unsupported emulator read as three different facts
 *    rather than as one dead button.
 * 2. **The probe is silent when there is no backend.** The browser sandboxes
 *    (`/radar-lab.html`) have no Tauri bridge, so a rejected probe renders
 *    nothing at all rather than an error the harness cannot act on.
 * 3. **A permission refusal offers the only thing that fixes it.** macOS records
 *    the refusal and never asks again, so "try again" is a lie; the pane in
 *    System Settings is the sole remedy. `denied` comes back as its own field so
 *    this never depends on matching English in a message.
 */
function TakeMeThere({ agentId }: { agentId: string }) {
  const [target, setTarget] = useState<TerminalTarget | null>(null);
  const [failure, setFailure] = useState<{ message: string; denied: boolean } | null>(null);
  const [probing, setProbing] = useState(false);
  const [raising, setRaising] = useState(false);
  const tokenRef = useRef(0);
  // Did the LAST agent have a row here? The probe is a round trip, so between two
  // agents this control is empty for a frame or two, and everything below it in the
  // panel (the context gauge, activity, the roster) jumped 37px each time. Holding
  // the height while the next answer arrives is what stops that, and it is only worth
  // holding when a row is actually likely, which is exactly what this remembers.
  const hadRow = useRef(false);

  useEffect(() => {
    if (!agentId) return;
    const token = ++tokenRef.current;
    setFailure(null);
    setRaising(false);
    setProbing(true);
    // Cleared rather than left showing the PREVIOUS agent's window for one round
    // trip. A stale "Terminal" caption under a newly selected agent is the exact
    // kind of small lie this panel does not tell. The SPACE it occupied is held (see
    // `hadRow`); the claim it made is not.
    setTarget(null);
    invoke('agent_terminal_target', { agentId })
      .then((raw) => {
        if (tokenRef.current !== token) return;
        const next = normalizeTarget(raw);
        hadRow.current = next !== null && (next.reachable || next.reason !== null);
        setTarget(next);
        setProbing(false);
      })
      .catch(() => {
        if (tokenRef.current !== token) return;
        // No backend at all (the browser sandboxes). Render nothing, ever, and stop
        // reserving room for something that is never coming.
        hadRow.current = false;
        setTarget(null);
        setProbing(false);
      });
  }, [agentId]);

  if (!target) {
    return probing && hadRow.current ? (
      <div className="wd-detail-goto wd-detail-goto-held" aria-hidden />
    ) : null;
  }

  if (!target.reachable) {
    return target.reason ? (
      <p className="wd-detail-goto-blocked" data-goto="blocked">
        {target.reason}
      </p>
    ) : null;
  }

  function go() {
    // Raising a window is an AppleScript round trip through another process, so it is
    // not instant and it can be slow when the target is on another Space. Without a
    // pending state the only feedback is the 160ms press scale, and a raise that takes
    // 300ms reads as a dead button and gets clicked again. The guard is the same fact
    // stated for the backend: two raises in flight is one wasted.
    if (raising) return;
    const token = tokenRef.current;
    setFailure(null);
    setRaising(true);
    const done = () => {
      if (tokenRef.current === token) setRaising(false);
    };
    invoke<{ ok?: boolean; denied?: boolean; message?: string | null }>('focus_agent_terminal', {
      agentId,
    })
      .then((out) => {
        done();
        if (tokenRef.current !== token) return;
        if (out?.ok) return;
        setFailure({
          message: out?.message || 'WARDEN could not raise that window',
          denied: out?.denied === true,
        });
      })
      .catch((err: unknown) => {
        done();
        if (tokenRef.current !== token) return;
        setFailure({
          message: typeof err === 'string' ? err : 'WARDEN could not raise that window',
          denied: false,
        });
      });
  }

  // A subagent runs inside its root's process, so the window belongs to the
  // root. Saying whose window it is beats silently jumping somewhere the
  // operator did not select.
  const via = target.viaLabel ?? (target.viaAgentId ? 'its parent' : null);

  return (
    <div className="wd-detail-goto">
      <button
        type="button"
        className="wd-detail-goto-btn"
        onClick={go}
        data-goto="button"
        aria-busy={raising || undefined}
        // The visible label cannot say WHERE without getting long, and the caption
        // beside it is not part of the button's accessible name.
        aria-label={
          via
            ? `Take me there: the ${target.app} window running ${via}`
            : `Take me there: the ${target.app} window`
        }
      >
        <span className="wd-detail-goto-glyph" aria-hidden>
          ↗
        </span>
        Take me there
      </button>
      <span
        className="wd-detail-goto-app"
        // The rail is ~300px and the button eats ~128 of it, so a root with a real
        // project name overflows. It truncates rather than wrapping the row onto a
        // second line: a caption that reflows moves everything below it in the panel,
        // and the app name (the part that matters) is at the FRONT of the string.
        title={via ? `${target.app} · in ${via}` : target.app ?? undefined}
      >
        {/* "in <root>" rather than a bare name: "Terminal · warden" reads as two
            tags, and the point is that this raises somebody ELSE's window. */}
        {via ? `${target.app} · in ${via}` : target.app}
      </span>
      {failure ? (
        <p className="wd-detail-goto-error" role="alert">
          {failure.message}
          {failure.denied ? (
            <button
              type="button"
              className="wd-detail-goto-settings"
              onClick={() => {
                invoke('open_automation_settings').catch(() => {});
              }}
            >
              Open Settings
            </button>
          ) : null}
        </p>
      ) : null}
    </div>
  );
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

      {/* Above the callout and the gauge on purpose: when an agent is waiting on
          you, going to it is the whole response, and the numbers cannot change
          until you do. */}
      <TakeMeThere agentId={agent.id} />
      <AwaitingCallout agent={agent} />
      <ContextSection agent={agent} />
      <ActivitySection agent={agent} />
      <RosterSection children={children} onJumpTo={onJumpTo} />
      <IdentitySection agent={agent} />
    </aside>
  );
}

/**
 * The one thing an operator opening a WAITING agent needs: what it is waiting for.
 *
 * Sits above the context gauge because it outranks it. A blocked agent's token count is
 * not going to change until the block clears, so the numbers can wait their turn.
 *
 * When the block is an `AskUserQuestion`, the in-flight action already carries the
 * question itself, so the callout quotes it rather than paraphrasing. Nothing is
 * fabricated: with no action to read, it falls back to the closed-vocabulary line.
 */
function AwaitingCallout({ agent }: { agent: RadarAgent }) {
  if (agent.status !== 'awaiting') return null;
  const asked = agent.currentAction?.kind === 'ask' ? agent.currentAction.label : null;
  return (
    <section className="wd-detail-awaiting" role="status">
      <span className="wd-detail-awaiting-glyph" aria-hidden>
        ?
      </span>
      <div className="wd-detail-awaiting-body">
        <div className="wd-detail-awaiting-head">{awaitingLine(agent.awaitingReason)}</div>
        {asked ? <div className="wd-detail-awaiting-ask">{asked}</div> : null}
      </div>
    </section>
  );
}

export default RadarDetailPanel;
