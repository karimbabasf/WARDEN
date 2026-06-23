# WARDEN M2 — "FACE" Design Spec

> *The jaw-drop. Hotkey → black screen → ask → a war room of agents dissecting your agents → a ranked, evidence-cited diagnosis.*

| | |
|---|---|
| **Milestone** | M2 — Face (the always-on overlay + live war-room + diagnosis) |
| **Date** | 2026-06-22 |
| **Depends on** | M0 (IR + Claude adapter + store + featurizer), M1 (Fugu Diagnostician→Coach→Verifier) — both complete |
| **Build target** | macOS (Apple Silicon), Tauri v2 (Rust core + web overlay) |
| **Status** | Approved design, pending user review of this spec |
| **Parent spec** | `SPEC.md` §11 (Face), §8.4 (war-room honesty), §20 (jaw-drop demo) |

---

## 0. Goal / Definition of Done

WARDEN becomes a **real always-on macOS daemon** you summon with a hotkey. Pressing the hotkey from anywhere drops a transparent green-on-black terminal overlay; you type (or, later, speak) *"what's wrong with how I use my agents?"*; a live **war-room visualizer** lights up as the existing Fugu pipeline runs; a **ranked, verified, evidence-cited diagnosis** slams in; each hole expands to the exact session/turn/quote; each hole shows a **read-only fix preview**.

M2 is **done** when, on this machine with its real transcripts:

1. The app runs as a **menubar agent** (no Dock icon), continuously ingesting `~/.claude/projects` **and** `~/.codex/sessions` in the background via FSEvents.
2. A global hotkey (**⌘⇧Space**) summons the pre-warmed overlay in **<150 ms**.
3. Asking a question runs the M1 pipeline and drives the **R3F war-room** from *real* Fugu signals (token deltas, orchestration-token weight, per-finding verdicts) — no fabricated per-agent theater.
4. The diagnosis renders with **severity, frequency, est. cost, do/stop, narrative, and per-finding evidence drill-down** to the exact `raw_ref`.
5. Each finding shows a **read-only fix preview** (a diff); applying is explicitly deferred to M4.
6. The UI **differentiates harnesses** (Claude vs Codex) everywhere a session/finding appears.
7. The reasoning engine is **env-swappable** (base URL + model names), defaulting to Fugu, ready to point at Near AI later.

### Non-goals (explicitly out of M2)
- ✗ RADAR fleet/Warp (M3), live hooks/interjection (M5), voice/screen (M6), Forge **apply** (M4 — M2 is preview-only).
- ✗ Cursor / Hermes / OpenClaw / Generic **adapters** — *architected for, not built in M2* (Claude + Codex only).
- ✗ Settings UI pane (env + `config.toml` only this milestone).
- ✗ Any `git push` / MR / PR / outbound publishing. No file writes to user projects (preview only).

---

## 1. Architecture Overview

```
                        WARDEN daemon (Tauri v2, menubar agent — ActivationPolicy::Accessory)
  on-disk transcripts   ┌───────────────────────────────────────────────────────────────┐
  ~/.claude/projects ──▶│ EYES (adapter registry)        MEMORY (rusqlite+FTS5)          │
  ~/.codex/sessions  ──▶│  ├─ ClaudeCodeAdapter  ──IR──▶  sessions/turns/events/...        │
  (FSEvents tail)       │  └─ CodexAdapter       ──IR──▶  watermarks (byte-offset)         │
                        │                                   │                              │
                        │  FEATURIZER + DETECTORS ◀─────────┘  nominate candidate holes    │
                        │        │ candidates                                              │
                        │        ▼                                                         │
                        │  BRAIN (Fugu client, env-swappable)   ── SSE ──▶ api.sakana.ai   │
                        │  Diagnostician → Coach → Verifier                                │
                        │        │ emits: candidates_nominated, fugu_delta, fugu_usage,    │
                        │        │        finding_verdict, diagnosis_ready                 │
                        └────────┼─────────────────── IPC (commands + events) ────────────┘
                                 ▼
            ┌──────────────────────────────────────────────────────────────────┐
            │ FACE — overlay webview (pre-warmed, hidden until hotkey)           │
            │  VANILLA TS SHELL: boot · terminal renderer · HUD · screen router  │
            │  REACT ISLAND (R3F + Remotion): war-room viz + cinematic reveal    │
            └──────────────────────────────────────────────────────────────────┘
```

