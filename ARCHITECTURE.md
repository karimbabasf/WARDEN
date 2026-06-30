# ARCHITECTURE — WARDEN codemap

A map of where things live and how they fit, to orient a new reader. Keep it in sync when the
structure changes (it is referenced from `CLAUDE.md`). Entities are named by symbol so `grep` stays
the source of truth. For the *why* behind the current layout, see `REFACTOR.md`.

WARDEN ingests local AI-coding transcripts → normalizes to a canonical IR → persists in SQLite →
computes features + runs detectors → runs an LLM diagnosis pipeline → serves results to a cinematic
overlay (R3F/Remotion), plus a live "RADAR" agent-forest and a "Living-Habits" streak engine.

---

## Backend — `src-tauri/src/` (single crate, `tauri` confined to the starred files)

**Data flow (down = depends-on):**
```
commands.rs* / lib.rs* / scheduler*          (Tauri shell: IPC, setup, task drivers)
        │
   brain.rs · detectors.rs · featurizer.rs · forge.rs · habits.rs · radar*   (analysis / domain)
        │
   store.rs            (rusqlite + FTS5, 16 tables, byte-offset watermarks)
        │
   ingest/             (Adapter trait → raw transcripts mapped to IR)
        │
   ir.rs               (canonical IR — single source of truth)
```

**Foundations**
- `ir.rs` — canonical IR: `Harness`, `Session`, `Turn`, `Event` (11 variants), `EventRecord{raw_ref}`, `Finding`, `Diagnosis`, `EvidenceRef`, `FeatureVector`, `CompetenceProfile`, `Artifact`, `RunScope`. Every adapter maps raw → this.
- `store.rs` — rusqlite + FTS5 (16 tables); `upsert_session_batch`, `counts`, `save_findings/diagnosis`, `latest_diagnosis`, `profile`, radar cache, watermarks keyed by `source_path` with byte `offset`.
- `util.rs` — env-helper template (`default_db_path` etc.), `stable_id`, `hash64`, `expand_tilde`. `config.rs` — `~/.warden/config.toml`. `redaction.rs` — PII scrub. `scaffold.rs` — `not_in_slice()` seam (M5–M7). `window.rs` — time-window math. `harness_theme.rs` — colour/glyph/label.

**Ingest — `ingest/` (the one real extension point)**
- `ingest/mod.rs` — `Adapter` trait + `AdapterRegistry` + `SessionBatch`. Adding a harness = one adapter, zero downstream changes; unknown record → `Event::SystemNotice`.
- `ingest/claude_code.rs`, `ingest/codex.rs` — the two adapters (detect, backfill, byte-offset tail). Consumed by brain, commands, scheduler, radar, and `bin/warden_cli.rs` — so `ingest` is top-level, not a radar submodule.

**Analysis / domain**
- `featurizer.rs` — `FeatureVector` + `CompetenceProfile` (pure). `detectors.rs` — `nominate(store,profile) -> Vec<Finding>` (deterministic; also the source of truth for Living-Habits).
- `brain.rs` — LLM client (GLM-5.2 via NEAR AI, OpenAI-compatible): `run_pipeline`, `diagnose/coach/verify`; streaming→blocking→curl transport fallback; emits `fugu_delta`/`fugu_usage`.
- `forge.rs` — M4 fix-preview (read-only diff) + apply/revert of guardrail blocks in `~/.claude/CLAUDE.md`.
- `habits.rs` — Living-Habits streak/dial resolution (pure, time-gated).

**RADAR — `radar.rs` (40-line façade) + `radar/`** (live agent-forest; split from a former 4,120-line module)
- `model.rs` shared types (leaf) · `assemble.rs` deterministic forest join (+ integration suite) · `live.rs` live FS orchestration · `agent.rs` per-agent build + activity · `context.rs` token/estimation/breakdown/cost · `identity.rs` IDs/naming/dedup · `status.rs` working/idle/terminated · `composition.rs`/`hierarchy.rs`/`liveness.rs` (pre-existing).
- Façade re-exports a *narrow* public API (`assemble`, `recompute_radar_state`, `refresh_live_context`, the `Radar*` types); cross-submodule internals are `pub(crate)`.

