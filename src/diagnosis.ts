// diagnosis.ts — the forensic readout (spec §5.3, §7, §8).
//
// This is the payload the whole overlay builds toward: the ranked "holes" in how
// you drive your agents, each with a real 1–5 severity meter, a cost ledger
// (tokens / minutes the hole is taxing you), a harness badge, and an evidence
// drill-down that surfaces the stored quote for each EvidenceRef. Below the
// holes sit the Do / Stop guidance and the narrative, then (on demand) a
// read-only fix preview rendered as a unified diff with a DISABLED [Y/n] apply
// affordance — apply is the M4 slice, never wired here.
//
// PURE DOM by design: this module imports only `harnessTheme` (the single source
// of harness identity) so it can be unit-tested under jsdom WITHOUT pulling the
// R3F war-room or the lazy Remotion chunk. `main.ts` owns the `./style.css`
// import and the Tauri wiring; it hands us a container + the harness resolver +
// (optionally) an `invoke` for the fix-preview fetch.

import { harnessTheme } from './viz/harnessTheme';

// ── web-side shapes (mirror the Rust serde, snake_case across the bridge) ─────

export type Evidence = {
  session_id: string;
  turn_id?: string | null;
  event_id?: string | null;
  quote?: string | null;
  source_path?: string | null;
};

export type Finding = {
  id: string;
  pattern_id: string;
  title: string;
  severity: number; // real 1..5
  frequency: number; // 0..1 share of sessions
  confidence: number; // 0..1
  est_cost_tokens: number;
  est_cost_minutes: number;
  rationale: string;
  evidence: Evidence[];
  status?: string | null;
  verifier_verdict?: string | null;
};

export type Diagnosis = {
  id: string;
  created_at: string;
  ranked_findings: Finding[];
  do_items: string[];
  stop_items: string[];
  narrative: string;
  detector_only: boolean;
};

export type FixPreview = {
  finding_id: string;
  pattern_id: string;
  target_path: string;
  diff: string;
  applied: boolean;
};

// The web `Finding` carries no harness field, but the war-room/reveal track it
// per candidate. `main.ts` knows the active harness (and the per-pattern harness
// from `candidates_nominated`), so it injects a resolver. Default: the scope
// harness, falling through to neutral.
// Recovered ground truth for an EvidenceRef whose stored `quote` is null (the
// Fugu pipeline leaves it null for some findings). Mirrors the Rust
// `ResolvedEvidence` serde shape.
export type ResolvedEvidence = {
  quote?: string | null;
  source_path?: string | null;
};

export type DiagnosisDeps = {
  harnessOf: (f: Finding) => string;
  // Optional fix-preview fetcher (Tauri `invoke`). When provided, each hole
  // grows a "fix preview" toggle that lazily fetches + renders the diff.
  fetchFixPreview?: (findingId: string) => Promise<FixPreview>;
  // Optional READ-ONLY evidence resolver (Tauri `invoke('resolve_evidence')`).
  // The fast path renders the stored `quote` directly; only when a ref has NO
  // stored quote but DOES carry an event_id do we fall back to this to recover
  // the excerpt from the underlying event. Absent resolver ⇒ keep the honest
  // "no excerpt stored" placeholder; never fabricate text.
  resolveEvidence?: (sessionId: string, eventId: string) => Promise<ResolvedEvidence>;
};

// ── small DOM helpers (local; main.ts has its own line()/html() for the log) ──

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = text;
  return node;
}

function shortId(value: string | null | undefined, keep = 10): string {
  if (!value) return 'unknown';
  return value.length > keep ? value.slice(0, keep) : value;
}

function clampSeverity(s: number): number {
  if (!Number.isFinite(s)) return 0;
  return Math.max(0, Math.min(5, Math.round(s)));
}

function fmt(n: number | null | undefined): string {
  return typeof n === 'number' && Number.isFinite(n) ? Math.round(n).toLocaleString() : '—';
}

function pct(x: number | null | undefined): string {
  return typeof x === 'number' && Number.isFinite(x) ? `${Math.round(x * 100)}%` : '—';
}

// ── signature element: the segmented severity meter ───────────────────────────
// Five DISCRETE ticks (severity is an integer 1..5, so a smooth gradient would
// lie about the resolution). Filled ticks read amber at 4–5 (verdict-critical),
// acid at 3, phosphor below — the same severity→colour ramp as the reveal, so
// the persistent screen and the slam-in agree.

