import './style.css';
import { animate, stagger } from 'animejs';
import { mountWarRoom } from './viz/mount';
import { renderDiagnosis as paintDiagnosis, type Diagnosis, type Finding, type FixPreview, type ResolvedEvidence } from './diagnosis';
import { harnessTheme } from './viz/harnessTheme';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

type HarnessRollup = {
  harness: string;
  sessions: number;
  events: number;
};

// `query_profile` now returns a per-harness breakdown (Task 6). The three
// top-level counts are unchanged, so existing HUD usage is unaffected; the
// `by_harness` rollup is consumed by the harness-breakdown HUD in a later task.
type Profile = {
  session_count: number;
  event_count: number;
  finding_count: number;
  by_harness?: HarnessRollup[];
};

type IngestProgress = {
  phase: string;
  status: string;
  ingested_sessions?: number;
  ingested_events?: number;
  total_sessions?: number;
  total_events?: number;
  finding_count?: number;
  session_count?: number;
  event_count?: number;
};

type FuguDelta = { stage: string; delta?: string };
type FuguUsage = {
  stage: string;
  input_tokens?: number;
  output_tokens?: number;
  orchestration_input_tokens?: number;
  orchestration_output_tokens?: number;
};

// Task-6 viz event contract (consumed by the R3F bridge). `harness` is snake_case.
type CandidateNomination = {
  pattern_id: string;
  session_id: string;
  harness: string;
  severity_hint: number;
};
type CandidatesNominated = { candidates: CandidateNomination[] };
type FindingVerdict = {
  finding_id: string;
  pattern_id: string;
  harness: string;
  verdict: 'confirmed' | 'refuted';
  severity: number;
};

const screen = document.querySelector<HTMLDivElement>('#screen')!;
const form = document.querySelector<HTMLFormElement>('#prompt')!;
const input = document.querySelector<HTMLInputElement>('#command')!;
const runButton = document.querySelector<HTMLButtonElement>('#run-button')!;
const status = document.querySelector<HTMLDivElement>('#status')!;
const hudSessions = document.querySelector<HTMLDivElement>('#hud-sessions')!;
const hudEvents = document.querySelector<HTMLDivElement>('#hud-events')!;
const hudFindings = document.querySelector<HTMLDivElement>('#hud-findings')!;
const hudStage = document.querySelector<HTMLDivElement>('#hud-stage')!;
const appWindow = getCurrentWindow();
// Mount the R3F war-room island ONCE (pre-warmed on the hidden overlay window);
// every Tauri viz event below is routed into this bridge via `bridge.ingest`.
const bridge = mountWarRoom('war-room-root');

let running = false;
let latestStreamingLine: HTMLDivElement | null = null;

// Harness provenance for the diagnosis screen. The web `Finding` carries no
// harness field, but `candidates_nominated` tells us which harness each pattern
// was nominated from — so we tag each rendered hole with its REAL harness
// (color+glyph+label) instead of guessing. `scopeHarness` is the run's target
// harness, used as the fallback when a finding has no nomination on record.
const harnessByPattern = new Map<string, string>();
let scopeHarness = 'claude_code';

function harnessOf(f: Finding): string {
  return harnessByPattern.get(f.pattern_id) ?? scopeHarness;
}

function fetchFixPreview(findingId: string): Promise<FixPreview> {
  // Tauri v2 maps camelCase JS args → snake_case Rust params (`finding_id`).
  return invoke<FixPreview>('get_fix_preview', { findingId });
}

// READ-ONLY evidence fallback: when an EvidenceRef has no stored quote but names
// an event, the drill-down calls this to recover the excerpt from ground truth.
// camelCase args → snake_case Rust params (`session_id`, `event_id`).
function resolveEvidence(sessionId: string, eventId: string): Promise<ResolvedEvidence> {
  return invoke<ResolvedEvidence>('resolve_evidence', { sessionId, eventId });
}

function setStatus(text: string) {
  status.textContent = text;
  hudStage.textContent = text;
}

// Boot HUD per-harness breakdown (spec §5.3 step 1): render the `by_harness`
// rollup as a phosphor strip under the totals — "◆ 47 Claude · ▲ 12 Codex" —
// colour ALWAYS paired with the glyph + label (color-blind a11y).
function renderHarnessBreakdown(rollup: HarnessRollup[]): void {
  const live = rollup.filter(r => r.sessions > 0 || r.events > 0);
  if (live.length === 0) return;
  const strip = document.createElement('div');
  strip.className = 'line muted harness-breakdown';
  strip.append(document.createTextNode('  '));
  live.forEach((r, i) => {
    const t = harnessTheme(r.harness);
    if (i > 0) strip.append(document.createTextNode(' · '));
    const chip = document.createElement('span');
    chip.className = 'breakdown-chip';
    chip.style.setProperty('--harness', t.color);
    chip.setAttribute('aria-label', `${r.sessions} ${t.label} sessions`);
    const glyph = document.createElement('span');
    glyph.className = 'breakdown-glyph';
    glyph.setAttribute('aria-hidden', 'true');
    glyph.textContent = t.glyph;
    const label = document.createElement('span');
    label.className = 'breakdown-label';
    label.textContent = `${r.sessions.toLocaleString()} ${t.label}`;
    chip.append(glyph, label);
    strip.appendChild(chip);
  });
  strip.append(document.createTextNode(' sessions'));
  screen.appendChild(strip);
  screen.scrollTop = screen.scrollHeight;
}

