# WARDEN — M2 Forensic Reconstruction (diagnosis fidelity + real-evidence layer)

> *The diagnosis stops labeling holes and starts reconstructing them from your real transcripts. Every mark on screen maps to a recorded signal — or is explicitly flagged as derived/modeled.*

| | |
|---|---|
| **Type** | Focused functional enhancement on top of built+verified M2 (`m2-face`) |
| **Date** | 2026-06-23 |
| **Depends on** | M0 (IR/store/featurizer), M1 (findings/diagnosis), M2 (overlay, war-room R3F island, `resolve_evidence`) |
| **Parent specs** | `docs/superpowers/specs/2026-06-22-m2-face-design.md`; `SPEC.md` §3 (raw_ref), §7 taxonomy, §8.4 honesty |
| **Implementer** | Opus-on-Max subagent (frontend-design + r3f-mastery), isolated worktree off `m2-face` |

---

## 0. Goal / Definition of Done

The diagnosis screen presents each hole as a **forensic reconstruction** driven by the user's real session data. Done when:

1. A read-only Rust command `get_finding_reconstruction(finding_id)` returns a typed `Reconstruction` assembled from the store/IR.
2. **CONTEXT_BLOAT** renders fully wired to real signals: the saturation curve, the main-context search events, the first-edit marker, the danger band, derived severity, the resolved evidence quote+citation, and the modeled counterfactual.
3. Every other pattern renders a **generic real-timeline** reconstruction (saturation + events + evidence) — no hardcoded data anywhere.
4. Each **derived or modeled** value carries a subtle **ⓘ provenance glyph**; hover reveals its exact definition/method. Measured values carry none.
5. Visual fidelity matches the approved "Forensic Reconstruction" mockup (war-room backdrop + Archivo Expanded / IBM Plex Mono + cinematic build), with fonts **bundled** (no CDN).
6. `cargo test` + `pnpm test` green; built `WARDEN.app` shows a real reconstruction for a real CONTEXT_BLOAT finding.

### Non-goals
- ✗ No new detectors or Brain changes; consumes existing findings.
- ✗ No writes anywhere (read-only). Fix remains preview-only (M4 applies).
- ✗ Bespoke per-pattern visuals beyond CONTEXT_BLOAT (others use the generic timeline this pass).

---

## 1. The honesty model (the spine of this feature)

Every value surfaced carries a **provenance**:

| Provenance | Meaning | UI |
|---|---|---|
| `measured` | read directly from the transcript/store | no glyph |
| `derived` | deterministic function of measured data (documented) | subtle ⓘ → method on hover |
| `modeled` | an estimate under stated assumptions | subtle ⓘ → "modeled, not recorded" + method |

`ProvenanceTag` is a reusable component: a small circled-ⓘ (low-contrast, inline, accessible — `aria-describedby` tooltip, keyboard-focusable). This is a product-wide primitive; the war-room and future milestones reuse it.

---

## 2. Data contract — `get_finding_reconstruction`

Read-only `#[tauri::command]`. Returns:

```rust
struct Reconstruction {
  finding_id: String, pattern_id: String, harness: Harness,
  session_id: String, model: String, context_window: u32,   // per-model map; default 200_000
  kind: ReconKind,                                            // ContextBloat | Generic
  series: Vec<SatPoint>,                                      // measured
  events: Vec<MarkEvent>,                                     // measured
  first_edit_turn: Option<u32>,                               // measured
  danger_threshold: f32,                                      // config (default 0.60)
  peak_saturation: f32,                                       // measured
  severity: u8,                                               // derived
  counterfactual: Option<Vec<SatPoint>>,                      // modeled (ContextBloat only)
  evidence: Vec<EvidenceQuote>,                               // measured (via resolve_evidence)
  wasted_cost_tokens_per_week: Option<u64>,                   // derived (from finding.est_cost_tokens)
  provenance: BTreeMap<String,String>,                        // field -> human method string for ⓘ
}
struct SatPoint { turn: u32, ts: DateTime, context_tokens: u32, saturation: f32 }
struct MarkEvent { turn: u32, kind: MarkKind, tool: Option<String>, raw_ref: RawRef } // Search|Edit|Error|Spawn
struct EvidenceQuote { quote: String, session_id: String, turn_id: String, source_ref: String } // path:line
enum ReconKind { ContextBloat, Generic }
```

### Signal → field mapping (all already in the store)

| Field | Source |
|---|---|
| `series[*].context_tokens` | `TokenUsage.input + cache_read` per assistant `Turn` (full re-sent context proxy) |
| `series[*].saturation` | `context_tokens / context_window` (model→window map; if model unknown → default + ⓘ "approximate window") |
| `events Search` | `ToolCall{tool ∈ Grep,Glob,Read,LS,Bash(grep/cat/find/rg)}` where `Turn.is_sidechain=false` |
| `events Edit` / `first_edit_turn` | first `FileSnapshot` or `ToolCall{Edit,Write,MultiEdit,NotebookEdit}` (non-sidechain) |
| `peak_saturation`, `severity` | featurizer `context_saturation_peak` / `saturation_at_first_edit`; severity derived vs `danger_threshold` |
| `evidence` | `finding.evidence_json` → `raw_ref` → existing **`resolve_evidence()`** |
| `wasted_cost_tokens_per_week` | `finding.est_cost_tokens` (derived) |
| `counterfactual` | modeled: subtract cumulative tokens attributable to in-context search **results** (`ToolResult.bytes/4` proxy) at/before each turn; method recorded in `provenance["counterfactual"]` |

