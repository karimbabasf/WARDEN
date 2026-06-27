# REFACTOR.md — WARDEN architecture refactor (decisions to approve)

> **Status:** PROPOSED — nothing has been changed in the codebase. This document is the
> decision surface. Check the `- [ ]` boxes (or strike a decision and write your call next to it),
> answer the **Open Questions** at the bottom, then I execute phase by phase.
>
> **Governing principle (your constraint):** *clear architecture, clean naming, no over-engineering.*
> This plan **removes more ceremony than it adds**. The only two things it *adds* are (1) one
> import-direction lint on the frontend and (2) façade discipline on two Rust god-modules. Everything
> else is moving files into folders, deleting dead weight, and fixing what's already broken.

---

## 0. What's actually true right now (verified, not summarized)

I re-checked all of this against the filesystem and git, because the first-pass review and CLAUDE.md
were both partly wrong.

**Good news — the design is sound (this is NOT "no architecture"):**
- Frontend import graph is a clean DAG. Pure-logic files (`bridge.ts`, `orbLayout.ts`, `radarLayout.ts`,
  `cameraFraming.ts`, `emphasis.ts`) import **zero** React/three — verified by grep. The DOM `chrome.tsx`
  imports **zero** three.js. Domains don't cross-import (only shared `theme` + `emphasis`). No cycles.
- Backend has a real extension point that's worth keeping: the `Adapter` trait + `AdapterRegistry`.
- Pure cores (`featurizer`, `detectors`, `habits`, `window`, `forge`-preview) are Tauri-free and tested.

