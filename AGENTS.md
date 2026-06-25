# WARDEN — Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** daemon that ingests local
AI-coding transcripts (`~/.Codex/projects`, `~/.codex/sessions`), diagnoses agentic-workflow
anti-patterns through a Diagnostician→Coach→Verifier reasoning pipeline (**GLM-5.2** via **NEAR AI**), and renders a cinematic war-room +
evidence-cited diagnosis overlay summoned by a global hotkey.

## Milestones (7 total)
- **M0 — Spine** ✅ IR + Codex adapter + rusqlite/FTS5 store + featurizer (commit `d87497d`).
- **M1 — Brain** ✅ Diagnostician→Coach→Verifier pipeline (GLM-5.2 via NEAR AI, OpenAI-compatible) + detectors (commit `7ac9b10`).
- **M2 — Face** ✅ verified. Always-on daemon, ⌘⇧Space hotkey, pre-warmed overlay, R3F/Remotion
  war-room, diagnosis/evidence/fix-preview, **Codex adapter**, live FSEvents tailing, env-swappable
  engine, harness differentiation.
  - Spec: `docs/superpowers/specs/2026-06-22-m2-face-design.md`
  - Plan: `docs/superpowers/plans/2026-06-22-m2-face.md`
- **M3 — RADAR** ✅ built & merged to `dev`. **M4 — Forge(apply)** ⬅️ next (in design 2026-06-24).
  M5 Live · M6 Voice · M7 Adapters — future; **stubbed** via `scaffold::not_in_slice()`. Do NOT implement M5–M7.
- **DOSSIER ("Profile by Proof")** 💡 idea captured 2026-06-24, no spec yet. Longitudinal, evidence-cited
  profile of how the operator drives agents (orchestration/patterns/holes/strengths/where-they-lose/
  project-archetypes/trajectory) + all-time·6mo·3mo·30d·2wk window toggle; GLM-5.2 over BRAIN+RADAR.
  Milestone number unresolved (Karim said "M4" → collides with Forge). Capture: `docs/ideas/2026-06-24-dossier-profile-by-proof.md`. Do NOT implement until spec'd.

## How we work in this repo
- **Delegate discovery.** Broad file search / multi-file reads → dispatch Explore or general-purpose
  subagents and keep only the conclusion. Never inventory files in the main context.
- **Use skills maximally** for the FACE: `r3f-mastery`, `remotion`, `frontend-design`, anime.js.
- **Verify before claiming done.** Run the build + tests and read the real output. Evidence before
  assertions — see `superpowers:verification-before-completion`.
- **M2 is preview-only.** No writes to user projects, ever. Fix preview renders diffs only; apply = M4.
- **Never `git push` / open PR/MR** without Karim's explicit instruction in that specific message.
- Package manager is **pnpm**. Platform target: macOS Apple Silicon.

## Commands
| Goal | Command |
|---|---|
| Rust unit/golden tests | `cd src-tauri && cargo test` |
| Rust fast typecheck | `cd src-tauri && cargo check` |
| Rust build | `cd src-tauri && cargo build` |
| Frontend typecheck+bundle | `pnpm build`  (= `tsc && vite build`) |
| Full app (real e2e gate, slow) | `pnpm tauri build` |
| Dev run | `pnpm tauri dev` |

Env: `WARDEN_DB_PATH` (db override) · engine via `WARDEN_BRAIN_BASE_URL` + `WARDEN_BRAIN_API_KEY` (`OPENAI_*` fallback) + `WARDEN_BRAIN_DIAGNOSE_MODEL`/`_VERIFY_MODEL` (default `z-ai/glm-5.2`); see `.env.example`.

## Repo map
**Rust `src-tauri/src/`**
- `ir.rs` — canonical IR: `Harness`, `Session`, `Turn`, `Event` (11 variants), `EventRecord{raw_ref}`,
  `Finding`, `Diagnosis`, `EvidenceRef`, `FeatureVector`, `CompetenceProfile`, `RunScope`.
  **Single source of truth; every adapter maps raw → this IR.**
- `ingest/mod.rs` — `Adapter` trait + `SessionBatch`. *(M2: add `AdapterRegistry` + `watch`/`map`.)*
- `ingest/Codex.rs` — Codex backfill + per-file hash dedup. *(M2: add FSEvents tail + byte watermark.)*
- `ingest/codex.rs` — **M2 new** Codex adapter.
- `store.rs` — rusqlite + FTS5, 14 tables; `upsert_session_batch`, `counts`, `save_findings/diagnosis`,
  `latest_diagnosis`, `profile`, `source_raw_hash`; watermarks keyed by `source_path` with byte `offset`.
