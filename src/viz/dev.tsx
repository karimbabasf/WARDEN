// dev.tsx — standalone visual QA harness for the war room (no Tauri app).
// `vite` serves it at /dev-viz.html. It mounts <WarRoom/> against a local bridge
// and replays a scripted mock-event sequence on a timer, looping forever, so the
// scene can be eyeballed in a plain browser. Doubles as the viz smoke test.
//
// The script walks the real lifecycle: idle → ~8 candidates across Claude+Codex
// → fugu_delta / fugu_usage pulses → several finding_verdict (confirmed+refuted)
// → diagnosis_ready (reveal) → reset → loop. Every event shape matches the
// locked Task-6 contract exactly, so what you see here is what the app renders.

import { createRoot } from 'react-dom/client';
import { createBridge } from './bridge';
import { WarRoom } from './WarRoom';
import './../style.css';

// listen is never used in dev (we drive ingest directly), so pass a no-op.
const noopListen = (async () => () => {}) as unknown as Parameters<typeof createBridge>[0];
const bridge = createBridge(noopListen);

type Step = { at: number; name: string; payload: any };

const PATTERNS = [
  'unbounded_context',
  'no_subagents',
  'repeated_reads',
  'thrash_edit_loop',
  'ignored_test_failures',
  'serial_tool_calls',
  'context_flood',
  'missing_verification',
];

const candidates = PATTERNS.map((pattern_id, i) => ({
  pattern_id,
  session_id: `sess-${i}`,
  harness: i % 3 === 2 ? 'codex' : 'claude_code',
  severity_hint: 1 + (i % 4),
}));

const STAGES = ['Diagnostician', 'Coach', 'Verifier'];

// Build a looping timeline (ms offsets from cycle start).
function buildScript(): Step[] {
  const s: Step[] = [];
  // 1) nominate the candidate cloud
  s.push({ at: 600, name: 'candidates_nominated', payload: { candidates } });
  // 2) Diagnostician + Coach + Verifier stream + token usage pulses
  let t = 1400;
  for (const stage of STAGES) {
    for (let k = 0; k < 4; k++) {
      s.push({ at: t, name: 'fugu_delta', payload: { stage, delta: 'reasoning '.repeat(2 + k) } });
      t += 220;
    }
    s.push({
      at: t,
      name: 'fugu_usage',
      payload: {
        stage,
        input_tokens: 1800 + Math.round(Math.random() * 4000),
        output_tokens: 400 + Math.round(Math.random() * 900),
        // Diagnostician/Coach get orchestration tokens (Fugu); Verifier left
        // bare to exercise the off-Fugu degradation path (plain-token weight).
        orchestration_input_tokens: stage === 'Verifier' ? 0 : 200 + Math.round(Math.random() * 600),
        orchestration_output_tokens: stage === 'Verifier' ? 0 : 80 + Math.round(Math.random() * 220),
      },
    });
    t += 500;
  }
  // 3) verdicts — mostly confirmed (amber flare), a couple refuted (collapse)
  const verdicts: Array<'confirmed' | 'refuted'> = ['confirmed', 'confirmed', 'refuted', 'confirmed', 'refuted', 'confirmed'];
  verdicts.forEach((verdict, i) => {
    const c = candidates[i];
    s.push({
      at: t,
      name: 'finding_verdict',
      payload: {
        finding_id: `f-${i}`,
        pattern_id: c.pattern_id,
        harness: c.harness,
        verdict,
        severity: verdict === 'confirmed' ? 3 + (i % 3) : 1,
      },
    });
    t += 420;
  });
  // 4) reveal
  s.push({ at: t + 400, name: 'diagnosis_ready', payload: { id: 'diag-dev', finding_count: 4 } });
  return s;
}

const script = buildScript();
const CYCLE = script[script.length - 1].at + 3000; // dwell on reveal, then loop

const root = createRoot(document.getElementById('dev-root')!);
root.render(<WarRoom bridge={bridge} />);

let cycleStart = performance.now();
let idx = 0;

function loop() {
  const now = performance.now();
  const elapsed = now - cycleStart;
  while (idx < script.length && elapsed >= script[idx].at) {
    const step = script[idx++];
    bridge.ingest(step.name, step.payload);
  }
  if (elapsed >= CYCLE) {
    bridge.reset();
    cycleStart = now;
    idx = 0;
  }
  requestAnimationFrame(loop);
}
requestAnimationFrame(loop);