function severityMeter(severity: number): HTMLElement {
  const sev = clampSeverity(severity);
  const meter = el('span', 'sev-meter');
  meter.setAttribute('role', 'meter');
  meter.setAttribute('aria-valuemin', '0');
  meter.setAttribute('aria-valuemax', '5');
  meter.setAttribute('aria-valuenow', String(sev));
  meter.setAttribute('aria-label', `severity ${sev} of 5`);
  const band = sev >= 4 ? 'crit' : sev >= 3 ? 'high' : 'low';
  meter.dataset.band = band;
  for (let i = 1; i <= 5; i++) {
    const tick = el('span', `sev-tick${i <= sev ? ' on' : ''}`);
    tick.dataset.band = band;
    meter.appendChild(tick);
  }
  return meter;
}

// ── harness badge: colour ALWAYS paired with glyph + label (a11y) ─────────────

function harnessBadge(harness: string): HTMLElement {
  const t = harnessTheme(harness);
  const badge = el('span', 'harness-badge');
  badge.style.setProperty('--harness', t.color);
  badge.setAttribute('aria-label', `${t.label} session`);
  const glyph = el('span', 'harness-glyph', t.glyph);
  glyph.setAttribute('aria-hidden', 'true');
  const label = el('span', 'harness-label', t.label);
  badge.append(glyph, label);
  return badge;
}

function severityWord(sev: number): string {
  return sev >= 4 ? 'critical' : sev >= 3 ? 'major' : sev >= 2 ? 'moderate' : 'minor';
}

// ── evidence drill-down ───────────────────────────────────────────────────────
// Fast path: an EvidenceRef that carries a stored `quote` (populated at
// detection time, Rust `detectors::evidence_for`) renders session · turn · quote
// directly, no round trip. Fallback: the Fugu pipeline leaves `quote` null for
// some findings — when that happens but the ref still names an `event_id`, we
// call the injected READ-ONLY `resolveEvidence` to recover the excerpt from the
// underlying event (spec §5.3/§7: every claim traceable to ground truth). If no
// resolver is wired, or it yields nothing, we keep the honest placeholder rather
// than inventing text.

function evidenceItem(ev: Evidence, deps: DiagnosisDeps): HTMLElement {
  const item = el('li', 'evidence-item');
  const loc = el('span', 'evidence-loc');
  loc.append(el('span', 'evidence-key', 'session'), el('span', 'evidence-val', shortId(ev.session_id)));
  if (ev.turn_id) {
    loc.append(el('span', 'evidence-key', 'turn'), el('span', 'evidence-val', ev.turn_id));
  }
  item.appendChild(loc);
  if (ev.quote && ev.quote.trim()) {
    item.appendChild(el('blockquote', 'evidence-quote', ev.quote.trim()));
  } else if (deps.resolveEvidence && ev.event_id && ev.session_id) {
    // Null stored quote but a resolvable event: show a recovering placeholder,
    // then swap in the ground-truth excerpt once the read-only fetch returns.
    const slot = el('div', 'evidence-quote evidence-quote-empty', 'recovering excerpt…');
    item.appendChild(slot);
    deps
      .resolveEvidence(ev.session_id, ev.event_id)
      .then(res => {
        const quote = res && res.quote ? res.quote.trim() : '';
        if (quote) {
          const block = el('blockquote', 'evidence-quote evidence-quote-resolved', quote);
          slot.replaceWith(block);
        } else {
          slot.textContent = 'no excerpt stored for this reference';
        }
      })
      .catch(() => {
        slot.textContent = 'no excerpt stored for this reference';
      });
  } else {
    item.appendChild(el('div', 'evidence-quote evidence-quote-empty', 'no excerpt stored for this reference'));
  }
  return item;
}

// ── one ranked hole ───────────────────────────────────────────────────────────