**Process model.** One Tauri process running as a macOS **Accessory** app (menubar/tray only, no Dock). At launch it: sets activation policy, builds the tray, registers the global shortcut, **creates the overlay window hidden and pre-warmed** (so summon is just `show()` + `setFocus()`), and spawns the **ingest watchers** (one Tokio task per adapter) plus the **scheduler**.

**Frontend split (locked decision).** The overlay shell — boot sequence, terminal renderer, HUD, screen routing, diagnosis/evidence DOM — stays **vanilla TypeScript** (fast, already built in `src/`). The **war-room visualizer and cinematic reveal** are a **React + react-three-fiber + Remotion island** mounted into a `<div id="war-room-root">`. The island mounts **once** at daemon start (window pre-warmed and hidden), so React/R3F mount cost is never on the summon hot path. Production bundles all deps via Vite — **no CDN** (the brainstorm mockups used jsdelivr; the app does not).

---

## 2. EYES — Ingest & the Adapter Contract

### 2.1 The Adapter trait (formalized in `ingest/mod.rs`)

The keystone promise (`SPEC.md` G/§4): **adding a harness = one adapter, zero downstream changes.** M2 makes the trait real with two implementors.

```rust
pub trait Adapter: Send + Sync {
    fn harness(&self) -> Harness;
    fn detect(&self) -> Vec<SourceLocator>;            // discover sources on disk
    fn backfill(&self, store: &Store) -> Result<IngestStats>;   // one-shot historical
    fn watch(&self, store: Store, tx: Sender<IngestProgress>);  // live FSEvents tail
    fn map(&self, raw: RawRecord) -> Vec<EventRecord>; // raw line → IR
}
```