- `featurizer.rs` — FeatureVector + CompetenceProfile. `detectors.rs` — `nominate(store,profile)->Vec<Finding>`.
- `brain.rs` — engine client (GLM-5.2 via NEAR AI, OpenAI-compatible Chat Completions): `run_pipeline`, `diagnose/coach/verify`; emits legacy-named `fugu_delta`,`fugu_usage`.
  *(M2: env-config the base URL/models/key/effort; emit `candidates_nominated`,`finding_verdict`.)*
- `commands.rs` — `#[tauri::command]`s. Real: `query_profile`,`get_diagnosis`,`get_findings`,
  `run_diagnosis`,`ask`,`hide_overlay`,`get_fix_preview`,`resolve_evidence`,`set_config`.
  Stubs (return `not_in_slice`): apply/revert/voice/screen/fleet/`mute_pattern`.
- `scaffold.rs` — `not_in_slice(feature)` seam helper. `redaction.rs` — PII scrub.
- `lib.rs` — Tauri builder/`setup()`; `ActivationPolicy::Accessory`, tray menu, pre-warmed hidden
  `overlay` window, click-through idle state, blur/Esc dismissal, startup backfill, live watchers,
  and guarded ⌘⇧Space global shortcut.
- `util.rs` — `default_db_path()` is the **env-helper template** to copy for new env vars.
- M2 new: `config.rs` (`~/.warden/config.toml` loader), `scheduler.rs` (live-ingest tasks + on-ask trigger).

**Frontend `src/`**
- `index.html` — overlay DOM: `#war-room-root` R3F island mount, `#terminal`, `#screen`, `#prompt`/`#command`,
  HUD `#hud-{sessions,events,findings,stage}`, `#status`.
- `main.ts` — vanilla-TS screen router. Listens `warden_hotkey`,`ingest_progress`,`fugu_delta`,`fugu_usage`,
  `candidates_nominated`,`finding_verdict`,`diagnosis_ready`; invokes `query_profile`,`get_diagnosis`,`run_diagnosis`.
- `diagnosis.ts` — pure-DOM forensic readout: ranked holes, discrete severity meter, cost ledger, harness
  badges, evidence drill-down (`resolve_evidence` fallback), read-only fix-preview diff. jsdom-unit-tested.
- `warRoom.ts` — Three.js viz. *(M2: **retired**, replaced by the R3F island in `src/viz/`.)*
- `style.css` — green-phosphor tokens: `--bg #020403`, `--green #76ff9d`, `--dim #1b6f3a`,
  `--acid #b8ff6b`, `--warn #ffd166`, `--red #ff5470`, verdict `--amber #ff5a37`.
- M2 new `src/viz/` — React + R3F + Remotion island: `WarRoom.tsx`, `compositions/` (`Intro`/`Reveal`/`Recap`
  + pure `timing.ts` + shared `palette.ts`), `bridge.ts`, `harnessTheme.ts`, `PlayerHost.tsx`,
  mounted once into `#war-room-root` on the pre-warmed hidden window.

## Conventions
- **Env helper**: `std::env::var("X").ok().map(...).unwrap_or_else(default)` (see `util.rs`).
- **IPC**: commands web→Rust via `invoke`; events Rust→web via `app.emit(name, json!{...})`.
- **Harness theme is one source of truth**: Codex = emerald `#3dffa0`, Codex = violet `#b98cff`,
  verdict-amber `#ff5a37`. Always pair color with a glyph + label (color-blind a11y).
- **Adapter contract**: adding a harness = one adapter, zero downstream changes. Unknown record →
  `Event::SystemNotice` (schema drift never drops a session).
- **Watermarks are byte-offset.** FSEvents coalesces rapid writes — on each event, seek to the saved
  offset and read all bytes to EOF; do not trust event counts.
- **Honest viz**: war-room nodes/flares map to *real* signals (candidate count, token deltas, verdicts).
  Engines without `orchestration_*` tokens (the current GLM-5.2/NEAR AI brain) → degrade to delta pulses + plain weight, never fake.

## External transcript layouts (confirmed on this machine)
- Codex: `~/.Codex/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (+ `~/.codex/archived_sessions/**`).
  Envelope: `{timestamp, type, payload}`. `token_count` nests under `payload.info.last_token_usage`
  (`input_tokens`,`cached_input_tokens`,`output_tokens`). See plan §2.3 for the full record→IR table.
