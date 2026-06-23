# WARDEN — Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** daemon that ingests local
AI-coding transcripts (`~/.claude/projects`, `~/.codex/sessions`), diagnoses agentic-workflow
anti-patterns through the Sakana **Fugu** reasoning pipeline, and renders a cinematic war-room +
evidence-cited diagnosis overlay summoned by a global hotkey.

## Milestones (7 total)
- **M0 — Spine** ✅ IR + Claude adapter + rusqlite/FTS5 store + featurizer (commit `d87497d`).
- **M1 — Brain** ✅ Fugu Diagnostician→Coach→Verifier pipeline + detectors (commit `7ac9b10`).
- **M2 — Face** ⬅️ *current*. Always-on daemon, ⌘⇧Space hotkey, pre-warmed overlay, R3F/Remotion
  war-room, diagnosis/evidence/fix-preview, **Codex adapter**, live FSEvents tailing, env-swappable
  engine, harness differentiation.
  - Spec: `docs/superpowers/specs/2026-06-22-m2-face-design.md`
  - Plan: `docs/superpowers/plans/2026-06-22-m2-face.md`
- M3 RADAR · M4 Forge(apply) · M5 Live · M6 Voice · M7 Adapters — future; **stubbed** via
  `scaffold::not_in_slice()`. Do NOT implement these.

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

Env: `WARDEN_DB_PATH` (db override) · `SAKANA_API_KEY` (Fugu key) · M2 adds `WARDEN_BRAIN_*` (see plan §4).

## Repo map
**Rust `src-tauri/src/`**
- `ir.rs` — canonical IR: `Harness`, `Session`, `Turn`, `Event` (11 variants), `EventRecord{raw_ref}`,
  `Finding`, `Diagnosis`, `EvidenceRef`, `FeatureVector`, `CompetenceProfile`, `RunScope`.
  **Single source of truth; every adapter maps raw → this IR.**
- `ingest/mod.rs` — `Adapter` trait + `SessionBatch`. *(M2: add `AdapterRegistry` + `watch`/`map`.)*
- `ingest/claude_code.rs` — Claude backfill + per-file hash dedup. *(M2: add FSEvents tail + byte watermark.)*
- `ingest/codex.rs` — **M2 new** Codex adapter.
- `store.rs` — rusqlite + FTS5, 14 tables; `upsert_session_batch`, `counts`, `save_findings/diagnosis`,
  `latest_diagnosis`, `profile`, `source_raw_hash`; watermarks keyed by `source_path` with byte `offset`.
- `featurizer.rs` — FeatureVector + CompetenceProfile. `detectors.rs` — `nominate(store,profile)->Vec<Finding>`.
- `brain.rs` — Fugu client: `run_pipeline`, `diagnose/coach/verify`; emits `fugu_delta`,`fugu_usage`.
  *(M2: env-config the base URL/models/key/effort; emit `candidates_nominated`,`finding_verdict`.)*
- `commands.rs` — `#[tauri::command]`s. Real: `query_profile`,`get_diagnosis`,`get_findings`,
  `run_diagnosis`,`ask`. Stubs (return `not_in_slice`): apply/revert/voice/screen/fleet/`set_config`/`mute_pattern`.
- `scaffold.rs` — `not_in_slice(feature)` seam helper. `redaction.rs` — PII scrub.
- `lib.rs` — Tauri builder/`setup()`; registers `tauri_plugin_global_shortcut` (currently the *poisoned*
  Alt+Space → change to ⌘⇧Space) and shows a visible `main` window.
  *(M2: `ActivationPolicy::Accessory` + tray + pre-warmed hidden `overlay` window + spawn watchers.)*
- `util.rs` — `default_db_path()` is the **env-helper template** to copy for new env vars.
- M2 new: `config.rs` (`~/.warden/config.toml` loader), `scheduler.rs` (live-ingest tasks + on-ask trigger).

**Frontend `src/`**
- `index.html` — overlay DOM: `#war-room` canvas, `#terminal`, `#screen`, `#prompt`/`#command`,
  HUD `#hud-{sessions,events,findings,stage}`, `#status`.
- `main.ts` — vanilla-TS screen router. Listens `warden_hotkey`,`ingest_progress`,`fugu_delta`,`fugu_usage`;
  invokes `query_profile`,`get_diagnosis`,`run_diagnosis`.
- `warRoom.ts` — Three.js viz. *(M2: **retired**, replaced by the R3F island in `src/viz/`.)*
- `style.css` — green-phosphor tokens: `--bg #020403`, `--green #76ff9d`, `--dim #1b6f3a`,
  `--acid #b8ff6b`, `--warn #ffd166`, `--red #ff5470`.
- M2 new `src/viz/` — React + R3F + Remotion island: `WarRoom.tsx`, `compositions/`, `bridge.ts`,
  `harnessTheme.ts`, mounted once into `#war-room-root` on the pre-warmed hidden window.

## Conventions
- **Env helper**: `std::env::var("X").ok().map(...).unwrap_or_else(default)` (see `util.rs`).
- **IPC**: commands web→Rust via `invoke`; events Rust→web via `app.emit(name, json!{...})`.
- **Harness theme is one source of truth**: Claude = emerald `#3dffa0`, Codex = violet `#b98cff`,
  verdict-amber `#ff5a37`. Always pair color with a glyph + label (color-blind a11y).
- **Adapter contract**: adding a harness = one adapter, zero downstream changes. Unknown record →
  `Event::SystemNotice` (schema drift never drops a session).
- **Watermarks are byte-offset.** FSEvents coalesces rapid writes — on each event, seek to the saved
  offset and read all bytes to EOF; do not trust event counts.
- **Honest viz**: war-room nodes/flares map to *real* signals (candidate count, token deltas, verdicts).
  Off-Fugu engines lack `orchestration_*` tokens → degrade to delta pulses + plain weight, never fake.

## External transcript layouts (confirmed on this machine)
- Claude: `~/.claude/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (+ `~/.codex/archived_sessions/**`).
  Envelope: `{timestamp, type, payload}`. `token_count` nests under `payload.info.last_token_usage`
  (`input_tokens`,`cached_input_tokens`,`output_tokens`). See plan §2.3 for the full record→IR table.
