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

import { Fragment, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { SceneState } from './bridge';
import { harnessTheme, severityColor } from './harnessTheme';
import type { LayoutNode, OrbIssue, OrbSceneModel } from './orbTypes';
import type { ConstellationTab } from './NavBar';

export type FixPreview = {
  finding_id: string;
  pattern_id: string;
  target_path: string;
  diff: string;
  applied: boolean;
};

// The reversible write record (M4 Forge). Mirrors the FROZEN IPC CONTRACT exactly
// — these camelCase names are what the backend serializes for every artifact
// command (stage / apply / revert / get / list). Drive ALL applied/reverted UI
// from `status`; never fabricate it client-side.
export type ArtifactStatus = 'pending' | 'applied' | 'reverted';
export type Artifact = {
  id: string;
  findingId: string | null;
  kind: string;
  targetPath: string;
  diff: string;
  block: string;
  status: ArtifactStatus;
  appliedAt: string | null;
  backupPath: string | null;
  preImageSha256: string | null;
  // SHA-256 of what WARDEN wrote at apply time. Revert refuses if the target has
  // drifted from this, so the user's out-of-band edits are never clobbered.
  postImageSha256: string | null;
};

// ── pure diff-line classification (unit-tested) ──────────────────────────────
// The unified diff is DISPLAY-ONLY; we only colour it. Each line maps to one
// phosphor role: add-lines blaze green, removals burn amber, hunk headers dim,
// file headers faint, context plain. Pure so the colour logic is testable
// without a DOM.
export type DiffLineKind = 'add' | 'del' | 'hunk' | 'file' | 'ctx';

export function classifyDiffLine(line: string): DiffLineKind {
  if (line.startsWith('+++') || line.startsWith('---')) return 'file';
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return 'ctx';
}

const DIFF_LINE_CLASS: Record<DiffLineKind, string> = {
  add: 'wd-diff-add',
  del: 'wd-diff-del',
  hunk: 'wd-diff-hunk',
  file: 'wd-diff-file',
  ctx: 'wd-diff-ctx',
};

// Path tail for the provenance header — show the resolved home (~) + the last
// two segments so the operator reads "…/.claude/CLAUDE.md" without a wall of
// absolute path. The full absolute path stays in the title attribute.
export function provenanceLabel(targetPath: string): string {
  if (!targetPath) return 'unknown target';
  const parts = targetPath.split('/').filter(Boolean);
  if (parts.length <= 2) return targetPath;
  return `…/${parts.slice(-2).join('/')}`;
}

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

// ── breadcrumb: the radar focus path (Overview › agent › …) ──────────────────
// Renders from WarRoom's `focusStack` (agent ids). "Overview" clears focus;
// each crumb pops focus to that depth. Renders nothing when the stack is empty
// (Overview is implicit then). Labels are looked up from the live scene model
// so the trail reads as agent names, not raw ids.
function Breadcrumb({
  focusStack,
  model,
  onPopFocus,
  onClearFocus,
}: {
  focusStack: string[];
  model: OrbSceneModel;
  onPopFocus: (index: number) => void;
  onClearFocus: () => void;
}) {
  if (focusStack.length === 0) return null;
  const labelFor = (id: string) => model.agents.find((a) => a.id === id)?.label ?? id;
  return (
    <nav className="wd-breadcrumb" aria-label="Focus path">
      <button type="button" className="wd-crumb wd-crumb-root" onClick={onClearFocus} title="Back to overview">
        Overview
      </button>
      {focusStack.map((id, i) => {
        const last = i === focusStack.length - 1;
        return (
          <Fragment key={`${id}-${i}`}>
            <span className="wd-crumb-sep" aria-hidden="true">›</span>
            <button
              type="button"
              className={`wd-crumb${last ? ' is-current' : ''}`}
              aria-current={last ? 'location' : undefined}
              onClick={() => onPopFocus(i)}
              title={`Focus ${labelFor(id)}`}
            >
              {labelFor(id)}
            </button>
          </Fragment>
        );
      })}
    </nav>
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

// ── line-typed unified diff (display-only, phosphor-coloured) ────────────────
// Upgrades the flat <pre> to per-line typed spans so add-lines blaze green on
// the phosphor bg. Pure render off `classifyDiffLine`; no state.
function DiffView({ diff }: { diff: string }) {
  const lines = diff.length ? diff.split('\n') : [];
  return (
    <pre className="wd-fix-diff" data-fix-diff aria-label="proposed unified diff">
      {lines.length === 0 ? (
        <span className="wd-diff-ctx">No diff — this guardrail is already present.</span>
      ) : (
        lines.map((line, i) => {
          const kind = classifyDiffLine(line);
          return (
            <span className={DIFF_LINE_CLASS[kind]} data-diff-kind={kind} key={i}>
              {line || ' '}
              {'\n'}
            </span>
          );
        })
      )}
    </pre>
  );
}

// ── the apply / diff / revert approval flow (M4 Forge) ───────────────────────
// The cinematic write-approval surface, rendered below the diff. Panel-by-panel:
//   (1) provenance header — the resolved absolute CLAUDE.md path, so the write is
//       never a surprise;
//   (2) the line-typed diff;
//   (3) the action row — APPLY (acid CTA, amber-pulse on hover = "this writes")
//       when pending; a locked APPLIED badge + REVERT once `status === 'applied'`.
// Every visible state is driven by the real `Artifact.status`, never faked. When
// the staged diff is empty the block is already-applied and Apply is inert.
function FixApprovalBlock({
  preview,
  artifact,
  applying,
  reverting,
  onApply,
  onRevert,
}: {
  preview: FixPreview;
  artifact?: Artifact;
  applying: boolean;
  reverting: boolean;
  onApply: () => void;
  onRevert: () => void;
}) {
  // The resolved write target — prefer the artifact's recorded path (the row the
  // backend actually wrote), fall back to the preview's resolved path.
  const target = artifact?.targetPath || preview.target_path;
  const diff = artifact?.diff ?? preview.diff;
  const status: ArtifactStatus | 'preview' = artifact?.status ?? 'preview';
  const applied = status === 'applied';
  const reverted = status === 'reverted';
  // Empty diff at stage time = the guardrail is already in the file. Apply is a
  // harmless no-op, so present it as inert "ALREADY APPLIED" rather than a CTA.
  const emptyDiff = (artifact?.diff ?? preview.diff).trim().length === 0;
  const browserQa = target === 'WARDEN overlay';

  return (
    <div
      className={`wd-fix-foot is-${status}`}
      data-fix-foot
      data-status={status}
    >
      <div className="wd-fix-target" data-fix-target title={target}>
        <span className="wd-fix-target-glyph" aria-hidden="true">⌖</span>
        <span className="wd-fix-target-label">writes to</span>
        <code className="wd-fix-target-path">{target}</code>
      </div>

      <DiffView diff={diff} />

      {browserQa ? (
        <div className="wd-fix-note" data-fix-note>
          Preview only — this browser QA stage never writes or applies fixes.
        </div>
      ) : applied ? (
        <div className="wd-fix-applied" data-fix-applied>
          <div className="wd-fix-applied-badge" data-applied-badge>
            <span className="wd-fix-lock" aria-hidden="true">⬢</span>
            <span className="wd-fix-applied-text">
              GUARDRAIL APPLIED
              {artifact?.appliedAt ? <em className="wd-fix-applied-at">{shortStamp(artifact.appliedAt)}</em> : null}
            </span>
          </div>
          <button
            type="button"
            className="wd-fix-revert"
            data-fix-revert
            onClick={onRevert}
            disabled={reverting}
          >
            {reverting ? 'REVERTING…' : 'REVERT'}
          </button>
        </div>
      ) : (
        <button
          type="button"
          className="wd-fix-apply"
          data-fix-apply
          onClick={onApply}
          disabled={applying || emptyDiff}
          aria-label={emptyDiff ? 'Guardrail already applied' : 'Apply guardrail to your agent config'}
        >
          {applying ? 'APPLYING…' : emptyDiff ? 'ALREADY APPLIED' : reverted ? 'RE-APPLY GUARDRAIL' : 'APPLY GUARDRAIL'}
        </button>
      )}
    </div>
  );
}

// Compact RFC3339 → "Jun 25 · 14:32" for the applied badge. Never NaN: an
// unparseable stamp yields '' so the badge simply omits it.
export function shortStamp(iso: string): string {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return '';
  const d = new Date(t);
  const mon = d.toLocaleString('en-US', { month: 'short' });
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `${mon} ${d.getDate()} · ${hh}:${mm}`;
}

// ── the guardrail ledger (history of applied + reverted writes) ──────────────
// A visible audit trail: every artifact that ever reached applied/reverted, so
// the operator can see (and undo) what WARDEN wrote. Pure off `list_artifacts`
// data threaded from WarRoom. Reverting from here re-points the same revert path.
function isHistoric(a: Artifact): boolean {
  return a.status === 'applied' || a.status === 'reverted';
}

function Ledger({
  artifacts,
  reverting,
  onRevert,
  onClose,
}: {
  artifacts: Artifact[];
  reverting: boolean;
  onRevert: (id: string) => void;
  onClose: () => void;
}) {
  const rows = artifacts.filter(isHistoric);
  return (
    <aside className="wd-ledger" data-ledger>
      <div className="wd-ledger-head">
        <div className="wd-card-kicker">guardrail ledger</div>
        <button type="button" className="wd-detail-close" onClick={onClose} aria-label="Close ledger">✕</button>
      </div>
      {rows.length === 0 ? (
        <div className="wd-ledger-empty" data-ledger-empty>
          No guardrails written yet. Apply a fix to start the trail.
        </div>
      ) : (
        <ul className="wd-ledger-list">
          {rows.map((a) => (
            <li className={`wd-ledger-row is-${a.status}`} data-ledger-row data-status={a.status} key={a.id}>
              <span className="wd-ledger-state" aria-hidden="true">
                {a.status === 'applied' ? '⬢' : '↺'}
              </span>
              <div className="wd-ledger-meta">
                <code className="wd-ledger-path" title={a.targetPath}>{provenanceLabel(a.targetPath)}</code>
                <span className="wd-ledger-sub">
                  {a.status === 'applied' ? 'applied' : 'reverted'}
                  {a.appliedAt ? ` · ${shortStamp(a.appliedAt)}` : ''}
                </span>
              </div>
              {a.status === 'applied' ? (
                <button
                  type="button"
                  className="wd-ledger-revert"
                  onClick={() => onRevert(a.id)}
                  disabled={reverting}
                >
                  revert
                </button>
              ) : (
                <span className="wd-ledger-tag">reverted</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}

// ── selected detail (the drill-in: real WARDEN data + read-only fix preview) ─
function DetailPanel({
  node,
  model,
  fixPreview,
  loadingFix,
  artifact,
  applying,
  reverting,
  onRequestFix,
  onApplyFix,
  onRevertFix,
  onClose,
}: {
  node: LayoutNode;
  model: OrbSceneModel;
  fixPreview?: FixPreview;
  loadingFix: boolean;
  /** The reversible write record for the open finding (M4). Drives applied/revert UI. */
  artifact?: Artifact;
  applying: boolean;
  reverting: boolean;
  onRequestFix: (issue: OrbIssue) => void;
  onApplyFix: (issue: OrbIssue) => void;
  onRevertFix: (id: string) => void;
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
      {!fixPreview && (
        <button className="wd-fix-button" type="button" onClick={() => onRequestFix(issue)} disabled={loadingFix}>
          {loadingFix ? 'LOADING PREVIEW' : 'PREVIEW GUARDRAIL'}
        </button>
      )}
      {fixPreview && (
        <FixApprovalBlock
          preview={fixPreview}
          artifact={artifact}
          applying={applying}
          reverting={reverting}
          onApply={() => onApplyFix(issue)}
          onRevert={() => artifact && onRevertFix(artifact.id)}
        />
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
  tab,
  hoveredNode,
  selectedNode,
  focusStack,
  running,
  error,
  fixPreview,
  loadingFix,
  artifact,
  artifacts,
  applying,
  reverting,
  ledgerOpen,
  onAsk,
  onRequestFix,
  onApplyFix,
  onRevertFix,
  onToggleLedger,
  onClearSelection,
  onDismiss,
  onPopFocus,
  onClearFocus,
}: {
  scene: SceneState;
  model: OrbSceneModel;
  tab: ConstellationTab;
  hoveredNode: LayoutNode | null;
  selectedNode: LayoutNode | null;
  focusStack: string[];
  running: boolean;
  error: string | null;
  fixPreview?: FixPreview;
  loadingFix: boolean;
  /** Reversible write record for the open finding (M4 Forge). */
  artifact?: Artifact;
  /** Full guardrail history (applied + reverted) for the ledger. */
  artifacts: Artifact[];
  applying: boolean;
  reverting: boolean;
  ledgerOpen: boolean;
  onAsk: (q: string) => void;
  onRequestFix: (issue: OrbIssue) => void;
  onApplyFix: (issue: OrbIssue) => void;
  onRevertFix: (id: string) => void;
  onToggleLedger: () => void;
  onClearSelection: () => void;
  onDismiss: () => void;
  onPopFocus: (index: number) => void;
  onClearFocus: () => void;
}) {
  const ledgerCount = artifacts.filter(isHistoric).length;
  // Chrome now carries only the radar focus trail (Breadcrumb, top of the chrome
  // on the Radar tab) and the orb inspector. The emphasis filter is its own
  // bottom-centre `FilterBar` dock and the old bottom `StatusDeck` was removed
  // (its agent count now lives in the roster Sidebar header). The HUD +
  // conversational ask bar stay defined-but-dormant for the later chat interface.
  return (
    <div className="wd-chrome">
      {tab === 'radar' && (
        <Breadcrumb
          focusStack={focusStack}
          model={model}
          onPopFocus={onPopFocus}
          onClearFocus={onClearFocus}
        />
      )}

      <div className={`wd-inspector ${selectedNode || hoveredNode ? 'is-open' : ''}`}>
        {selectedNode ? (
          <DetailPanel
            node={selectedNode}
            model={model}
            fixPreview={fixPreview}
            loadingFix={loadingFix}
            artifact={artifact}
            applying={applying}
            reverting={reverting}
            onRequestFix={onRequestFix}
            onApplyFix={onApplyFix}
            onRevertFix={onRevertFix}
            onClose={onClearSelection}
          />
        ) : hoveredNode ? (
          <PreviewCard node={hoveredNode} />
        ) : null}
      </div>

      {/* Guardrail ledger — a visible, reversible history of every write WARDEN
          made to your agent config. Toggle lives bottom-right; the panel slides
          up from the corner. Counts only applied/reverted artifacts (honest). */}
      <button
        type="button"
        className={`wd-ledger-toggle${ledgerOpen ? ' is-open' : ''}`}
        aria-expanded={ledgerOpen}
        aria-controls="wd-ledger"
        title="Guardrail ledger"
        onClick={onToggleLedger}
      >
        <span className="wd-ledger-toggle-glyph" aria-hidden="true">⬢</span>
        LEDGER
        {ledgerCount > 0 ? <span className="wd-ledger-toggle-count">{ledgerCount}</span> : null}
      </button>
      <div className={`wd-ledger-dock${ledgerOpen ? ' is-open' : ''}`} id="wd-ledger">
        {ledgerOpen ? (
          <Ledger
            artifacts={artifacts}
            reverting={reverting}
            onRevert={onRevertFix}
            onClose={onToggleLedger}
          />
        ) : null}
      </div>
    </div>
  );
}

export default Chrome;