function formatCount(value: number | undefined) {
  return typeof value === 'number' ? value.toLocaleString() : '—';
}

function updateHud(profile: Partial<Profile>) {
  if (typeof profile.session_count === 'number') hudSessions.textContent = formatCount(profile.session_count);
  if (typeof profile.event_count === 'number') hudEvents.textContent = formatCount(profile.event_count);
  if (typeof profile.finding_count === 'number') hudFindings.textContent = formatCount(profile.finding_count);
}

function line(text: string, cls = '') {
  const d = document.createElement('div');
  d.className = `line ${cls}`.trim();
  d.textContent = text;
  screen.appendChild(d);
  screen.scrollTop = screen.scrollHeight;
  return d;
}

function html(markup: string) {
  const d = document.createElement('div');
  d.innerHTML = markup;
  screen.appendChild(d);
  screen.scrollTop = screen.scrollHeight;
  return d;
}

async function boot() {
  line('▌ WARDEN v0.1 — mounting Claude Code memory spine…');
  setStatus('cold boot');

  try {
    const profile = await invoke<Profile>('query_profile');
    updateHud(profile);
    line(`  MEMORY online: ${profile.session_count} sessions · ${profile.event_count.toLocaleString()} events · ${profile.finding_count} findings`, 'muted');
    if (profile.by_harness?.length) renderHarnessBreakdown(profile.by_harness);
  } catch (e) {
    line(`  MEMORY cold: ${String(e)}`, 'warn');
  }

  try {
    const cached = await invoke<Diagnosis | null>('get_diagnosis');
    if (cached) {
      line(`  Last diagnosis cached: ${new Date(cached.created_at).toLocaleString()} · ${cached.ranked_findings.length} holes`, 'muted');
    }
  } catch {
    // The cache is convenience only; a missing row should not make the overlay feel broken.
  }

  line('  Ask: "what is wrong with how I use my agents?"');
  line('  Press Esc to dismiss. Press ⌘⇧Space to summon WARDEN.');
  line('  ');
  html('<span class="cursor" aria-hidden="true"></span>');
  animate('#terminal', { opacity: [0, 1], translateY: [12, 0], duration: 650, ease: 'outExpo' });
}

// Router → Diagnosis state. The cinematic slam-in is the R3F/Remotion reveal
// (driven separately by `diagnosis_ready`); here we paint the PERSISTENT,
// drill-downable readout into the terminal log via the pure `diagnosis.ts`
// renderer, tagging each hole with its real harness and wiring the read-only
// fix-preview fetch. A calm staggered fade-in echoes the reveal without
// re-staging it.
function renderDiagnosis(d: Diagnosis) {
  setStatus(d.detector_only ? 'detector-only diagnosis' : 'verified diagnosis ready');
  latestStreamingLine = null;

  const root = paintDiagnosis(screen, d, { harnessOf, fetchFixPreview, resolveEvidence });
  screen.scrollTop = screen.scrollHeight;

  const holes = root.querySelectorAll('.diag-hole');
  if (holes.length) {
    animate(holes, {
      opacity: [0, 1],
      translateX: [-18, 0],
      delay: stagger(90, { start: 80 }),
      duration: 460,
      ease: 'outExpo',
    });
  }
  animate(root.querySelector('.diag-header') ?? root, { opacity: [0, 1], duration: 380, ease: 'outQuad' });
}

function setRunning(value: boolean) {
  running = value;
  input.disabled = value;
  runButton.disabled = value;
  runButton.textContent = value ? 'RUNNING' : 'RUN';
}

form.addEventListener('submit', async ev => {
  ev.preventDefault();
  if (running) return;

  const q = input.value.trim();
  if (!q) return;

  line(`> ${q}`);
  setRunning(true);
  setStatus('ingesting transcripts');
  line('  Diagnostician entering war room…');
  // Fresh run: forget the previous run's nominations so harness tags reflect
  // THIS diagnosis only.
  harnessByPattern.clear();
  scopeHarness = 'claude_code';

  try {
    const d = await invoke<Diagnosis>('run_diagnosis', {
      scope: { harness: scopeHarness, query: q, force: false, max_files: null }
    });
    renderDiagnosis(d);
  } catch (e) {
    setStatus('diagnosis failed');
    line(String(e), 'bad');
  } finally {
    setRunning(false);
    input.focus();
  }
});

