# WARDEN — Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** daemon that ingests local
AI-coding transcripts (Claude Code, Codex, Claude Desktop), diagnoses agentic-workflow
anti-patterns through a Diagnostician→Coach→Verifier reasoning pipeline (**GLM-5.2** via **NEAR AI**), and renders a cinematic war-room +
evidence-cited diagnosis overlay summoned by a global hotkey.

## Milestones (M0–M4 + DOSSIER shipped; M5–M7 stubbed)
- **M0 — Spine** ✅ IR + Claude adapter + rusqlite/FTS5 store + featurizer (commit `d87497d`).
- **M1 — Brain** ✅ Diagnostician→Coach→Verifier pipeline (GLM-5.2 via NEAR AI, OpenAI-compatible) + detectors (commit `7ac9b10`).
- **M2 — Face** ✅ verified. Always-on daemon, global-hotkey overlay, pre-warmed overlay, R3F/Remotion
  war-room, diagnosis/evidence/fix-preview, **Codex adapter**, live FSEvents tailing, env-swappable
  engine, harness differentiation.
  - Spec: `docs/superpowers/specs/2026-06-22-m2-face-design.md` · Plan: `docs/superpowers/plans/2026-06-22-m2-face.md`
- **M3 — RADAR** ✅ built & merged: live agent-fleet presence map (liveness, hierarchy, context size, composition).
- **M4 — Forge(apply)** ✅ built & merged (commit `9dee88d`): applies the WARDEN guardrail block to the
  user's agent-config surface (`~/.claude/CLAUDE.md`) with integrity-checked backup + revert.
  - Spec: `docs/superpowers/specs/2026-06-25-m4-forge-design.md` · Plan: `docs/superpowers/plans/2026-06-25-m4-forge.md`
- **DOSSIER ("Profile by Proof")** ✅ built & merged (commit `13830b1`). Sits **outside the M-series**
  (M4 remains Forge). Longitudinal, evidence-cited profile of how the operator drives agents
  (orchestration/patterns/holes/strengths/where-they-lose/project-archetypes/trajectory) +
  all-time·6mo·3mo·30d·2wk window toggle; GLM-5.2 over BRAIN+RADAR. Backend `src-tauri/src/dossier/**`,
  frontend `src/viz/profile/**`.
  - Spec: `docs/superpowers/specs/2026-06-27-dossier-profile-by-proof-design.md` · Plan: `docs/superpowers/plans/2026-06-27-dossier.md`
- **Claude Desktop adapter** ✅ (commit `e944f95`) — third ingest source: desktop local-agent-mode transcripts.
- M5 Live · M6 Voice · M7 Adapters — future; **stubbed** via `scaffold::not_in_slice()`. Do NOT implement M5–M7.

## How we work in this repo
- **Delegate discovery.** Broad file search / multi-file reads → dispatch Explore or general-purpose
  subagents and keep only the conclusion. Never inventory files in the main context.
- **Use skills maximally** for the FACE: `r3f-mastery`, `remotion`, `frontend-design`, anime.js.
- **Verify before claiming done.** Run the build + tests and read the real output. Evidence before
  assertions — see `superpowers:verification-before-completion`.
- **Writes to user files are Forge-only.** The ONLY sanctioned write surface is the WARDEN guardrail
  block in `~/.claude/CLAUDE.md` via forge apply/revert (atomic, backed up). Everything else renders
  previews/diffs only — never write to user projects.
- **Never `git push` / open PR/MR** without Karim's explicit instruction in that specific message.
- Package manager is **pnpm**. Platform target: macOS Apple Silicon.

## Commands
| Goal | Command |
|---|---|
| Rust unit/golden tests | `cd src-tauri && cargo test` |
| Rust fast typecheck | `cd src-tauri && cargo check` |
| Rust build | `cd src-tauri && cargo build` |
| Frontend typecheck+bundle | `pnpm build`  (= `tsc && vite build`) |
| Frontend unit tests | `pnpm test`  (= `vitest run`; jsdom per-file via pragma) |
| Full app (real e2e gate, slow) | `pnpm tauri build` |
| Dev run | `pnpm tauri dev`  (vite pinned to **1421**, strictPort; `WARDEN_DEV_PORT` moves both sides) |
| Dev run, auto-port | `pnpm dev:app`  (= `scripts/warden-dev.mjs`, picks a free port for vite+tauri in lockstep) |

Headless verification (no GUI): examples `dossier_smoke` (build_profile on a **copied** DB, blank
`WARDEN_BRAIN_API_KEY` ⇒ deterministic detector-only) and `radar_probe` (live radar snapshot →
writes `src/viz/preview/realRadar.json`); CLI `cargo run --bin warden -- ingest|detectors|diagnose`.
Browser-only FACE QA via `pnpm dev`: `/dev-viz.html` (scripted mock event loop), `/dev-warroom.html`
(static scene), `preview/radarReal.tsx` (real captured snapshot). Always point parallel runs at a DB
copy via `WARDEN_DB_PATH` — the installed daemon keeps `~/.warden/warden.db` open.