**Scheduler — `scheduler.rs` (29-line façade) + `scheduler/`** (separates WHEN-runs from WHAT-runs; split from a former 2,043-line module)
- `watch.rs` live-ingest file watcher (watermarks, `ingest_file_once`, FSEvents `spawn_watchers`) · `radar.rs` recompute task (`RadarStateCache` + `RadarDirtySignal`; the cache's invalidation invariant is documented on the type) · `habits.rs` heartbeat task.
- Named `watch` (not `ingest`) to avoid colliding with the top-level `crate::ingest` adapter module.

**App shell (Tauri)**
- `lib.rs` — builder/`setup()`: tray, pre-warmed hidden `overlay` window, ⌘⌥⌃M global hotkey, startup backfill, watcher wiring (OS-specific bits via `platform::`). `commands.rs` — `#[tauri::command]`s (`query_profile`, `get_diagnosis`, `run_diagnosis`, `ask`, `get_fix_preview`, `resolve_evidence`, `set_config`, …; M5+ stubs return `not_in_slice`). `main.rs` → `lib::run()`. `bin/warden_cli.rs` — headless backfill CLI.

**Platform seam — `platform/`** (macOS today; prepared for ports). The single place OS-specific runtime code lives.
- `mod.rs` is the **port** (stable interface: `apply_activation_policy`, `is_reopen_event`, `primary_hotkey`, `process_alive`); `macos.rs` is the macOS **adapter** (the only place macOS-only Tauri APIs are used); `fallback.rs` is the no-op adapter for other targets. No other module branches on the OS.
- **Adding a platform** = implement one adapter file + a `#[cfg]` arm in `mod.rs` + a `tauri.conf.json` bundle target (Windows also needs an `OpenProcess`-based `process_alive`). See the `platform/mod.rs` doc.

**Conventions:** `anyhow::Result` + `?` internally, `.expect("invariant")` for true invariants — production `.unwrap()` is **denied** by clippy (`Cargo.toml [lints.clippy] unwrap_used = "deny"`; tests exempt via `clippy.toml`). Module form is `name.rs` + `name/` (façade re-exports a narrow surface, not a glob).

---

## Frontend — `web/viz/` (FSD-lite; imports point DOWN only)

Vanilla `web/main.ts` routes Tauri events into the React/R3F island mounted once at `#war-room-root`.
(The frontend folder is `web/`; the Rust backend lives in `src-tauri/`.)
Layers (a file may import only from layers below; `dev/` exempt; enforced by `pnpm check:arch`):

```
app/        → mount.tsx (mounts the island; Tauri IPC subscription)
views/      → war-room/ : WarRoom + the domain-aware chrome (chrome · NavBar · FilterBar · Sidebar)
modules/    → habits/ · radar/ · diagnosis/ · cinematics/   (no module imports a sibling module)
shared/     → state/ (bridge reducer) · types/ (orbTypes, radarTypes) · theme/ · scene/ · lib/
```
- **`shared/state/bridge.ts`** — the pure reducer: Tauri events → immutable `SceneState`. The honest seam every visual derives from. Imports only `shared/types`.
- **`modules/cinematics/`** — Remotion compositions + `PlayerHost`, lazy-loaded via `React.lazy` in `WarRoom` (its own Vite chunk). No app-wide barrel imports it eagerly.
- **Cross-layer contracts (`orbTypes`/`radarTypes`) live in `shared/types`** because `bridge` (shared) imports them — they cannot live in a module without breaking the import rule.
- Imports use the `@/` alias (`@/* → web/*`; tsconfig + vite). Tests are colocated. Components PascalCase, logic camelCase, folders kebab-case.

---

## Guardrails (run before review)
- `cd src-tauri && cargo clippy` — denies production `unwrap()`.
- `pnpm check:arch` — enforces the frontend import-direction rule.
- `cargo test` (Rust) · `pnpm test` (vitest) · `pnpm build` (tsc + vite).
- Toolchain pinned in `src-tauri/rust-toolchain.toml` (stable ≥ 1.85 for edition2024 deps).

> Note: the codebase is not currently `cargo fmt`-formatted (deliberate compact style in places); adopting default rustfmt would be a separate, sweeping reformat — not done here.
