// ObservedRadarView.tsx: the ONE renderer for an `ObservedState` frame, whichever door it
// came through. The observer's peer view and the host's "preview what others see" control
// both mount this exact component: a preview that could drift from the real observer
// experience would be worse than no preview at all, so there is deliberately no second
// formatter anywhere in the frontend either.
//
// `ObservedAgent` carries no name, label, title, path, prompt or cost, and this component
// must never invent one to fill the gap. The header badge, the dashed hazard-stripe
// border and the raw hash ids are deliberate: this view has to read as someone else's
// machine at a glance, never as a slightly different local radar.

import type { CSSProperties } from 'react';
import type { ObservedAction, ObservedAgent, ObservedState } from '@/viz/shared/types/observedTypes';
import { harnessColor } from '@/viz/shared/theme/harnessColors';
import { humanizeSecs } from './observeFormat';

const STATUS_LABEL: Record<string, string> = {
  working: 'Working',
  idle: 'Idle',
  closed: 'Closed',
  terminated: 'Terminated',
};

function statusLabel(status: string): string {
  return STATUS_LABEL[status] ?? (status || 'Unknown');
}

const ACTION_KIND_GLYPH: Record<string, string> = {
  read: '▤',
  write: '◆',
  search: '⌕',
  run: '»',
  tool: '◈',
  message: '≡',
  thinking: '◌',
};

function actionGlyph(kind: string): string {
  return ACTION_KIND_GLYPH[kind] ?? '•';
}

/** First 8 chars of a salted hash, bracketed, so an id always reads as opaque data
 * rather than something that could be mistaken for a name. */
function shortId(id: string): string {
  return id ? `[${id.slice(0, 8)}]` : '[unknown]';
}

function pct(fill: number): string {
  return `${Math.round(fill * 100)}%`;
}

function CurrentActionBadge({ action }: { action: ObservedAction | null }) {
  if (!action) {
    return (
      <div className="wd-observe-action is-idle" data-current-action="idle">
        <span aria-hidden>·</span> No action in flight
      </div>
    );
  }
  return (
    <div className="wd-observe-action" data-current-action="active" data-kind={action.kind}>
      <span className={`wd-observe-action-glyph is-${action.kind}`} aria-hidden>
        {actionGlyph(action.kind)}
      </span>
      <span className="wd-observe-action-kind">{action.kind}</span>
      <span className="wd-observe-action-tool">{action.tool}</span>
      <span className="wd-observe-action-elapsed">{humanizeSecs(action.elapsedSecs)}</span>
    </div>
  );
}

function AgentCard({ agent }: { agent: ObservedAgent }) {
  const theme = harnessColor(agent.harness);
  return (
    <li
      className="wd-observe-agent"
      data-observed-agent={agent.id}
      data-depth={agent.depth}
      style={{ paddingLeft: `${10 + agent.depth * 16}px` } as CSSProperties}
    >
      <div className="wd-observe-agent-head">
        <span className="wd-observe-agent-glyph" aria-hidden style={{ color: theme.hue }}>
          {theme.glyph}
        </span>
        <span className="wd-observe-agent-harness">{theme.label}</span>
        <span className={`wd-observe-agent-status is-${agent.status}`}> · {statusLabel(agent.status)}</span>
        <span className="wd-observe-agent-id" title="Salted id, not the real session id">
          {shortId(agent.id)}
        </span>
      </div>

      <div className="wd-observe-agent-meta">
        <span>{agent.project ?? 'No project'}</span>
        {agent.role ? <span>{agent.role}</span> : null}
        <span>{agent.model ?? 'Unknown model'}</span>
        <span>up {humanizeSecs(agent.ageSecs)}</span>
        {agent.childCount > 0 ? <span>{agent.childCount} children</span> : null}
      </div>

      <CurrentActionBadge action={agent.currentAction} />

      <div className="wd-observe-gauge" data-context-gauge>
        <div className="wd-observe-gauge-track">
          <div className="wd-observe-gauge-fill" style={{ width: pct(agent.fillPct) }} />
        </div>
        <div className="wd-observe-gauge-meta">
          <span>{pct(agent.fillPct)} full</span>
          <span>{agent.contextTokens.toLocaleString()} tok</span>
        </div>
      </div>

      {agent.recentActivity.length > 0 ? (
        <ul className="wd-observe-activity">
          {agent.recentActivity.slice(0, 6).map((a, i) => (
            <li key={`${a.kind}-${i}`} data-activity-row>
              <span className={`wd-observe-activity-glyph is-${a.kind}`} aria-hidden>
                {actionGlyph(a.kind)}
              </span>
              <span>{a.kind}</span>
              <span className="wd-observe-activity-time">{humanizeSecs(a.secsAgo)} ago</span>
            </li>
          ))}
        </ul>
      ) : null}

      {agent.team ? (
        <div className="wd-observe-team" data-team>
          Team · {agent.team.memberCount} member{agent.team.memberCount === 1 ? '' : 's'}
          {agent.team.isLead ? ' · lead' : ''}
          {agent.team.memberType ? ` · ${agent.team.memberType}` : ''}
        </div>
      ) : null}
    </li>
  );
}

export type ObservedRadarViewProps = {
  /** null = no frame has landed yet (waiting), distinct from an empty agent list. */
  state: ObservedState | null;
  title: string;
  subtitle?: string | null;
};

/**
 * Renders exactly one `ObservedState` frame. Used verbatim by both the observer's peer
 * dock and the host's own "preview what others see" control (`PreviewModal.tsx`) so the
 * two can never drift apart.
 */
export function ObservedRadarView({ state, title, subtitle }: ObservedRadarViewProps) {
  return (
    <section className="wd-observe-radar" aria-label={title}>
      <header className="wd-observe-radar-head">
        <span className="wd-observe-radar-kicker">◈ Remote, read-only</span>
        <h3 className="wd-observe-radar-title">{title}</h3>
        {subtitle ? <p className="wd-observe-radar-sub">{subtitle}</p> : null}
        <p className="wd-observe-radar-note">
          Shapes and numbers only. No names, file paths, prompts, or costs ever leave the host.
        </p>
      </header>

      {state === null ? (
        <div className="wd-observe-empty" aria-live="polite">
          <span className="wd-observe-empty-pulse" aria-hidden />
          Waiting for the first frame...
        </div>
      ) : state.agents.length === 0 ? (
        <div className="wd-observe-empty">No active agents on this machine right now.</div>
      ) : (
        <>
          {state.truncated ? (
            <div className="wd-observe-truncated" role="status">
              Showing the first 64 agents; more are running than fit in one frame.
            </div>
          ) : null}
          <ul className="wd-observe-agent-list">
            {state.agents.map((a) => (
              <AgentCard agent={a} key={a.id} />
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

export default ObservedRadarView;