Env (full defaults in `.env.example`): core — `WARDEN_DB_PATH`, `WARDEN_CONFIG_PATH` ·
engine — `WARDEN_BRAIN_BASE_URL`+`WARDEN_BRAIN_API_KEY` (`OPENAI_*` fallback, see `util.rs:231`),
`WARDEN_BRAIN_DIAGNOSE_MODEL`/`_VERIFY_MODEL` (default `z-ai/glm-5.2`), `_STRUCTURED_OUTPUT`, `_STREAM`,
`_TRANSPORT`, `_TIMEOUT_SECS`, `_CURL_TIMEOUT_SECS` ·
ingest paths — `WARDEN_CLAUDE_PROJECTS`, `WARDEN_CLAUDE_SESSIONS`, `WARDEN_CLAUDE_MD`,
`WARDEN_CLAUDE_DESKTOP_SESSIONS`, `WARDEN_CODEX_SESSIONS`, `WARDEN_CODEX_ARCHIVED_SESSIONS`,
`WARDEN_MAX_FILES`, `WARDEN_WATCH_DEBOUNCE_MS` ·
RADAR — `WARDEN_RADAR_TICK_MS`, `_DEBOUNCE_MS`, `_CODEX_STALE_HRS`, `_ROOT_IDLE_SECS`, `_WORKING_MS`,
`_WORKING_STALE_SECS`, `_SUBAGENT_TERMINATE_MS`, `_TERMINATE_GRACE_MS` ·
Habits — `WARDEN_HABITS_TICK_MS`, `_DEBOUNCE_MS`, `_ERASE_WINDOW` ·
Dossier — `WARDEN_DOSSIER_DISABLE_EMBEDDINGS`, `_ENABLE_EMBEDDINGS_IN_TEST`.

## Repo map
**Rust `src-tauri/src/`**
- `ir.rs` — canonical IR: `Harness`, `Session`, `Turn`, `Event` (11 variants), `EventRecord{raw_ref}`,
  `Finding`, `Diagnosis`, `EvidenceRef`, `FeatureVector`, `CompetenceProfile`, `RunScope`.
  **Single source of truth; every adapter maps raw → this IR.**
- `ingest/mod.rs` — `Adapter` trait + `SessionBatch` + `AdapterRegistry` + post-ingest subagent linkage.
- `ingest/claude_code.rs` — Claude Code backfill + per-file hash dedup + FSEvents tail + byte watermark.
- `ingest/codex.rs` — Codex adapter. `ingest/claude_desktop.rs` — Claude Desktop adapter: one interleaved
  `audit.jsonl` → many IR sessions split by `session_id`; dir-owner heuristic links subagents; sessions
  are `Harness::ClaudeCode` + meta `originator:"Claude Desktop"` (FACE shows a "Desktop" badge).
- `store.rs` — rusqlite + FTS5, 19 tables (incl. `artifacts` for forge, `dossier_*`/`profile_history`
  for DOSSIER, `radar_token_cache`); `upsert_session_batch`, `counts`, `save_findings/diagnosis`,
  `latest_diagnosis`, `profile`, `source_raw_hash`; watermarks keyed by `source_path` with byte `offset`.
- `featurizer.rs` — FeatureVector + CompetenceProfile. `detectors.rs` — `nominate(store,profile)->Vec<Finding>`.
- `brain.rs` — engine client (GLM-5.2 via NEAR AI, OpenAI-compatible Chat Completions): `run_pipeline`,
  `diagnose/coach/verify`; emits legacy-named `fugu_delta`,`fugu_usage` + `candidates_nominated`,`finding_verdict`.
- `radar/` — M3 live fleet map: `hierarchy.rs` (orchestrator/subagent grouping incl. desktop dir-owner
  linking), `liveness.rs`, `composition.rs`, `mod.rs` (snapshot assembly, `identity()` naming, live refresh).
- `forge.rs` — M4 artifact IR + apply/revert (atomic temp+rename, integrity-checked backup, idempotent `ensure_block`).
- `dossier/` — DOSSIER: `build.rs` orchestration, `aggregate/archetype/cluster/efficiency/heatmap/outcome/
  proof/rubric/scope(Window enum)/summarize/synthesize/trajectory/types`.
- `habits.rs` — Living-Habits windowed dial/heartbeat/streaks. `harness_theme.rs` — per-harness color/glyph
  source of truth (Rust side). `window.rs` — window-management helpers.
- `commands.rs` — 32 `#[tauri::command]`s; authoritative list = `generate_handler!` in `lib.rs`. Still
  stubbed via `not_in_slice`: `start_voice`,`stop_voice`,`capture_screen`,`mute_pattern`,`locate_agent`,
  `warp_to_agent` (M5/M6 seams). Everything else — incl. forge `stage/apply/revert/get/list_artifact` and
  dossier `build_profile`,`get_profile`,`get_activity_heatmap`,`get_efficiency_score` — is real.