Representative session for a finding = the session in its evidence with the highest `saturation_at_first_edit` (deterministic tiebreak: earliest `started_at`).

### CONTEXT_BLOAT vs Generic
- **ContextBloat**: full set above incl. counterfactual + danger band + bespoke annotations.
- **Generic**: `series` + `events` (search/edit/error/spawn) + `evidence`, no counterfactual; a real timeline any pattern renders. Degrades cleanly when a session lacks `TokenUsage` (e.g. sparse Codex) → events-only timeline, saturation omitted with ⓘ.

---

## 3. Frontend — the Forensic renderer

- Lives in the **React island** (`src/viz/`) as `ForensicView`, layered as an SVG/DOM overlay above the existing war-room R3F backdrop (the canvas blurs/dims to depth-of-field during the Diagnosis phase). The vanilla diagnosis list selects a finding → `invoke('get_finding_reconstruction')` → island renders `ForensicView` from the real object.
- **Chart**: SVG; the curve, dots, first-edit marker, danger band, and counterfactual are all positioned from `Reconstruction` (no hardcoded paths). Curve draws via `pathLength` dashoffset; events pop at their real turn x-positions; severity bar reflects derived value.
- **Aesthetic (locked)**: war-room backdrop, Archivo Expanded (display) + IBM Plex Mono (telemetry), grain/scanline/vignette, ACES-toned bloom backdrop, cinematic eased build (frontend-design + r3f-mastery). Boldness spent on the one reconstruction moment; everything else quiet.
- **Evidence row**: real quote + citation (`source_ref` clickable later), derived cost with ⓘ, fix-preview (read-only).
- **ProvenanceTag (ⓘ)**: rendered next to `severity`, `wasted_cost`, `counterfactual`, and any approximate saturation; tooltip text from `Reconstruction.provenance`.
- **Performance** (r3f-mastery): island already mounted/pre-warmed; pause RAF when hidden; SVG is cheap; no geometry in `useFrame`.

---

## 4. Design system (bank it for M3–M7)

Codify into `src/viz/designSystem.ts` (+ bundled fonts under `src/assets/fonts/`):
- **Color** tokens (reuse `harnessTheme`): void `#000402`, emerald `#3dffa0`, hot `#eafff4`, amber `#ff5a37`, danger `#ff3b3b`, muted `#3f7d62`.
- **Type**: Archivo Expanded (display) + IBM Plex Mono (mono/data), bundled `@font-face` (no CDN); type scale + weights.
- **Motion**: standard easing `cubic-bezier(.16,1,.3,1)`, draw/pop/up keyframes, stagger steps.
- **Post**: bloom ≈1.0, ACES, FogExp2, vignette, grain 0.10–0.12 (fixes the near-invisible 0.06).
Future milestones inherit this; no more under-crafted output.

---

## 5. Module / file plan

**Rust (`src-tauri/src/`)**
- `reconstruction.rs` — **new**: assemble `Reconstruction` from store/IR (series, events, counterfactual, provenance).
- `commands.rs` — `get_finding_reconstruction(finding_id)`.
- `featurizer.rs`/`store.rs` — expose per-turn series helper if not already; model→window map in `util.rs`/`config`.
**Frontend (`src/`)**
- `viz/ForensicView.tsx` + `viz/forensicChart.ts` (SVG geometry from real series) — **new**.
- `viz/ProvenanceTag.tsx` — **new** (the ⓘ primitive).
- `viz/designSystem.ts` + `assets/fonts/` — **new** (bundled fonts + tokens).
- diagnosis screen: wire finding-selection → reconstruction; mount `ForensicView` in the island.

---

## 6. Build order
1. `reconstruction.rs` + command + **golden test** (seeded session → exact series/events/first_edit/severity).
2. Bundle fonts + `designSystem.ts` (grain/bloom fix).
3. `ForensicView` (CONTEXT_BLOAT) wired to the real command, over war-room backdrop, with motion + `ProvenanceTag`.
4. Generic fallback renderer.
5. Wire diagnosis selection → reconstruction; degraded states.
6. Tests + `pnpm tauri build` verify.

## 7. Testing
- **Rust golden**: synthetic session (known `TokenUsage` per turn, N non-sidechain Grep `ToolCall`s, a first edit) → assert `series`, `events`, `first_edit_turn`, derived `severity`, counterfactual method string. Deterministic (no clock/rng).
- **resolve_evidence** integration: evidence quote + citation resolve.
- **Web smoke**: `ForensicView` renders from a fixture `Reconstruction`; ⓘ tooltips present & focusable; degraded (no-TokenUsage) path renders events-only.

## 8. Risks
- **Window proxy** — `input+cache_read` ≈ context; not exact under cache churn → label saturation method in ⓘ; default 200k window per-model map.
- **Counterfactual crudeness** — `bytes/4` token proxy → `modeled` + method in ⓘ; never stated as fact.
- **Sparse TokenUsage** (some Codex sessions) → generic events-only timeline, saturation omitted with ⓘ.
- **Scope creep** — keep bespoke to CONTEXT_BLOAT this pass; others generic.

---

*Process: user reviews this spec → orchestrator dispatches Opus-on-Max implementer (worktree off `m2-face`, frontend-design + r3f-mastery) → subagent-verify (frontend + read-only backend scope; golden tests) → merge → resume M3 grill.*
