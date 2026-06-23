import './style.css';
import anime from 'animejs';
import { WarRoom } from './warRoom';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type Evidence = { session_id: string; turn_id?: string | null; event_id?: string | null; quote?: string | null; source_path?: string | null };
type Finding = { id: string; pattern_id: string; title: string; severity: number; frequency: number; confidence: number; est_cost_tokens: number; est_cost_minutes: number; rationale: string; evidence: Evidence[]; verifier_verdict?: string | null };
type Diagnosis = { id: string; created_at: string; ranked_findings: Finding[]; do_items: string[]; stop_items: string[]; narrative: string; detector_only: boolean };

const screen = document.querySelector<HTMLDivElement>('#screen')!;
const form = document.querySelector<HTMLFormElement>('#prompt')!;
const input = document.querySelector<HTMLInputElement>('#command')!;
const status = document.querySelector<HTMLDivElement>('#status')!;
const war = new WarRoom(document.querySelector<HTMLCanvasElement>('#war-room')!);

function line(text: string, cls = '') {
  const d = document.createElement('div');
  d.className = `line ${cls}`;
  d.textContent = text;
  screen.appendChild(d);
  screen.scrollTop = screen.scrollHeight;
}
function html(markup: string) { const d = document.createElement('div'); d.innerHTML = markup; screen.appendChild(d); screen.scrollTop = screen.scrollHeight; }
function esc(s: string) { return s.replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]!)); }

async function boot() {
  line('▌ WARDEN v0.1 — mounting Claude Code memory spine…');
  try {
    const profile = await invoke<any>('query_profile');
    line(`  MEMORY online: ${profile.session_count} sessions · ${profile.event_count} events · ${profile.finding_count} findings`, 'muted');
  } catch (e) { line(`  MEMORY cold: ${String(e)}`, 'warn'); }
  line('  Ask: "what's wrong with how I use my agents?"');
  line('  '); html('<span class="cursor"></span>');
  anime({ targets: '#terminal', opacity: [0, 1], translateY: [12, 0], duration: 650, easing: 'easeOutExpo' });
}

function renderDiagnosis(d: Diagnosis) {
  status.textContent = d.detector_only ? 'detector-only diagnosis' : 'verified diagnosis ready';
  line('');
  line('┌─ VERIFIED DIAGNOSIS ─────────────────────────────────────────┐');
  line(`│ ${d.narrative.slice(0, 76).padEnd(76)} │`);
  line('└───────────────────────────────────────────────────────────────┘');
  d.ranked_findings.forEach((f, idx) => {
    const pct = Math.max(8, Math.min(100, f.severity * 20));
    const evidence = (f.evidence || []).slice(0, 3).map(e => `${esc(e.session_id.slice(0, 10))}${e.quote ? ' — ' + esc(e.quote.slice(0, 130)) : ''}`).join('<br/>');
    html(`<article class="hole"><h3>HOLE #${idx + 1} — ${esc(f.title || f.pattern_id)} <span class="bar" style="--pct:${pct}%"></span></h3><div>${esc(f.rationale)}</div><div class="evidence">confidence ${(f.confidence*100).toFixed(0)}% · seen ${(f.frequency*100).toFixed(0)}% · est ${Math.round(f.est_cost_tokens).toLocaleString()} tokens / ${Math.round(f.est_cost_minutes)} min<br/>Evidence:<br/>${evidence || 'detector evidence unavailable'}</div></article>`);
  });
  if (d.do_items?.length) { line('DO:', 'muted'); d.do_items.forEach(x => line(`  ✓ ${x}`)); }
  if (d.stop_items?.length) { line('STOP:', 'warn'); d.stop_items.forEach(x => line(`  ✗ ${x}`)); }
}

form.addEventListener('submit', async ev => {
  ev.preventDefault();
  const q = input.value.trim();
  if (!q) return;
  line(`> ${q}`);
  status.textContent = 'ingesting transcripts';
  line('  Diagnostician entering war room…');
  try {
    const d = await invoke<Diagnosis>('run_diagnosis', { scope: { harness: 'claude_code', query: q, force: false } });
    renderDiagnosis(d);
  } catch (e) { status.textContent = 'diagnosis failed'; line(String(e), 'bad'); }
});

listen<any>('fugu_delta', e => { status.textContent = `${e.payload.stage} · streaming`; war.pulse({ stage: e.payload.stage, output: String(e.payload.delta || '').length }); });
listen<any>('fugu_usage', e => { war.pulse({ stage: e.payload.stage, input: e.payload.input_tokens, output: e.payload.output_tokens, orchestrationInput: e.payload.orchestration_input_tokens, orchestrationOutput: e.payload.orchestration_output_tokens }); });
listen<any>('warden_hotkey', () => { input.focus(); anime({ targets: '#terminal', scale: [0.985, 1], duration: 220, easing: 'easeOutQuad' }); });

boot();
