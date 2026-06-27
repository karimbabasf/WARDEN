# WARDEN — Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** daemon that ingests local
AI-coding transcripts (`~/.claude/projects`, `~/.codex/sessions`), diagnoses agentic-workflow
anti-patterns through a Diagnostician→Coach→Verifier reasoning pipeline (**GLM-5.2** via **NEAR AI**), and renders a cinematic war-room +
evidence-cited diagnosis overlay summoned by a global hotkey.

## Milestones (7 total)
- **M0 — Spine** ✅ IR + Claude adapter + rusqlite/FTS5 store + featurizer (commit `d87497d`).
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
- Package manager is **pnpm**. Platform target: macOS Apple Silicon — OS-specific code is isolated in `platform/`, ready for future ports.

## Commands
| Goal | Command |
|---|---|
| Rust unit/golden tests | `cd src-tauri && cargo test` |
| Rust fast typecheck | `cd src-tauri && cargo check` |
| Rust build | `cd src-tauri && cargo build` |
| Frontend typecheck+bundle | `pnpm build`  (= `tsc && vite build`) |
| Full app (real e2e gate, slow) | `pnpm tauri build` |
| Dev run | `pnpm tauri dev` |
| Rust lint (denies prod `unwrap()`) | `cd src-tauri && cargo clippy` |
| Frontend import-boundary check | `pnpm check:arch` |

Toolchain pinned in `src-tauri/rust-toolchain.toml` (stable ≥ 1.85, for edition2024 deps).

Env: `WARDEN_DB_PATH` (db override) · engine via `WARDEN_BRAIN_BASE_URL` + `WARDEN_BRAIN_API_KEY` (`OPENAI_*` fallback) + `WARDEN_BRAIN_DIAGNOSE_MODEL`/`_VERIFY_MODEL` (default `z-ai/glm-5.2`); see `.env.example`.

## Repo map
> Full codemap (radar/scheduler submodule breakdown + the FSD import rule): **`ARCHITECTURE.md`**.
> Refactor decision log + rationale: **`REFACTOR.md`**.

**Rust `src-tauri/src/`** — single crate; `tauri` confined to `lib.rs`/`commands.rs`. Layered
`ingest → store → (featurizer · detectors · brain · forge · habits · radar) → commands/lib/scheduler`.
- `ir.rs` canonical IR (single source of truth; every adapter maps raw → this) · `store.rs` rusqlite + FTS5
  (16 tables, byte-offset watermarks) · helpers `util.rs` (env-helper template)/`config.rs`/`redaction.rs`/`scaffold.rs`/`window.rs`/`harness_theme.rs` · `platform/` (OS seam — macOS adapter today).
- `ingest/` — `Adapter` trait + `AdapterRegistry` + `claude_code.rs`/`codex.rs`. Top-level (consumed by
  brain/commands/scheduler/radar/cli); adding a harness = one adapter, zero downstream changes.
- `featurizer.rs` + `detectors.rs` (`nominate(store,profile)->Vec<Finding>`) · `brain.rs` (GLM-5.2 pipeline;
  emits `fugu_delta`/`fugu_usage`) · `forge.rs` (M4 fix-preview + apply) · `habits.rs` (Living-Habits streak).
- `radar.rs` + `radar/` — live agent-forest: façade + `model/assemble/agent/context/identity/live/status` + `composition/hierarchy/liveness`.
- `scheduler.rs` + `scheduler/` — task drivers: `watch` (live-ingest) · `radar` (recompute + `RadarStateCache`) · `habits` (heartbeat).
- `lib.rs` Tauri builder/`setup()` (tray, pre-warmed hidden `overlay`, ⌘⌥⌃M hotkey, startup backfill, watchers;
  OS-specific bits via `platform::`) · `commands.rs` `#[tauri::command]`s (M5+ stubs → `not_in_slice`) · `bin/warden_cli.rs`.

**Frontend `src/`** — FSD-lite island; imports point DOWN only (`app → views → modules → shared`, `pnpm check:arch` enforces); `@/` alias → `src/`.
- `index.html` (`#war-room-root` mount, HUD `#hud-{sessions,events,findings,stage}`, `#status`) · `main.ts`
  vanilla-TS Tauri router · `style.css` green-phosphor tokens (`--bg #020403`, `--green #76ff9d`, verdict `--amber #ff5a37`).
- `src/viz/`: `app/` (mount) · `views/war-room/` (WarRoom + chrome/NavBar/FilterBar/Sidebar) ·
  `modules/{habits,radar,diagnosis,cinematics}` (no sibling-module imports; `diagnosis` = the pure-DOM forensic
  readout; lazy Remotion in `cinematics/`) · `shared/{state,types,theme,scene,lib}` (`bridge.ts` pure reducer in `state/`) · `dev/`.

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
  Engines without `orchestration_*` tokens (the current GLM-5.2/NEAR AI brain) → degrade to delta pulses + plain weight, never fake.
- **FSD layering (frontend)**: imports point DOWN only — `app → views → modules → shared`; no sibling-module
  imports; `dev/` exempt. Enforced by `pnpm check:arch`. Use the `@/` alias; colocate tests; no app-wide barrels.
- **Rust module form**: `name.rs` + `name/` with a slim façade re-exporting a *narrow* public API (not a glob
  `pub use *`); cross-submodule internals are `pub(crate)`.
- **No production `unwrap()`**: clippy denies it (`unwrap_used = "deny"`); use `.expect("invariant")` for true
  invariants and `?` to propagate. Tests are exempt (`clippy.toml`). `anyhow` everywhere (no `thiserror`).
- **Platform isolation**: all OS-specific runtime code lives in `platform/` (port + `macos.rs` adapter + `fallback.rs`);
  no `#[cfg(target_os)]` scattered elsewhere. Adding an OS = one adapter file + a `tauri.conf.json` target (see `ARCHITECTURE.md`).

## External transcript layouts (confirmed on this machine)
- Claude: `~/.claude/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (+ `~/.codex/archived_sessions/**`).
  Envelope: `{timestamp, type, payload}`. `token_count` nests under `payload.info.last_token_usage`
  (`input_tokens`,`cached_input_tokens`,`output_tokens`). See plan §2.3 for the full record→IR table.