listen<IngestProgress>('ingest_progress', e => {
  const p = e.payload;
  // Tolerate both the live-tail shape `{ harness, path, events, phase:"live" }`
  // and the M1 batch shape `{ phase, status }`.
  const anyP = p as IngestProgress & { harness?: string; path?: string; events?: number };
  setStatus(p.status ? `${p.phase} · ${p.status}` : `${anyP.harness ?? 'ingest'} · ${p.phase}`);
  bridge.ingest('ingest_progress', anyP);

  if (p.status === 'complete' && p.phase === 'ingest') {
    updateHud({ session_count: p.total_sessions ?? 0, event_count: p.total_events ?? 0, finding_count: p.finding_count ?? 0 });
    line(`  EYES complete: ${formatCount(p.ingested_sessions)} new/changed sessions · ${formatCount(p.ingested_events)} events processed`, 'muted');
  }
  if (p.status === 'complete' && p.phase === 'featurize') {
    updateHud({ session_count: p.session_count ?? 0, event_count: p.event_count ?? 0 });
    line(`  MEMORY re-indexed: ${formatCount(p.session_count)} sessions · ${formatCount(p.event_count)} events`, 'muted');
  }
});

listen<{ phase: string; status: string }>('diagnosis_status', e => {
  setStatus(`${e.payload.phase} · ${e.payload.status}`);
});

// Task-6: candidate nominations spawn war-room nodes (real candidate count).
// We also record each pattern's source harness so the diagnosis screen can tag
// every hole with its REAL harness identity (color+glyph+label).
listen<CandidatesNominated>('candidates_nominated', e => {
  bridge.ingest('candidates_nominated', e.payload);
  const candidates = e.payload?.candidates ?? [];
  candidates.forEach(c => {
    if (c.pattern_id && c.harness) harnessByPattern.set(c.pattern_id, c.harness);
  });
  const n = candidates.length;
  setStatus(`nominated · ${n} candidate${n === 1 ? '' : 's'}`);
});

// Task-6: per-finding verdicts drive the core flare (confirmed) / collapse (refuted).
listen<FindingVerdict>('finding_verdict', e => {
  bridge.ingest('finding_verdict', e.payload);
  // Backstop the harness map: a verdict names the pattern + harness, so even if
  // a nomination event was missed the rendered hole still tags the right harness.
  if (e.payload.pattern_id && e.payload.harness) {
    harnessByPattern.set(e.payload.pattern_id, e.payload.harness);
  }
});

listen<{ id: string; finding_count: number }>('diagnosis_ready', e => {
  setStatus('diagnosis ready');
  hudFindings.textContent = e.payload.finding_count.toLocaleString();
  bridge.ingest('diagnosis_ready', e.payload);
});

listen<FuguDelta>('fugu_delta', e => {
  const delta = String(e.payload.delta || '');
  setStatus(`${e.payload.stage} · streaming`);
  bridge.ingest('fugu_delta', e.payload);

  if (!delta.trim()) return;
  const preview = delta.replace(/\s+/g, ' ').slice(0, 120);
  if (!latestStreamingLine || !latestStreamingLine.dataset.stage || latestStreamingLine.dataset.stage !== e.payload.stage) {
    latestStreamingLine = line(`  ${e.payload.stage}: ${preview}`, 'stream');
    latestStreamingLine.dataset.stage = e.payload.stage;
  } else {
    latestStreamingLine.textContent = `${latestStreamingLine.textContent}${preview}`.slice(0, 180);
  }
});

listen<FuguUsage>('fugu_usage', e => {
  const p = e.payload;
  bridge.ingest('fugu_usage', p);
  line(`  FUGU ${p.stage}: ${formatCount(p.input_tokens)} in · ${formatCount(p.output_tokens)} out · orchestration ${formatCount((p.orchestration_input_tokens ?? 0) + (p.orchestration_output_tokens ?? 0))}`, 'muted');
});

listen('warden_hotkey', () => {
  input.focus();
  setStatus('summoned');
  animate('#terminal', { scale: [0.985, 1], duration: 220, ease: 'outQuad' });
});

document.addEventListener('keydown', ev => {
  if (ev.key === 'Escape') {
    ev.preventDefault();
    // Dismiss via the daemon so it also restores click-through (idle state),
    // matching the tray + blur dismissal path. Fall back to a direct hide.
    invoke('hide_overlay').catch(() => appWindow.hide().catch(() => input.blur()));
  }
});

boot();