**Bad news — the repo is in a broken, mid-surgery state (this is the part that's fair to call "trash" right now):**

| # | Problem | Evidence |
|---|---------|----------|
| **B1** | **Build can't even resolve deps.** `getrandom 0.4.3` requires Cargo feature `edition2024` (Cargo ≥1.85); this machine is on `cargo 1.84.1`. `cargo check` fails before compiling. | Ran `cargo check` → `feature 'edition2024' is required ... not stabilized in 1.84.1`. |
| **B2** | **`ingest` module is half-moved and won't compile.** Working tree **deleted** `src/ingest/{mod,claude_code,codex}.rs` and added **untracked** `src/radar/ingest/`. But `lib.rs:9` still declares top-level `pub mod ingest;`, there is **no `#[path]`**, and `radar/mod.rs` never declares `pub mod ingest`. The module is dangling. | `git status`: `D src/ingest/*` + `?? src/radar/ingest/`; `grep` for `#[path` → none. |
| **B3** | **CLAUDE.md is stale.** It documents `ingest/` as top-level and describes a 14-table store etc.; the tree has moved on. Docs can't be trusted as a map. | `git ls-files` vs working tree mismatch. |
| **B4** | **8 files over 1,000 lines** (god-modules). | `wc -l`: `radar/mod.rs` 4120, `scheduler.rs` 2043, `store.rs` 1687, `commands.rs` 1616, `radar/ingest/claude_code.rs` 1533, `brain.rs` 1333, `radar/ingest/codex.rs` 1275, `forge.rs` 1198. |
| **B5** | **~680 `unwrap()` in non-test code** — top: `radar/mod.rs` 100, `forge.rs` 98, `scheduler.rs` 91, `store.rs` 88, `commands.rs` 84. | grep counts. |
| **B6** | **Frontend is ~60 files in ONE flat folder** (`src/viz/`), source + tests interleaved, three domains piled together. The wiring is clean; the *layout* is a junk drawer. | `ls src/viz`. |

**Honest framing:** the *engineering* (boundaries, separation of pure logic) is decent. The *repo hygiene*
right now is bad — broken build, uncommitted half-refactor, stale docs, oversized files. Both are true at once.
The file **count** on the frontend is mostly justified (each pure-logic module + its test is a legit split);
the **organization** is the problem. So the frontend job is ~95% *move files into folders*, not delete them.

---

## 1. Core decisions

### PHASE 0 — Stabilize to green *(must happen before any reorg)*

- [x] **D0.1 — Fix the toolchain blocker.** ✅ DONE (`0b808b6`) — pinned `rust-toolchain.toml` to stable (resolved **1.96.0**). Add a pinned `rust-toolchain.toml` (channel = a current stable ≥ 1.85)
      so `edition2024` transitive deps resolve, and so the build is reproducible. *(Alternative: `cargo update -p getrandom --precise 0.2.x/0.3.x` to stay under the old MSRV — but edition2024 deps will keep arriving, so pinning the toolchain forward is the durable fix.)* **Recommend: pin toolchain forward.**
- [x] **D0.2 — Finish the `ingest` move by reverting it to top-level.** ✅ DONE (`0b808b6`) — restored `src/ingest/` from HEAD; orphan `radar/ingest/` parked out of tree. `ingest` is consumed by `lib.rs`,
      `commands.rs`, `scheduler.rs`, `bin/warden_cli.rs`, **and** `radar/*` — it is a **foundational** layer, not a
      radar submodule. Restore `src/ingest/` at the crate root (move the untracked `radar/ingest/` files back,
      re-add to git) so `pub mod ingest;` resolves. **Reject** nesting ingest under `radar/`. Get `cargo build` +
      `cargo test` green and `pnpm build` green.
- [x] **D0.3 — Commit the green baseline** ✅ DONE (`0b808b6`) — cargo check ok · cargo test 268/0 · pnpm build ok. before touching structure. One commit: "stabilize: green build + ingest
      restored to top-level". No refactoring rides along with it. *(No push / no PR — local commit only.)*

### PHASE 1 — Frontend: FSD-lite (your proposal, adapted)

> **✅ DONE — green on `refactor/architecture`.** `pnpm build` ok (lazy `PlayerHost` chunk preserved) · `pnpm test` 27 files / 254 passed · 70 files moved as git renames (history preserved) · `@/*` path alias added (tsconfig + vite).
> **Two refinements vs this draft, forced by the real import graph:** (1) shared contracts `orbTypes`/`radarTypes` live in `shared/types/` — not in a module, because `bridge` (shared) imports them; (2) `chrome`/`NavBar`/`FilterBar`/`Sidebar` live in `views/war-room/` — not `shared/ui`, because they're domain-aware.
> **Simplifications:** modules flattened (no `ui/`/`model/` segments); **no per-module `index.ts` barrels** (Vite perf + protects the lazy Remotion chunk).
> **Two pre-existing couplings the reorg exposed were fixed:** `AgentCore` promoted to `shared/scene/`; `frameloopFor` extracted from `WarRoom` to `shared/scene/frameloop.ts`. Final import-direction audit: fully clean.

- [x] **D1.1 — Adopt the 4-layer FSD-lite you proposed**, applied **inside `src/viz/`**: `app / views / modules / shared`.
      This is a recognized, *lighter* variant (same model as bulletproof-react) — it drops FSD's heaviest part
      (the `entities`/`features`/`widgets` 3-way split), which is exactly the over-engineering you want gone.
- [x] **D1.2 — Reinterpret `views/` as "screens", not routes.** This app has no router. `views/` = the
      composition roots the tab/state switch mounts (`WarRoom`, the diagnosis screen) — thin wiring, no logic.
- [x] **D1.3 — Module split by domain:** `modules/{habits, radar, diagnosis, cinematics}`. Each is self-contained
      (`ui/` = R3F/DOM components, `model/` = pure node-tested logic) with a **thin `index.ts` public API**.
      `shared/{scene, state, theme, ui, lib}` holds cross-cutting primitives (the pure `bridge` reducer lives in
      `shared/state`; only the impure Tauri subscription lives in `app/`).
- [x] **D1.4 — Enforce exactly ONE rule:** dependencies point **down only** — `app → views → modules → shared`,
      and **modules never import sibling modules**. Enforced by a single ESLint `import/no-restricted-paths` rule.
      That rule *is* the 80% of FSD's value; everything else is dropped.
      *(PARTIAL: the rule is followed by construction and grep-verified clean after Phase 1. The ESLint
      tooling itself is **deferred** — adding a linter to a repo that currently has none is a separate
      toolchain decision; do it as a small follow-up if you want it machine-enforced.)*
- [x] **D1.5 — Conventions (lowest-churn, fully conventional):** PascalCase components, camelCase logic/hooks,
      kebab-case folders, `UPPER_SNAKE` consts, PascalCase types. **Colocate tests inside their module** (`.test.ts`
      next to source). **No app-wide barrel file** (Vite perf + it would break the lazy Remotion chunk — keep
      `compositions/` imported only at the dynamic-import site).

### PHASE 2 — Backend: split god-modules, keep one crate

> **✅ DONE (D2.1–D2.2) — green on `refactor/architecture`** (commits `0ed4570` radar, `fa2846a` scheduler). Both split with **zero behavior change**; `cargo test` **268 passed / 0 failed** throughout; every external caller byte-identical.
> **Radar** (4,120 lines) → 40-line façade + 9 submodules: `model · assemble · agent · context · identity · live · status` (+ kept `composition/hierarchy/liveness`).
> **Scheduler** (2,043) → 29-line façade + `watch · radar · habits` (named `watch`, not `ingest`, to avoid colliding with `crate::ingest`).
> `AppState`→`app_state.rs` extraction **deferred** (low value). **D2.3** (clippy lint + ~680-`unwrap` burn-down) and the lock-encapsulation part of **D2.4** move to **Phase 3**; the `RadarStateCache` invalidation invariant was documented now.

- [x] **D2.1 — Stay single-crate.** Do **not** split into a `warden-core` workspace crate now. Keep domain modules
      `tauri`-free by convention. *(The compile-time argument barely applies for a 2-crate graph, and `#[tauri::command]`
      doesn't move cleanly into a child crate.)* Revisit only when a second real binary needs the core. *(Note: `bin/warden_cli.rs`
      technically already consumes core — see Open Q3.)*
- [x] **D2.2 — Adopt modern `name.rs` + `name/` module form** (drop `mod.rs` style) and split the two worst god-modules
      into **façade + submodules** (a slim parent that orchestrates + re-exports a *narrow* surface, not a glob forwarder):
  - `radar/mod.rs` (4120) → `radar.rs` façade + `radar/{model, assemble, agent, composition, hierarchy, liveness, identity}.rs`
    *(composition/hierarchy/liveness already exist — this mostly carves up the 4120-line `mod.rs`).*
  - `scheduler.rs` (2043) → `scheduler.rs` façade + `scheduler/{ingest, radar, habits}.rs`, separating **WHEN** things run
    (scheduler = thin task drivers) from **WHAT** runs (radar/habits domain logic).
  - *(Lower priority:* `forge.rs`, `store.rs`, `commands.rs`, `brain.rs` are large but cohesive — split only if a clean seam exists; `commands.rs` → move `AppState` out to `app_state.rs`.)*
- [x] **D2.3 — Error handling: `anyhow` everywhere, no `thiserror`** (no failure site is matched on — adding error enums
      is ceremony). Burn down `unwrap()` incrementally under `[lints.clippy] unwrap_used = "warn"` + `clippy.toml`
      `allow-unwrap-in-tests = true`; convert `anyhow::Error → String` only at the `#[tauri::command]` edge. CI/local gate:
      `cargo clippy -- -D warnings`.
- [~] **D2.4 — Document state invariants.** Encapsulate `RadarStateCache`/locks behind methods; write down the cache
      invalidation policy (what dirties it, how stale a reader may be); **never hold a lock across `.await`** (especially
      the `reqwest` LLM call). `std::sync::Mutex` stays correct for the rusqlite connection.

### PHASE 3 — Conventions, docs, cleanup

> **✅ Largely DONE — green on `refactor/architecture`.** Key finding: the "~680 unwraps" was a **measurement error** — that count included test code, where `unwrap()` is idiomatic. With `allow-unwrap-in-tests`, clippy finds only **20 production unwraps**, all converted to `.expect("invariant")` (zero behavior change — each is a static regex / freshly-built object / home-dir / JSON we serialized ourselves). `[lints.clippy] unwrap_used = "deny"` now hard-fails `cargo clippy` on any new production unwrap; `cargo test` still 268/0.
> **D1.4** is enforced by a no-dependency `scripts/check-arch.mjs` (`pnpm check:arch`) instead of a full ESLint toolchain — passes clean.
> **D3.1**: both "cut" candidates were FALSE ALARMS — `windowChrome.test.ts` is a real tauri-config/source regression test, and `harnessTheme`/`radarTheme` are distinct (not a dup) → correctly **no deletions**.
> **Remaining: D3.2** (rewrite stale CLAUDE.md + add ARCHITECTURE.md) and the lock-audit half of D2.4.

- [x] **D3.1 — Targeted cuts (no mass deletion):** delete the orphan `windowChrome.test.ts` (verify it tests nothing live
      first); **do NOT** merge `harnessTheme.ts`/`radarTheme.ts` unless confirmed an exact dup (likely an intentional
      radar-specific palette). Resist merging small *pure+tested* files (e.g. `cameraFraming.ts`) — that separation is the good kind.
- [x] **D3.2 — Rewrite CLAUDE.md to match reality** + add a short `ARCHITECTURE.md` codemap (symbols, not prose) so the
      map stays greppable. ✅ DONE — refreshed CLAUDE.md repo-map/commands/conventions; added `ARCHITECTURE.md`.
      **rustfmt deliberately NOT adopted:** `cargo fmt` would reformat ~107 sites across the codebase (intentional
      compact style in places) — standardizing it is a separate, owner-approved sweep, not slipped into this refactor.

### PHASE 5 — Platform seam (cross-platform readiness)

> **✅ DONE — green on `refactor/architecture`.** Requested forward-prep: isolate OS-specific code so a future
> Linux/Windows port is "implement one adapter", not "untangle `#[cfg]`s scattered across the tree". Added
> `src-tauri/src/platform/`: `mod.rs` (the **port**: `apply_activation_policy`, `is_reopen_event`, `primary_hotkey`,
> `process_alive`), `macos.rs` (the macOS **adapter** — the only place macOS-only Tauri APIs are used),
> `fallback.rs` (no-op adapter for other targets). Routed the three scattered `#[cfg(target_os = "macos")]` sites in
> `lib.rs` + the `libc::kill` liveness syscall through the seam; **`lib.rs` now has zero `#[cfg(target_os)]`**.
> `cargo build`/`test` green (268/0), zero new clippy warnings.
> **Adding a platform** = implement one adapter + a `#[cfg]` arm in `mod.rs` + a `tauri.conf.json` bundle target
> (Windows also needs an `OpenProcess`-based `process_alive`). The macOS bundle config (`targets: "app"`,
> `macOSPrivateApi`, the `macos-private-api` feature) is left as-is — per-platform build config, documented in
> `ARCHITECTURE.md`, to wire up when a target is actually brought up.

---

## 2. Target trees

### Frontend (`src/viz/`)
```
src/viz/
  app/            # runs once on the prewarmed window; may import anything below
    mount.tsx · bridge-host.ts (impure Tauri IPC subscription) · PlayerHost.tsx
  views/          # screens the tab/state switch mounts (no router)
    WarRoom.tsx · DiagnosisView.tsx
  modules/
    habits/    ui/{Orb,Constellation,StarCatalog}.tsx  model/{orbLayout,orbTypes,useOrbCamera}.ts  *.test.ts  index.ts
    radar/     ui/{RadarConstellation,RadarDetailPanel,RadarHoverCard,AgentCore}.tsx
               model/{radarLayout,radarLifecycle,radarTheme,radarTypes,rosterTree}.ts  *.test.ts  index.ts
    diagnosis/ diagnosis.ts (+ .test.ts)  index.ts          # moved from src/diagnosis.ts
    cinematics/ {Intro,Reveal,Recap,IntroVideo}.tsx  model/{timing,palette}.ts   # lazy; NOT via app barrel
  shared/
    scene/ {CameraRig,Transition}.tsx {cameraFraming,emphasis}.ts   # shared R3F primitives + pure math
    state/ bridge.ts                                                # the pure reducer
    theme/ {harnessTheme,harnessColors}.ts
    ui/    {NavBar,Sidebar,FilterBar,chrome}.tsx
    lib/   recorder.ts
  dev/            # exempt from the import rule
    dev.tsx · devWarRoom.tsx · preview/*
```
**Rule:** `app → views → modules → shared`, no sibling-module imports; `dev/` exempt. One ESLint `import/no-restricted-paths`.

### Backend (`src-tauri/src/`)
```
src-tauri/src/
  main.rs · lib.rs* · commands.rs* · app_state.rs*     # (*) the only tauri-aware files
  ir.rs · store.rs · featurizer.rs · detectors.rs · brain.rs · redaction.rs · scaffold.rs · config.rs · util.rs · window.rs · harness_theme.rs
  ingest/            # TOP-LEVEL again (foundational): mod.rs (Adapter+Registry) · claude_code.rs · codex.rs
  radar.rs           # façade
  radar/             model.rs · assemble.rs · agent.rs · composition.rs · hierarchy.rs · liveness.rs · identity.rs
  habits.rs · forge.rs
  scheduler.rs       # façade: start()/supervision/on-ask trigger
  scheduler/         ingest.rs (watchers+watermark) · radar.rs (recompute task) · habits.rs (heartbeat task)
  bin/warden_cli.rs
```

---

## 3. Execution plan (how, with subagents)

Each phase: **git-mv to preserve history → build + tests green → one commit → no behavior change.** No push, no PR.

| Phase | Work | Parallelizable? |
|------|------|-----------------|
| 0 | Stabilize: toolchain pin + restore `ingest/` top-level + green baseline commit | sequential (gate) |
| 1 | Frontend FSD-lite move + ESLint rule + colocate tests | 1 agent (FE is one coherent move) |
| 2 | Backend god-module splits — `radar/` and `scheduler/` are **independent** → 2 agents in parallel, each on its own worktree | yes (2 agents) |
| 3 | clippy `unwrap_used=warn` + incremental unwrap burn-down + cuts | 1 agent, incremental |
| 4 | Rewrite CLAUDE.md + add ARCHITECTURE.md | 1 agent |

Backend Phase 2 splits are pure mechanical moves (cut a 4120-line file into 7, fix `use` paths) — ideal for
worktree-isolated subagents that must leave `cargo test` green before returning.

---

## 4. What we are deliberately NOT doing (anti-over-engineering)

- ❌ No `entities/features/widgets` split — one `modules/` layer.
- ❌ No empty `ui/model/lib/config` segment folders — add a segment only when files fill it.
- ❌ No app-wide barrel; no routing Remotion through an eager `index.ts`.
- ❌ No architecture-linting framework — one `import/no-restricted-paths` rule.
- ❌ No `warden-core` workspace split (yet).
- ❌ No `thiserror`, no hexagonal/repository ceremony over rusqlite, no single-impl traits, no generics-where-concrete-works.
- ❌ No file line-count cap; no `clippy::pedantic`-everything noise.
- ❌ No mass kebab rename; no merging pure+tested small files; no speculative M5–M7 folders.

---

## 5. Open questions for you (please answer)

1. **Toolchain (D0.1):** pin `rust-toolchain.toml` forward to a current stable, or pin `getrandom` back to stay on 1.84.1? *(I recommend pinning forward.)*
2. **ingest placement (D0.2):** confirm `ingest` returns to **top-level** (my strong recommendation), vs. keeping it under `radar/` as the half-move intended?
3. **Workspace split (D2.1):** `bin/warden_cli.rs` already imports core modules — that's technically the "second binary" trigger for a `warden-core` crate. Stay single-crate anyway (recommended, it's tiny), or treat the CLI as the reason to split now?
4. **Scope/sequencing:** do all phases, or stop after Phase 0 (just get it building) and reassess?
5. **Branch:** new branch `refactor/architecture` off `dev` for all of this? (I will not push.)
```
