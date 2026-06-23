import './style.css';
import { animate } from 'animejs';
import { WarRoom } from './warRoom';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

type Evidence = {
  session_id: string;
  turn_id?: string | null;
  event_id?: string | null;
  quote?: string | null;
  source_path?: string | null;
};

type Finding = {
  id: string;
  pattern_id: string;
  title: string;
  severity: number;
  frequency: number;
  confidence: number;
  est_cost_tokens: number;
  est_cost_minutes: number;
  rationale: string;
  evidence: Evidence[];
  verifier_verdict?: string | null;
};

type Diagnosis = {
  id: string;
  created_at: string;
  ranked_findings: Finding[];
  do_items: string[];
  stop_items: string[];
  narrative: string;
  detector_only: boolean;
};

type Profile = {
  session_count: number;
  event_count: number;
  finding_count: number;
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
const war = new WarRoom(document.querySelector<HTMLCanvasElement>('#war-room')!);

let running = false;
let latestStreamingLine: HTMLDivElement | null = null;

function setStatus(text: string) {
  status.textContent = text;
  hudStage.textContent = text;
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

function esc(s: string) {
  return s.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));
}

function shortId(value: string | undefined | null) {
  return value ? value.slice(0, 10) : 'unknown';
}

async function boot() {
  line('▌ WARDEN v0.1 — mounting Claude Code memory spine…');
  setStatus('cold boot');

  try {
    const profile = await invoke<Profile>('query_profile');
    updateHud(profile);
    line(`  MEMORY online: ${profile.session_count} sessions · ${profile.event_count.toLocaleString()} events · ${profile.finding_count} findings`, 'muted');
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
  line('  Press Esc to dismiss. Press ⌥Space to summon WARDEN.');
  line('  ');
  html('<span class="cursor" aria-hidden="true"></span>');
  animate('#terminal', { opacity: [0, 1], translateY: [12, 0], duration: 650, ease: 'outExpo' });
}

function renderDiagnosis(d: Diagnosis) {
  setStatus(d.detector_only ? 'detector-only diagnosis' : 'verified diagnosis ready');
  latestStreamingLine = null;
  line('');
  line('┌─ VERIFIED DIAGNOSIS ─────────────────────────────────────────┐');
  line(`│ ${d.narrative.slice(0, 76).padEnd(76)} │`);
  line('└───────────────────────────────────────────────────────────────┘');

  d.ranked_findings.forEach((f, idx) => {
    const pct = Math.max(8, Math.min(100, f.severity * 20));
    const evidence = (f.evidence || [])
      .slice(0, 3)
      .map(e => `${esc(shortId(e.session_id))}${e.quote ? ` — ${esc(e.quote.slice(0, 150))}` : ''}`)
      .join('<br/>');
    html(`<article class="hole">
      <div class="hole-head">
        <h3>HOLE #${idx + 1} — ${esc(f.title || f.pattern_id)}</h3>
        <span class="bar" style="--pct:${pct}%" title="severity ${f.severity}/5"></span>
      </div>
      <div>${esc(f.rationale)}</div>
      <div class="evidence">confidence ${(f.confidence * 100).toFixed(0)}% · seen ${(f.frequency * 100).toFixed(0)}% · est ${Math.round(f.est_cost_tokens).toLocaleString()} tokens / ${Math.round(f.est_cost_minutes)} min<br/>Evidence:<br/>${evidence || 'detector evidence unavailable'}</div>
      ${f.verifier_verdict ? `<div class="verdict">Verifier: ${esc(f.verifier_verdict.slice(0, 220))}</div>` : ''}
    </article>`);
  });

  if (d.do_items?.length) {
    line('DO:', 'muted');
    d.do_items.forEach(x => line(`  ✓ ${x}`));
  }
  if (d.stop_items?.length) {
    line('STOP:', 'warn');
    d.stop_items.forEach(x => line(`  ✗ ${x}`));
  }
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

  try {
    const d = await invoke<Diagnosis>('run_diagnosis', {
      scope: { harness: 'claude_code', query: q, force: false, max_files: null }
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
  setStatus(`${p.phase} · ${p.status}`);
  war.pulse({ stage: p.phase, input: p.ingested_events ?? p.event_count ?? 0, output: p.ingested_sessions ?? p.session_count ?? 0 });

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
  war.pulse({ stage: e.payload.phase, input: 128, output: 32 });
});

listen<{ id: string; finding_count: number }>('diagnosis_ready', e => {
  setStatus('diagnosis ready');
  hudFindings.textContent = e.payload.finding_count.toLocaleString();
});

listen<FuguDelta>('fugu_delta', e => {
  const delta = String(e.payload.delta || '');
  setStatus(`${e.payload.stage} · streaming`);
  war.pulse({ stage: e.payload.stage, output: delta.length });

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
  war.pulse({
    stage: p.stage,
    input: p.input_tokens,
    output: p.output_tokens,
    orchestrationInput: p.orchestration_input_tokens,
    orchestrationOutput: p.orchestration_output_tokens
  });
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