function renderHole(f: Finding, rank: number, deps: DiagnosisDeps, detectorOnly: boolean): HTMLElement {
  const sev = clampSeverity(f.severity);
  const hole = el('article', 'diag-hole');
  hole.dataset.severityBand = sev >= 4 ? 'crit' : sev >= 3 ? 'high' : 'low';

  // Head: rank gutter · title · severity meter.
  const head = el('div', 'hole-head');
  head.appendChild(el('span', 'hole-rank', `#${rank + 1}`));

  const titleWrap = el('div', 'hole-title-wrap');
  titleWrap.appendChild(el('h3', 'hole-title', f.title || f.pattern_id));
  const sub = el('div', 'hole-sub');
  sub.appendChild(harnessBadge(deps.harnessOf(f)));
  sub.appendChild(el('span', 'hole-pattern', f.pattern_id));
  if (detectorOnly) {
    sub.appendChild(el('span', 'tag tag-detector', 'detector-only · no API key / budget'));
  } else if (f.verifier_verdict) {
    sub.appendChild(el('span', 'tag tag-verified', 'verifier-confirmed'));
  }
  titleWrap.appendChild(sub);
  head.appendChild(titleWrap);

  const meterWrap = el('div', 'hole-meter');
  meterWrap.appendChild(severityMeter(sev));
  meterWrap.appendChild(el('span', 'sev-word', `${severityWord(sev)} · ${sev}/5`));
  head.appendChild(meterWrap);
  hole.appendChild(head);

  // One-line summary (rationale).
  hole.appendChild(el('p', 'hole-summary', f.rationale));

  // Cost ledger — the forensic line: how often, how confident, what it costs.
  const ledger = el('div', 'hole-ledger');
  const stat = (key: string, val: string) => {
    const cell = el('span', 'ledger-stat');
    cell.append(el('span', 'ledger-key', key), el('span', 'ledger-val', val));
    return cell;
  };
  ledger.appendChild(stat('seen', pct(f.frequency)));
  ledger.appendChild(stat('confidence', pct(f.confidence)));
  ledger.appendChild(stat('cost', `${fmt(f.est_cost_tokens)} tok`));
  ledger.appendChild(stat('time', `${fmt(f.est_cost_minutes)} min`));
  hole.appendChild(ledger);

  // Evidence drill-down (collapsed until toggled).
  const evWrap = el('div', 'evidence-wrap');
  const evCount = (f.evidence || []).length;
  const toggle = el('button', 'evidence-toggle');
  toggle.type = 'button';
  toggle.setAttribute('aria-expanded', 'false');
  toggle.textContent = `▸ evidence (${evCount})`;
  const evList = el('ul', 'evidence-list');
  evList.hidden = true;
  let built = false;
  toggle.addEventListener('click', () => {
    const open = toggle.getAttribute('aria-expanded') === 'true';
    if (!open && !built) {
      (f.evidence || []).forEach(ev => evList.appendChild(evidenceItem(ev, deps)));
      if (evCount === 0) evList.appendChild(el('li', 'evidence-item evidence-empty', 'no stored references'));
      built = true;
    }
    toggle.setAttribute('aria-expanded', open ? 'false' : 'true');
    toggle.textContent = `${open ? '▸' : '▾'} evidence (${evCount})`;
    evList.hidden = open;
  });
  evWrap.append(toggle, evList);
  hole.appendChild(evWrap);

  // Fix preview (read-only) — only when a fetcher was injected.
  if (deps.fetchFixPreview) {
    const fixWrap = el('div', 'fix-wrap');
    const fixToggle = el('button', 'fix-toggle');
    fixToggle.type = 'button';
    fixToggle.textContent = '▸ fix preview';
    const fixSlot = el('div', 'fix-slot');
    fixSlot.hidden = true;
    let loaded = false;
    fixToggle.addEventListener('click', async () => {
      const open = !fixSlot.hidden;
      if (open) {
        fixSlot.hidden = true;
        fixToggle.textContent = '▸ fix preview';
        return;
      }
      fixSlot.hidden = false;
      fixToggle.textContent = '▾ fix preview';
      if (!loaded) {
        loaded = true;
        fixSlot.textContent = 'resolving diff…';
        try {
          const preview = await deps.fetchFixPreview!(f.id);
          fixSlot.textContent = '';
          renderFixPreview(fixSlot, preview);
        } catch (e) {
          fixSlot.textContent = '';
          fixSlot.appendChild(el('div', 'fix-error', `fix preview unavailable: ${String(e)}`));
        }
      }
    });
    fixWrap.append(fixToggle, fixSlot);
    hole.appendChild(fixWrap);
  }

  return hole;
}

// ── public: renderDiagnosis ───────────────────────────────────────────────────
// Builds the full readout into `container`. Returns the root section so callers
// can animate it. Does not clear the container — `main.ts` appends it to the log
// flow so the conversation history is preserved.

