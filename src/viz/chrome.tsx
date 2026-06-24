// chrome.tsx — the war-room's glass cockpit (the interface formerly known as the
// terminal). Everything the old green-on-black terminal showed now lives here as
// screen-space DOM over the 3D canvas: the WARDEN HUD + memory profile, the ask
// bar, a live pipeline rail (real Fugu stages + token weight + streamed
// reasoning), the orb inspector (hover preview + click-through detail with the
// read-only fix preview), the harness/severity legend, and an empty-state invite.
//
// It is a pure presentational layer: it reads a normalized `SceneState` + the
// resolved orb layout and calls back to WarRoom for the few user actions
// (ask, request fix, clear, dismiss). No Tauri, no Three — trivially reasoned about.

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { SceneState } from './bridge';
import { harnessTheme, severityColor } from './harnessTheme';
import type { LayoutNode, OrbIssue, OrbSceneModel } from './orbTypes';

export type FixPreview = {
  finding_id: string;
  pattern_id: string;
  target_path: string;
  diff: string;
  applied: boolean;
};

const PIPELINE_STAGES = ['Diagnostician', 'Coach', 'Verifier'] as const;
const DEFAULT_QUERY = "what's wrong with how I use my agents?";

function fmtCount(n: number | undefined): string {
  return typeof n === 'number' && Number.isFinite(n) ? Math.round(n).toLocaleString() : '—';
}
function pct(n: number): string {
  return Number.isFinite(n) ? `${Math.round(n * 100)}%` : '0%';
}
function compact(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k`;
  return String(Math.round(n));
}

// ── top HUD: identity + live memory profile ────────────────────────────────
function Hud({ scene, onDismiss }: { scene: SceneState; onDismiss: () => void }) {
  const p = scene.profile;
  return (
    <header className="wd-hud">
      <div className="wd-hud-brand">
        <span className="wd-sigil">WARDEN</span>
        <span className="wd-tag">the agent that watches your agents</span>
        {p && p.byHarness.length > 0 && (
          <div className="wd-harness-strip" aria-label="sessions by harness">
            {p.byHarness.map((r) => {
              const t = harnessTheme(r.harness);
              return (
                <span className="wd-harness-chip" key={r.harness} style={{ '--harness': t.color } as CSSProperties}>
                  <span className="wd-harness-glyph" aria-hidden="true">{t.glyph}</span>
                  {fmtCount(r.sessions)} {t.label}
                </span>
              );
            })}
          </div>
        )}
      </div>
      <div className="wd-hud-metrics">
        <Metric label="sessions" value={fmtCount(p?.sessions)} />
        <Metric label="events" value={fmtCount(p?.events)} />
        <Metric label="findings" value={fmtCount(p?.findings)} />
        <button className="wd-dismiss" type="button" onClick={onDismiss} aria-label="Dismiss WARDEN (Esc)" title="Dismiss · Esc">
          ✕
        </button>
      </div>
    </header>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="wd-metric">
      <span className="wd-metric-val">{value}</span>
      <span className="wd-metric-key">{label}</span>
    </div>
  );
}

// ── bottom console: ask bar + live pipeline rail ───────────────────────────
function Console({
  scene,
  running,
  error,
  onAsk,
}: {
  scene: SceneState;
  running: boolean;
  error: string | null;
  onAsk: (q: string) => void;
}) {
  const [value, setValue] = useState(DEFAULT_QUERY);
  const inputRef = useRef<HTMLInputElement>(null);

  // Summon focuses the ask bar so the user can type over the suggestion at once.
  useEffect(() => {
    if (scene.summoned) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [scene.summoned]);

  return (
    <div className="wd-console">
      {(running || scene.stream) && <PipelineRail scene={scene} running={running} />}
      <form
        className="wd-ask"
        onSubmit={(e) => {
          e.preventDefault();
          const q = value.trim();
          if (q && !running) onAsk(q);
        }}
      >
        <span className="wd-ask-chevron" aria-hidden="true">▸</span>
        <input
          ref={inputRef}
          className="wd-ask-input"
          value={value}
          spellCheck={false}
          disabled={running}
          aria-label="Ask WARDEN about your agent habits"
          onChange={(e) => setValue(e.target.value)}
        />
        <button className="wd-ask-run" type="submit" disabled={running}>
          {running ? 'DIAGNOSING' : 'DIAGNOSE'}
        </button>
      </form>
      {error && <div className="wd-ask-error">{error}</div>}
    </div>
  );
}

function PipelineRail({ scene, running }: { scene: SceneState; running: boolean }) {
  const activeIndex = PIPELINE_STAGES.indexOf((scene.stage ?? '') as (typeof PIPELINE_STAGES)[number]);
  return (
    <div className="wd-pipeline" role="status" aria-live="polite">
      <div className="wd-pipeline-stages">
        {PIPELINE_STAGES.map((stage, i) => {
          const usage = scene.usage[stage];
          const tokens = usage ? usage.in + usage.out : 0;
          const orchestrated = usage ? usage.orchIn + usage.orchOut > 0 : false;
          const isActive = running && stage === scene.stage;
          const isDone = activeIndex > i || (tokens > 0 && !isActive);
          const cls = isActive ? 'is-active' : isDone ? 'is-done' : 'is-pending';
          return (
            <div className={`wd-stage ${cls}`} key={stage}>
              <span className="wd-stage-dot" aria-hidden="true" />
              <span className="wd-stage-name">{stage}</span>
              {tokens > 0 && (
                <span className="wd-stage-tokens" title={orchestrated ? 'orchestration tokens (Fugu)' : 'tokens'}>
                  {compact(tokens)}
                  {orchestrated ? ' ✦' : ''}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {scene.stream?.text && (
        <div className="wd-stream">
          <span className="wd-stream-stage">{scene.stream.stage}</span>
          <span className="wd-stream-text">{scene.stream.text}</span>
        </div>
      )}
    </div>
  );
}

// ── bottom status deck: a READ-ONLY instrument readout ───────────────────────
// Harnesses present · live telemetry (habits/agents/sessions/events/findings) ·
// the severity ramp · a "watching" pulse. There are no inputs — the conversational
// "ask WARDEN" surface is a separate chat interface, added later. pointer-events
// are off in CSS so the deck never intercepts the orbit camera. Honest-viz: every
// figure is a real field (counts fall back to "—" rather than fabricating).
function StatusDeck({ scene, model }: { scene: SceneState; model: OrbSceneModel }) {
  const p = scene.profile;
  const agents = model.agents.length
    ? model.agents
    : [{ id: 'unknown', harness: 'unknown', label: 'Unknown', glyph: '●', color: '#76ff9d', sessions: 0, eventCount: 0, totalLoad: 0 }];
  const habits = model.issues.length;
  const ramp: Array<[string, number]> = [
    ['low', 2],
    ['watch', 3],
    ['high', 4],
    ['critical', 5],
  ];
  const phaseLabel = scene.running ? 'scanning' : scene.phase === 'reveal' ? 'diagnosis ready' : 'watching';
  return (
    <div className="wd-deck" role="status" aria-label="WARDEN status">
      <div className="wd-deck-group">
        {agents.map((a) => {
          const t = harnessTheme(a.harness);
          return (
            <span className="wd-deck-harness" key={a.id} style={{ '--harness': t.color } as CSSProperties}>
              <span className="wd-deck-glyph" aria-hidden="true">{t.glyph}</span>
              {t.label}
            </span>
          );
        })}
      </div>
      <span className="wd-deck-div" aria-hidden="true" />
      <div className="wd-deck-group wd-deck-stats">
        <DeckStat value={String(habits)} label={habits === 1 ? 'habit' : 'habits'} />
        <DeckStat value={String(model.agents.length)} label={model.agents.length === 1 ? 'agent' : 'agents'} />
        <DeckStat value={fmtCount(p?.sessions)} label="sessions" />
        <DeckStat value={fmtCount(p?.events)} label="events" />
        <DeckStat value={fmtCount(p?.findings)} label="findings" />
      </div>
      <span className="wd-deck-div" aria-hidden="true" />
      <div className="wd-deck-group wd-deck-ramp" aria-label="severity ramp">
        <span className="wd-deck-ramp-key">severity</span>
        {ramp.map(([label, sev]) => (
          <span className="wd-deck-swatch" key={label} title={label} style={{ background: severityColor(sev) }} />
        ))}
      </div>
      <span className="wd-deck-div" aria-hidden="true" />
      <div className="wd-deck-group wd-deck-live">
        <span className={`wd-deck-pulse${scene.running ? ' is-live' : ''}`} aria-hidden="true" />
        <span className="wd-deck-phase">{phaseLabel}</span>
      </div>
    </div>
  );
}

function DeckStat({ value, label }: { value: string; label: string }) {
  return (
    <span className="wd-deck-stat">
      <span className="wd-deck-stat-val">{value}</span>
      <span className="wd-deck-stat-key">{label}</span>
    </span>
  );
}

// ── hover preview (screen-space card) ──────────────────────────────────────
function PreviewCard({ node }: { node: LayoutNode }) {
  const t = harnessTheme(node.harness);
  return (
    <div className="wd-preview" style={{ '--harness': t.color } as CSSProperties}>
      <div className="wd-card-kicker">
        <span className="wd-card-glyph">{t.glyph}</span>
        {node.kind === 'hub' ? t.label : node.issue?.title}
      </div>
      {node.kind === 'hub' ? (
        <div className="wd-card-main">
          {fmtCount(node.agent?.sessions)} sessions · {fmtCount(node.agent?.totalLoad)} total load
        </div>
      ) : (
        <div className="wd-card-main">
          {t.label} · ×{fmtCount(node.issue?.count)} · severity {node.issue?.severity ?? 0}/5
        </div>
      )}
      {node.issue?.rationale && <div className="wd-card-note">{node.issue.rationale}</div>}
      <div className="wd-card-hint">click to dive in</div>
    </div>
  );
}

// ── selected detail (the drill-in: real WARDEN data + read-only fix preview) ─
function DetailPanel({
  node,
  model,
  fixPreview,
  loadingFix,
  onRequestFix,
  onClose,
}: {
  node: LayoutNode;
  model: OrbSceneModel;
  fixPreview?: FixPreview;
  loadingFix: boolean;
  onRequestFix: (issue: OrbIssue) => void;
  onClose: () => void;
}) {
  const t = harnessTheme(node.harness);

  if (node.kind === 'hub') {
    const issues = model.issues.filter((i) => i.agentId === node.id).sort((a, b) => b.count - a.count);
    const worst = issues[0];
    return (
      <aside className="wd-detail wd-detail-hub" style={{ '--harness': t.color } as CSSProperties}>
        <DetailHead kicker={`${t.glyph} ${t.label}`} title={`${t.label} workload`} onClose={onClose} />
        <div className="wd-detail-ledger">
          <span>{fmtCount(node.agent?.sessions)} sessions</span>
          <span>{fmtCount(issues.length)} issue types</span>
          <span>{fmtCount(node.agent?.totalLoad)} total load</span>
        </div>
        <p>{worst ? `Worst habit: ${worst.title} (×${worst.count}).` : 'Clean hub: no detected issues for this agent.'}</p>
        {issues.length > 0 && (
          <ul className="wd-hub-issues">
            {issues.slice(0, 6).map((i) => (
              <li key={i.id}>
                <span className="wd-hub-sev" style={{ background: severityColor(i.severity) }} />
                {i.title}
                <span className="wd-hub-count">×{i.count}</span>
              </li>
            ))}
          </ul>
        )}
      </aside>
    );
  }

  const issue = node.issue!;
  return (
    <aside
      className="wd-detail"
      style={{ '--harness': t.color, '--severity': severityColor(issue.severity) } as CSSProperties}
    >
      <DetailHead kicker={`${t.glyph} ${t.label} · ${issue.patternId}`} title={issue.title} onClose={onClose} />
      <div className="wd-severity" aria-label={`severity ${issue.severity} of 5`}>
        {Array.from({ length: 5 }, (_, i) => (
          <span key={i} className={i < issue.severity ? 'on' : ''} />
        ))}
      </div>
      <p>{issue.rationale}</p>
      <div className="wd-detail-ledger">
        <span>×{fmtCount(issue.count)} occurrences</span>
        <span>{fmtCount(issue.estCostTokens)} tokens</span>
        <span>{fmtCount(issue.estCostMinutes)} min</span>
        <span>{pct(issue.frequency)} freq</span>
        <span>{pct(issue.confidence)} conf</span>
      </div>
      <div className="wd-detail-where">
        <div className="wd-card-kicker">where</div>
        <div>{issue.sessionIds.map((id) => id.slice(0, 10)).join(' · ') || 'no session ids stored'}</div>
      </div>
      {(model.guidance.doItems.length > 0 || model.guidance.stopItems.length > 0) && (
        <div className="wd-guidance">
          {model.guidance.doItems[0] && (
            <div className="wd-guide-do">
              <span>DO</span>
              {model.guidance.doItems[0]}
            </div>
          )}
          {model.guidance.stopItems[0] && (
            <div className="wd-guide-stop">
              <span>STOP</span>
              {model.guidance.stopItems[0]}
            </div>
          )}
        </div>
      )}
      <button className="wd-fix-button" type="button" onClick={() => onRequestFix(issue)} disabled={loadingFix}>
        {loadingFix ? 'LOADING PREVIEW' : 'FIX PREVIEW (read-only)'}
      </button>
      {fixPreview && (
        <pre className="wd-fix-diff">{fixPreview.diff || 'No diff: this guardrail already appears to be present.'}</pre>
      )}
    </aside>
  );
}

function DetailHead({ kicker, title, onClose }: { kicker: string; title: string; onClose: () => void }) {
  return (
    <div className="wd-detail-head">
      <div>
        <div className="wd-card-kicker">{kicker}</div>
        <h2 className="wd-detail-title">{title}</h2>
      </div>
      <button className="wd-detail-close" type="button" onClick={onClose} aria-label="Close detail">
        ✕
      </button>
    </div>
  );
}

function EmptyState({ running }: { running: boolean }) {
  if (running) return null;
  return (
    <div className="wd-empty">
      <div className="wd-empty-card">
        <div className="wd-empty-kicker">no habits mapped yet</div>
        <p>
          Ask WARDEN what's wrong with how you use your agents. It reads your local Claude &amp; Codex transcripts and
          maps every recurring habit as an orb you can explore.
        </p>
        <div className="wd-empty-hint">type below · press DIAGNOSE</div>
      </div>
    </div>
  );
}

export function Chrome({
  scene,
  model,
  hoveredNode,
  selectedNode,
  running,
  error,
  fixPreview,
  loadingFix,
  onAsk,
  onRequestFix,
  onClearSelection,
  onDismiss,
}: {
  scene: SceneState;
  model: OrbSceneModel;
  hoveredNode: LayoutNode | null;
  selectedNode: LayoutNode | null;
  running: boolean;
  error: string | null;
  fixPreview?: FixPreview;
  loadingFix: boolean;
  onAsk: (q: string) => void;
  onRequestFix: (issue: OrbIssue) => void;
  onClearSelection: () => void;
  onDismiss: () => void;
}) {
  // The bottom is a read-only StatusDeck (no inputs). The top HUD + conversational
  // ask bar are intentionally not rendered yet — Hud/Console/EmptyState stay
  // defined and wired so the later chat interface drops straight in.
  return (
    <div className="wd-chrome">
      <StatusDeck scene={scene} model={model} />

      <div className={`wd-inspector ${selectedNode || hoveredNode ? 'is-open' : ''}`}>
        {selectedNode ? (
          <DetailPanel
            node={selectedNode}
            model={model}
            fixPreview={fixPreview}
            loadingFix={loadingFix}
            onRequestFix={onRequestFix}
            onClose={onClearSelection}
          />
        ) : hoveredNode ? (
          <PreviewCard node={hoveredNode} />
        ) : null}
      </div>
    </div>
  );
}

export default Chrome;