- **Registry** (`AdapterRegistry`) holds all enabled adapters; `lib.rs` setup calls `backfill()` once, then spawns `watch()` per adapter on its own Tokio task. A failing adapter is isolated — it never stalls the others (`SPEC.md` §16).
- **Idempotency + watermarks.** Per-source **byte-offset watermark** in the `watermarks` table; on FSEvents `Modify`, seek to the saved offset, read to EOF, parse new JSONL lines, advance the offset. Restart resumes cleanly, never re-processes. (Claude's existing hash-dedup is kept as a backstop for whole-file rewrites.)

### 2.2 Claude Code adapter — add **live** tailing

`ingest/claude_code.rs` today is backfill + per-file hash dedup. M2 adds:
- `notify` `RecommendedWatcher` (FSEvents) on `~/.claude/projects/**`, filtered to `*.jsonl` in the handler (not the watcher).
- Byte-offset watermark per file; **read all bytes since offset** on each event (FSEvents coalesces rapid writes — do not trust event counts).
- Existing record→IR mapping is unchanged (confirmed schema, `SPEC.md` §4.2).

### 2.3 Codex adapter — **new** (`ingest/codex.rs`)

Disk-confirmed on this machine. Source: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid7>.jsonl` (+ `~/.codex/archived_sessions/**` for backfill). Append-only JSONL; envelope `{timestamp, type, payload}`.

| Codex record (`type` / `payload.type`) | → IR |
|---|---|
| `session_meta` | `Session { external_id: payload.id, project: cwd, model_ids: [model_provider…], harness: Codex }` |
| `turn_context` | `Turn` boundary + `ModeChange { mode: collaboration_mode.mode }` |
| `event_msg/task_started` | open `Turn { id: turn_id, started_at }` |
| `event_msg/task_complete` \| `turn_aborted` | close `Turn` (+ `Error` on abort) |
| `event_msg/user_message` | `UserPrompt { text: message, attachments: images }` |
| `event_msg/agent_message` | `AssistantText { text: message }` |
| `event_msg/token_count` | `TokenUsage { input, output, cache_read: cached_input, model }` |
| `event_msg/patch_apply_end` | `FileSnapshot { files: changes.keys() }` |
| `response_item/reasoning` | `Thinking { tokens≈len }` |
| `response_item/function_call` \| `custom_tool_call` | `ToolCall { tool: name, input: arguments, call_id }` |
| `response_item/function_call_output` | `ToolResult { call_id, status: ok\|error, summary: output }` |
| unknown `type`/`payload.type` | `SystemNotice { subtype, data }` (defensive — schema drift never drops a session, `SPEC.md` §16) |

Ingest strategy: backfill `archived_sessions` + existing `sessions` on first run; FSEvents tail live files with byte-offset watermark. Session boundary = `session_meta`; turn boundary = `task_started`→`task_complete`/`turn_aborted`.

> Codex subagent/orchestration signals are sparser than Claude's `isSidechain`; detectors that need delegation data degrade gracefully (feature marked unknown, not zero — `SPEC.md` §16).

### 2.4 Harness identity is first-class

`Harness` (already in `ir.rs`: `ClaudeCode, Codex, Cursor, Hermes, Generic(String)`) is carried on every `Session` and surfaced in every UI element that shows a session or finding (§6.4, §7). A central `harness_theme(Harness) -> {label, color, glyph}` map is the single source of truth (Claude = emerald `#3dffa0`, Codex = violet `#b98cff`; others reserved). No harness-specific logic leaks into the featurizer or Brain.

---

## 3. MEMORY — store deltas

Schema is already complete (M0, 14 tables). M2 changes:
- **Watermarks**: ensure per-source **byte-offset** semantics are used by both adapters (column exists).
- **Sessions**: confirm `harness` is queryable for per-harness rollups.
- **New read query**: `profile_with_harness_breakdown()` → `{ session_count, event_count, finding_count, by_harness: [{harness, sessions, events}] }` for the HUD.
- No migration beyond what M0 ships unless a column is missing; any change is forward-only and version-gated.

---

## 4. BRAIN — engine swappability + viz signals

### 4.1 Env-swappable engine (locked)

`brain.rs` currently hardcodes `https://api.sakana.ai/v1/responses` (×2) and `"fugu-ultra"`/`"fugu"` (×4). Extract to config/env, following the existing `WARDEN_DB_PATH` pattern in `util.rs`:

| Env var | Default | Purpose |
|---|---|---|
| `WARDEN_BRAIN_BASE_URL` | `https://api.sakana.ai/v1` | OpenAI-compatible base (Near AI swap target) |
| `WARDEN_BRAIN_API_KEY` | falls back to `SAKANA_API_KEY` | bearer key |
| `WARDEN_BRAIN_DIAGNOSE_MODEL` | `fugu-ultra` | Diagnostician + Coach |
| `WARDEN_BRAIN_VERIFY_MODEL` | `fugu` | Verifier |
| `WARDEN_BRAIN_EFFORT` | `xhigh` / `high` | reasoning effort per tier |

The endpoint path (`/responses`) and request shape stay Responses-API (Near AI is OpenAI-compatible). **Orchestration-token war-room signals are Fugu-specific**: if a provider omits `orchestration_*` usage fields, the viz **degrades to delta-driven pulses + plain token weight** (documented, honest) rather than faking them.

### 4.2 New IPC events the viz needs (the honest mapping, option ①)

The war room visualizes the **real pipeline judging your candidate holes**. Two new events bracket the existing stream:

- **`candidates_nominated`** — emitted after detectors run, *before* Fugu: `{ candidates: [{ pattern_id, session_id, harness, severity_hint }] }`. → the war room spawns one **candidate-hole node** per candidate.
- (existing) **`fugu_delta`** `{stage, delta}` → pulses along edges (activity).
- (existing) **`fugu_usage`** `{stage, input, output, orchestration_input, orchestration_output}` → node size/glow (orchestration weight).
- **`finding_verdict`** — emitted during/after the Verifier per finding: `{ finding_id, pattern_id, harness, verdict: "confirmed"|"refuted", severity }`. → node **flares amber and survives** (confirmed) or **dims, collapses, dies** (refuted).
- (existing) **`diagnosis_ready`** `{id, finding_count}` → surviving nodes fly into the diagnosis list.

This keeps the visual and the data the *same thing*: node count = real candidate count; flares = real verdicts (`SPEC.md` §8.4, risk R2).

---

## 5. FACE — the overlay shell (vanilla TS)

### 5.1 Daemon + window (research-confirmed Tauri v2)

- **Menubar agent**: `app.set_activation_policy(ActivationPolicy::Accessory)` in `setup()` **before** any window shows (else a Dock icon flashes) + `LSUIElement=true` in `Info.plist`; tray via Tauri tray-icon (quit, summon, status).
- **Overlay window** (created hidden at startup, pre-warmed):
  ```jsonc
  { "label":"overlay", "transparent":true, "decorations":false, "alwaysOnTop":true,
    "skipTaskbar":true, "visibleOnAllWorkspaces":true, "focus":false, "visible":false,
    "resizable":false, "shadow":false, "width":960, "height":640 }
  ```
  `macos-private-api` is already enabled. Click-through when idle via `set_ignore_cursor_events(true)` (set in Rust at creation, before first show); on summon → `setIgnoreCursorEvents(false)`, `show()`, `setFocus()`. Dismiss on `Esc` and `tauri://blur` → `hide()` + re-enable click-through.
- **Global hotkey**: `tauri-plugin-global-shortcut` (already a dep), **⌘⇧Space** (Alt+Space is poisoned — inserts a non-breaking space). Toggle show/hide; check `is_registered` before re-register to survive stale bindings.

### 5.2 Terminal renderer (locked: custom, not xterm.js)

Keep/extend the existing custom DOM/canvas green-phosphor renderer in `src/`: typewriter output, optional scanline/CRT bloom (CSS `drop-shadow`), phosphor decay. Output-only (never a real shell), so xterm.js's VT emulator is unnecessary weight. Disable subpixel AA (`imageRendering: pixelated`) for crisp glyphs.

### 5.3 Screen router (states)

`Boot → Idle → Ask → War Room → Diagnosis (+evidence) → Fix preview`, driven by a tiny state machine in `main.ts` reacting to IPC events:
- **Boot**: Matrix-coded mount; `query_profile` → HUD shows live counts incl. **per-harness breakdown** ("47 Claude · 12 Codex sessions").
- **Idle**: ambient status; cached last diagnosis summary if present.
- **Ask**: text input (voice later). Canonical prompt suggested: *"what's wrong with how I use my agents?"*
- **War Room**: the React island takes the stage; driven by §4.2 events.
- **Diagnosis**: §7.
- **Fix preview**: §8.

---

## 6. The War Room — React + R3F + Remotion island

### 6.1 Visual identity (locked)
- **Wireframe Cells** (calm single-cage): each node = one wireframe icosahedron + a hot-white core. No double shells / vertex sparkle (rejected as overwhelming).
- **Constellation layout**: 3 large **stage nodes** (Diagnostician/Coach/Verifier) + a cloud of **candidate-hole nodes**; nearest-neighbor edges; traveling token-pulses; ambient dust for depth; `UnrealBloom` (strength ≈ 1.0), `ACESFilmic` tone-mapping, `FogExp2`, vignette + film grain.

### 6.2 Data → visual (honest, option ①)
| Real signal | Visual |
|---|---|
| `candidates_nominated` | spawn N candidate-hole nodes (emerald), one per candidate |
| `fugu_delta` | pulse intensity along edges (activity) |
| `fugu_usage` orchestration tokens | stage-node size + glow (degrades to plain tokens off-Fugu) |
| `finding_verdict: confirmed` | node flares **amber `#ff5a37`**, grows, persists |
| `finding_verdict: refuted` | node dims, collapses, dies |
| `diagnosis_ready` | surviving nodes animate into the diagnosis list |

### 6.3 Performance
Mount once on pre-warmed hidden window; `setPixelRatio(min(dpr,2))`; pause `requestAnimationFrame` on `visibilitychange`/hide; bounded node count (clamp/cluster if candidates ≫ ~24). anime.js drives DOM/screen transitions; R3F owns the canvas.

### 6.4 Harness differentiation in the viz (kept subtle)
Verdict drives the **core** color (the dramatic axis). Harness is a **secondary accent**: a thin rim/edge tint on the node's cage (Claude emerald, Codex violet) + a small legend. Avoids color doing double duty; keeps it readable, not busy.

### 6.5 Remotion (locked: all three roles)
1. **Live cinematic reveal** — `@remotion/player` drives the boot sequence and the diagnosis "slam-in" as frame-accurate compositions synced to real data (timeline cued by IPC events).
2. **Pre-rendered intro asset** — a short branded boot clip rendered at build time, played on first summon.
3. **Exportable diagnosis recap** — on demand, render the holes as a shareable cinematic.
   - **Feasibility note / risk R-Rem**: `@remotion/player` (React) runs natively in the webview. Programmatic **export** (`@remotion/renderer`) needs a render backend; in a Tauri desktop app that means a bundled Node/headless-Chromium sidecar (heavy) **or** a `MediaRecorder` canvas-capture fallback. M2 ships **(1)+(2)** fully and **(3) via the MediaRecorder fallback**, with `@remotion/renderer` as a post-M2 upgrade. Flagged in §11.

---

## 7. Diagnosis screen — full reveal + evidence drill-down (locked)

Renders from `diagnoses` + `findings` (M1 already produces these):
- **Ranked holes**, each: title, **severity bar** (1–5), **frequency**, **est. cost** (tokens/min), one-line summary, **harness badge**.
- **Do / Stop** lists + **narrative** (Coach output).
- **Evidence drill-down**: expand a finding → its `evidence_json` refs → resolve each `raw_ref` `(source_path, byte_offset|rowid)` back to the exact quote (via FTS5 / direct read) → show session · turn · quote. Every claim traceable to ground truth (`SPEC.md` §3).
- Degraded mode: no API key / budget cap → detector-only findings, clearly labeled (`SPEC.md` §8.5).

---

## 8. Fix preview — read-only (locked; apply = M4)

For each confirmed finding, render the **proposed** artifact as a unified **diff** against the real target (`CLAUDE.md` block, hook, skill, etc.) with a `[Y/n]` prompt — but **apply is disabled and labeled "coming in M4."** No file writes, no backups, nothing touches user projects this milestone. (Diff *generation* may reuse the Forge templates; only *application* is deferred.)

---

## 9. IPC contract (M2 surface)

**Commands (web→Rust):** `query_profile` (+harness breakdown), `run_diagnosis(scope)`, `get_diagnosis`, `get_findings`, `ask(query)`, `set_config(env/config.toml write)`, `get_fix_preview(finding_id)`. RADAR/voice/forge-apply commands remain present but return the existing `not_in_slice(...)` errors.
**Events (Rust→web):** `ingest_progress`, `candidates_nominated` *(new)*, `fugu_delta`, `fugu_usage`, `diagnosis_status`, `finding_verdict` *(new)*, `diagnosis_ready`.

---

## 10. Module / file plan

**Rust (`src-tauri/src/`)**
- `ingest/mod.rs` — formalize `Adapter` trait + `AdapterRegistry`.
- `ingest/claude_code.rs` — add FSEvents watch + byte-offset watermark tailing.
- `ingest/codex.rs` — **new** Codex adapter (§2.3) + golden tests.
- `brain.rs` — env-config base URL/model/key/effort; emit `candidates_nominated` + `finding_verdict`.
- `detectors.rs` — expose nominated candidate list for the viz event.
- `scheduler.rs` — **new** (or in `lib.rs`): live-ingest tasks + on-ask trigger, debounce, budget-aware.
- `lib.rs` — Accessory policy, tray, global shortcut, pre-warmed hidden overlay, spawn watchers.
- `commands.rs` — wire `query_profile` breakdown, `get_fix_preview`, `set_config`.
- `util.rs` — env helpers (brain base/model, codex path); `config.rs` minimal `~/.warden/config.toml` loader.

**Frontend**
- `src/main.ts`, `src/style.css`, `index.html` — shell: boot, terminal, HUD (+harness breakdown), screen router, event wiring.
- `src/viz/` — **new** React + R3F + Remotion island: `WarRoom.tsx` (scene), `compositions/` (Remotion: intro, reveal, recap), `bridge.ts` (Tauri event → scene state), `harnessTheme.ts`.
- `src/warRoom.ts` — retired/replaced by the island.
- `vite.config.ts` / `package.json` — add `@vitejs/plugin-react`, `react`, `react-dom`, `@react-three/fiber`, `three`, `@react-three/drei`, `@react-three/postprocessing`, `@remotion/player` (+ `@remotion/renderer` post-M2).

---

## 11. Build order (M2-internal)

1. **Engine env-config** (small; unblocks Near AI) + test.
2. **Adapter trait + registry**; **Codex adapter** + golden test.
3. **Live FSEvents ingest** (Claude+Codex) + watermarks.
4. **Daemon shell**: Accessory + tray + global hotkey + pre-warmed hidden overlay + click-through.
5. **IPC**: `candidates_nominated` + `finding_verdict`; `query_profile` breakdown.
6. **React/R3F island**: wireframe war room driven by real events.
7. **Remotion**: live reveal + intro asset + recap (MediaRecorder fallback).
8. **Diagnosis screen** + evidence drill-down + **read-only fix preview**.
9. **Harness differentiation** polish (HUD, badges, viz rim accents).
10. **Tests + e2e + spec self-review.**

---

## 12. Testing

- **Adapter golden** (`tests/fixtures/`): Codex `rollout-*.jsonl` → expected IR; Claude live-tail offset resume.
- **Engine env override**: base URL/model honored; missing key → detector-only path.
- **Event contract**: `candidates_nominated`/`finding_verdict` shapes; viz bridge maps them.
- **Viz smoke**: island mounts on hidden window; pauses RAF when hidden.
- **E2E**: seeded sample DB → `run_diagnosis` → asserts ranked findings with resolvable evidence refs + harness tags.

---

## 13. Risks & mitigations

- **R-Bundle** — R3F+Remotion inflate the bundle / summon latency. *Mitigate:* mount once on pre-warmed hidden window (summon = `show()`), pause RAF when hidden, lazy-load Remotion compositions.
- **R-Rem** — Remotion programmatic export needs a render backend. *Mitigate:* ship Player (live) + pre-rendered intro now; recap via `MediaRecorder` fallback; `@remotion/renderer` sidecar post-M2.
- **R-Codex** — Codex payload schema is internal/may drift. *Mitigate:* defensive `unknown → SystemNotice`; golden tests pinned to real fixtures.
- **R-Honesty** — non-Fugu engines lack orchestration tokens. *Mitigate:* viz degrades to delta pulses + plain weight; never fabricate.
- **R-NodeScale** — candidate count can be large. *Mitigate:* clamp/cluster nodes > ~24; HUD shows true count.
- **R-Color-a11y** — emerald vs violet harness accents. *Mitigate:* pair color with glyph + label, never color alone.

---

*End M2 spec. Next per process: user review of this document → `writing-plans` to produce the implementation plan. Subsequent milestones (M3 RADAR, M4 Forge, M5 Live, M6 Voice, M7 Adapters) each get their own spec; this document is M2 only.*