export function renderDiagnosis(container: HTMLElement, d: Diagnosis, deps: DiagnosisDeps): HTMLElement {
  const root = el('section', 'diagnosis');
  root.dataset.detectorOnly = d.detector_only ? 'true' : 'false';

  // Verdict header — amber when verified, dimmed when detector-only.
  const header = el('header', 'diag-header');
  const title = el('div', 'diag-title');
  title.appendChild(el('span', 'diag-sigil', 'WARDEN'));
  title.appendChild(
    el('span', 'diag-verdict', d.detector_only ? 'DETECTOR READOUT' : 'VERIFIED DIAGNOSIS'),
  );
  header.appendChild(title);

  const holeCount = d.ranked_findings.length;
  const meta = el('div', 'diag-meta');
  meta.textContent = `${holeCount} hole${holeCount === 1 ? '' : 's'} · ${shortId(d.id, 12)}`;
  header.appendChild(meta);

  if (d.detector_only) {
    header.appendChild(
      el('div', 'diag-degraded', 'detector-only — no API key / budget; findings are heuristic, not Fugu-verified'),
    );
  }
  root.appendChild(header);

  // Narrative — the one-paragraph "what's going on".
  if (d.narrative && d.narrative.trim()) {
    root.appendChild(el('p', 'diag-narrative', d.narrative.trim()));
  }

  // Ranked holes.
  const holes = el('div', 'diag-holes');
  if (holeCount === 0) {
    holes.appendChild(el('div', 'diag-empty', 'No confirmed holes this run — the war room held.'));
  } else {
    d.ranked_findings.forEach((f, i) => holes.appendChild(renderHole(f, i, deps, d.detector_only)));
  }
  root.appendChild(holes);

  // Do / Stop guidance.
  const guidance = el('div', 'diag-guidance');
  guidance.appendChild(actionList('diag-do', 'DO', '+', d.do_items));
  guidance.appendChild(actionList('diag-stop', 'STOP', '–', d.stop_items));
  root.appendChild(guidance);

  container.appendChild(root);
  return root;
}

function actionList(cls: string, heading: string, glyph: string, items: string[]): HTMLElement {
  const wrap = el('div', cls);
  wrap.appendChild(el('div', 'action-head', heading));
  const list = el('ul', 'action-list');
  if (!items || items.length === 0) {
    list.appendChild(el('li', 'action-item action-empty', '—'));
  } else {
    items.forEach(item => {
      const li = el('li', 'action-item');
      li.append(el('span', 'action-glyph', glyph), el('span', 'action-text', item));
      list.appendChild(li);
    });
  }
  wrap.appendChild(list);
  return wrap;
}

// ── public: renderFixPreview ──────────────────────────────────────────────────
// Read-only unified-diff render with a DISABLED apply affordance. M2 is
// preview-only: there is no code path here that writes — apply ships in M4.

export function renderFixPreview(container: HTMLElement, p: FixPreview): HTMLElement {
  const block = el('div', 'fix-preview');

  const head = el('div', 'fix-head');
  head.appendChild(el('span', 'fix-label', 'FIX PREVIEW'));
  const path = el('span', 'fix-path', p.target_path);
  path.title = p.target_path;
  head.appendChild(path);
  block.appendChild(head);

  // The diff body — each line typed by its leading char so +/- read in colour.
  const pre = el('pre', 'fix-diff');
  const lines = (p.diff || '').split('\n');
  lines.forEach(raw => {
    const row = el('span', 'diff-line');
    if (raw.startsWith('+++') || raw.startsWith('---')) {
      row.className = 'diff-line diff-file';
    } else if (raw.startsWith('@@')) {
      row.className = 'diff-line diff-hunk';
    } else if (raw.startsWith('+')) {
      row.className = 'diff-line diff-add';
    } else if (raw.startsWith('-')) {
      row.className = 'diff-line diff-del';
    }
    row.textContent = raw === '' ? ' ' : raw;
    pre.appendChild(row);
    pre.appendChild(document.createTextNode('\n'));
  });
  block.appendChild(pre);

  // Apply prompt — present, disabled, honest about why.
  const foot = el('div', 'fix-foot');
  const apply = el('button', 'apply-prompt');
  apply.type = 'button';
  apply.disabled = true;
  apply.textContent = 'apply [Y/n]';
  apply.setAttribute('aria-disabled', 'true');
  foot.appendChild(apply);
  foot.appendChild(el('span', 'fix-note', 'apply coming in M4 — WARDEN never writes to your projects in this build'));
  block.appendChild(foot);

  container.appendChild(block);
  return block;
}