- `scaffold.rs` — `not_in_slice(feature)` seam helper. `redaction.rs` — PII scrub.
- `lib.rs` — Tauri builder/`setup()`; `ActivationPolicy::Accessory`, tray menu (Summon/Status/Quit),
  pre-warmed hidden `overlay` window, click-through idle state, blur/Esc dismissal, startup backfill
  (async, non-blocking), live watchers, guarded **⌘⌥⌃M** global shortcut (replaced old ⌘⇧Space),
  and an unconditional `summon_overlay` at the end of `setup()` — booting the app shows the overlay.
- `bin/warden_cli.rs` — headless CLI (`cargo run --bin warden -- ingest|detectors|diagnose`).
- `util.rs` — `default_db_path()` is the **env-helper template** to copy for new env vars.
- `config.rs` — `~/.warden/config.toml` loader. `scheduler.rs` — live-ingest tasks, on-ask trigger,
  RADAR debounced recompute/cache, Habits ticking.

**Frontend `src/`**
- `index.html` — overlay DOM: `#war-room-root` R3F island mount, `#terminal`, `#screen`, `#prompt`/`#command`,
  HUD `#hud-{sessions,events,findings,stage}`, `#status`.
- `main.ts` — vanilla-TS screen router. Listens `warden_hotkey`,`ingest_progress`,`fugu_delta`,`fugu_usage`,
  `candidates_nominated`,`finding_verdict`,`diagnosis_ready`; invokes `query_profile`,`get_diagnosis`,`run_diagnosis`.
- `diagnosis.ts` — pure-DOM forensic readout: ranked holes, discrete severity meter, cost ledger, harness
  badges, evidence drill-down (`resolve_evidence` fallback), read-only fix-preview diff. jsdom-unit-tested.
- `style.css` — green-phosphor tokens: `--bg #020403`, `--green #76ff9d`, `--dim #1b6f3a`,
  `--acid #b8ff6b`, `--warn #ffd166`, `--red #ff5470`, verdict `--amber #ff5a37`.
- `src/viz/` — React + R3F + Remotion island, mounted once into `#war-room-root` on the pre-warmed hidden
  window. Core: `WarRoom.tsx`, `compositions/` (Intro/Reveal/Recap + pure `timing.ts` + shared `palette.ts`),
  `bridge.ts`, `harnessTheme.ts`/`harnessColors.ts`, `PlayerHost.tsx`, `Orb.tsx`/`AgentCore.tsx`/
  `Constellation.tsx`/`StarCatalog.tsx`, `chrome.tsx`, `NavBar.tsx`/`Sidebar.tsx`/`FilterBar.tsx`.
  **RADAR**: `RadarConstellation.tsx`, `RadarHoverCard.tsx`, `RadarDetailPanel.tsx`,
  `radarLayout/radarLifecycle/radarTheme/radarTypes.ts`. **DOSSIER**: `profile/` (`ProfileScreen.tsx`,
  `heatmap.ts`, `mount.tsx`, `types.ts`). Gesture: `gesture/` (hand-tracked orbit). Dev harnesses:
  `preview/` (`orbLab`,`radarLab`,`radarReal`), `devWarRoom.tsx`.

## Conventions
- **Env helper**: `std::env::var("X").ok().map(...).unwrap_or_else(default)` (see `util.rs`).
- **IPC**: commands web→Rust via `invoke`; events Rust→web via `app.emit(name, json!{...})`.
- **Harness theme is one source of truth**: Claude = emerald `#3dffa0`, Codex = violet `#b98cff`,
  verdict-amber `#ff5a37`. Always pair color with a glyph + label (color-blind a11y). Claude Desktop
  rides the `claude_code` identity + an origin badge (`radarOriginBadge`), not a fourth color.
- **Adapter contract**: adding a harness = one adapter, zero downstream changes. Unknown record →
  `Event::SystemNotice` (schema drift never drops a session).
- **Watermarks are byte-offset.** FSEvents coalesces rapid writes — on each event, seek to the saved
  offset and read all bytes to EOF; do not trust event counts.
- **Honest viz**: war-room nodes/flares map to *real* signals (candidate count, token deltas, verdicts).
  Engines without `orchestration_*` tokens (the current GLM-5.2/NEAR AI brain) → degrade to delta pulses + plain weight, never fake.

## External transcript layouts (confirmed on this machine)
- Claude Code: `~/.claude/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (+ `~/.codex/archived_sessions/**`).
  Envelope: `{timestamp, type, payload}`. `token_count` nests under `payload.info.last_token_usage`
  (`input_tokens`,`cached_input_tokens`,`output_tokens`). See plan §2.3 for the full record→IR table.
- Claude Desktop: `~/Library/Application Support/Claude/local-agent-mode-sessions/<workspace>/<context>/
  local_<uuid>/audit.jsonl` — JSONL, one Anthropic message object per line; subagents interleave into the
  SAME file (unlike Claude Code's per-file split). Ignore sibling `local_<uuid>.json` and nested
  `.claude/projects/*.jsonl` (no ingestible stream). Adapter: `ingest/claude_desktop.rs`.
