# WARDEN: Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** app that tails your
local AI-coding transcripts (`~/.claude/projects`, `~/.codex/sessions`) and renders every
active and idle agent as a live 3D radar. It is read-only and fully local: no network, no
API keys, no writes to your projects.

## How we work in this repo
- **Delegate discovery.** For broad file search or multi-file reads, dispatch Explore or
  general-purpose subagents and keep only the conclusion. Do not inventory files in the
  main context.
- **Verify before claiming done.** Run the build and tests and read the real output.
  Evidence before assertions.
- **Read-only, always.** WARDEN never writes to your watched projects.
- **Never `git push` or open a PR** without an explicit instruction in that message.
- Package manager is **pnpm**. Platform target: macOS Apple Silicon. OS-specific code is
  isolated in `platform/`, ready for future ports.

## Commands
| Goal | Command |
|---|---|
| Rust unit/golden tests | `cd src-tauri && cargo test` |
| Rust fast typecheck | `cd src-tauri && cargo check` |
| Rust lint (denies prod `unwrap()`) | `cd src-tauri && cargo clippy` |
| Frontend typecheck + bundle | `pnpm build` (= `tsc && vite build`) |
| Frontend tests | `pnpm test` (vitest) |
| Frontend import-boundary check | `pnpm check:arch` |
| Dev run (full app) | `pnpm tauri dev` |
| Full app bundle (real e2e gate, slow) | `pnpm tauri build` |
| Radar sandbox in a browser | `pnpm dev`, then open `/radar-lab.html` |

Toolchain pinned in `src-tauri/rust-toolchain.toml` (stable >= 1.85, for edition2024 deps).
Env is all OPTIONAL (see `.env.example`): `WARDEN_DB_PATH` plus the transcript-root
overrides `WARDEN_CLAUDE_PROJECTS` / `WARDEN_CODEX_SESSIONS`. No API keys: the radar is
fully local.

## Repo map
**Rust `src-tauri/src/`**: a single crate; `tauri` is confined to `lib.rs` / `commands.rs`.
Layered `ingest -> store -> radar -> commands/lib/scheduler`.
- `ir.rs` the canonical IR (single source of truth; every adapter maps raw records to this).
- `store.rs` rusqlite + FTS5 (sessions/turns/events/watermarks/radar_token_cache), byte-offset watermarks.
- `ingest/` the `Adapter` trait + `AdapterRegistry` + `claude_code.rs` / `codex.rs`. Adding a harness is one adapter, zero downstream changes.
- `radar.rs` + `radar/` the live agent forest: a façade over `model/assemble/agent/context/identity/live/status` + `composition/hierarchy/liveness`.
- `scheduler.rs` + `scheduler/` the task drivers: `watch` (live-ingest) and `radar` (recompute + `RadarStateCache`).
- `util.rs` env + path helpers; `platform/` the OS seam (port + `macos.rs` + `fallback.rs`).
- `lib.rs` the Tauri builder / `setup()` (visible window on launch, tray, hotkey, startup backfill, watchers); `commands.rs` the `#[tauri::command]`s.

**Frontend `web/`**: an FSD-lite island; imports point DOWN only (`app -> views -> modules -> shared`, enforced by `pnpm check:arch`); the `@/` alias maps to `web/`.
- `index.html` (the `#war-room-root` mount); `main.ts` the Tauri event router; `style.css` green-phosphor tokens (`--bg #020403`, `--green #76ff9d`).
- `web/viz/`: `app/` (mount); `views/war-room/` (WarRoom + FilterBar + Breadcrumb); `modules/radar/` (the constellation, layout, detail panel, hover card, theme); `shared/{state,types,theme,scene,lib}` (`bridge.ts` is the pure reducer in `state/`); `dev/preview/` (radar sandboxes).

## Conventions
- **Env helper**: `std::env::var("X").ok().map(...).unwrap_or_else(default)` (see `util.rs`).
- **IPC**: commands go web to Rust via `invoke`; events go Rust to web via `app.emit(name, json!{...})`.
- **Harness theme is one source of truth**: Claude is emerald, Codex is violet. Always pair colour with a glyph and label (color-blind a11y).
- **Adapter contract**: adding a harness is one adapter, zero downstream changes. An unknown record degrades gracefully; schema drift never drops a session.
- **Watermarks are byte-offset.** FSEvents coalesces rapid writes: on each event, seek to the saved offset and read to EOF; do not trust event counts.
- **Honest viz**: every globe and flare maps to a REAL signal (session liveness, context-token weight, subagent hierarchy). Never fabricate a count or a link.
- **FSD layering (frontend)**: imports point DOWN only; no sibling-module imports; `dev/` is exempt. Use the `@/` alias; colocate tests; no app-wide barrels.
- **Rust module form**: `name.rs` + `name/` with a slim façade re-exporting a narrow public API (not a glob `pub use *`); cross-submodule internals are `pub(crate)`.
- **No production `unwrap()`**: clippy denies it (`unwrap_used = "deny"`); use `.expect("invariant")` for true invariants and `?` to propagate. Tests are exempt. `anyhow` everywhere.
- **Platform isolation**: all OS-specific runtime code lives in `platform/`; no `#[cfg(target_os)]` scattered elsewhere.

## External transcript layouts
- Claude: `~/.claude/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (plus `~/.codex/archived_sessions/**`). Envelope: `{timestamp, type, payload}`.
