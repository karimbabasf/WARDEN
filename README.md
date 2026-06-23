# WARDEN

WARDEN is the agent that watches your agents: a macOS Apple Silicon-first Tauri v2 daemon that ingests local AI-coding transcripts, normalizes them into a canonical Rust IR, diagnoses recurring agentic workflow leaks through Sakana Fugu, and renders a cinematic green/black overlay with evidence-cited findings.

The current repo state is a verified M3-ready slate. M0 Spine, M1 Brain, and M2 Face are complete; M3 RADAR is next and has not been implemented yet.

## Product thesis

The value is not a UI wrapped around an LLM. WARDEN’s value is the chain:

1. EYES — local transcript adapters watch Claude Code and Codex.
2. MEMORY — normalized sessions/events/findings live in SQLite + FTS5.
3. SPINE — every harness maps into one canonical IR.
4. BRAIN — deterministic detectors nominate candidate holes; Fugu diagnoses/coaches/verifies them.
5. FACE — the overlay makes the diagnosis legible through a war-room visualization, ranked evidence, and read-only fix previews.
6. RADAR — next milestone: locate and navigate the live agent fleet visibly, without blind teleportation.

## Current milestone state

- M0 Spine: complete.
- M1 Brain: complete.
- M2 Face: complete and verified.
- M3 RADAR: next, not started.
- M4 Forge apply, M5 Live, M6 Voice, M7 Adapters: future and stubbed through `scaffold::not_in_slice()`.

## What is in M2

- Tauri v2 macOS accessory app with tray menu.
- Hidden pre-warmed transparent overlay window.
- ⌘⇧Space global hotkey to summon; Esc/blur dismisses.
- Click-through idle state so the desktop is not blocked while WARDEN is hidden.
- Startup backfill and live FSEvents tailing for:
  - Claude: `~/.claude/projects/**/*.jsonl`
  - Codex: `~/.codex/sessions/**/rollout-*.jsonl` plus archived sessions
- SQLite/FTS5 store, byte-offset watermarks, source raw hashes, findings, diagnoses, artifacts.
- Env-swappable Brain defaults to Fugu; missing key or failed API can degrade to detector-only diagnosis.
- Honest R3F war-room island driven by real Tauri events:
  - `candidates_nominated`
  - `finding_verdict`
  - `fugu_delta`
  - `fugu_usage`
  - `diagnosis_ready`
- Lazy Remotion intro/reveal overlays through `@remotion/player`.
- Diagnosis UI with ranked holes, severity meter, frequency/confidence/cost ledger, do/stop guidance, evidence drill-down, and read-only fix preview.
- Harness identity everywhere a finding/session is shown: Claude emerald ◆, Codex violet ▲, unknown neutral; color is always paired with glyph + label.

## Repository map

```text
SPEC.md                                      master product spec
CLAUDE.md                                    current repo guide for agents
PROGRESS.md                                  milestone ledger and verification log
package.json                                 pnpm scripts + web dependencies
vite.config.ts                               Vite/Vitest config
index.html                                   overlay DOM shell
src/main.ts                                  web-side Tauri event router + terminal UI
src/diagnosis.ts                             pure DOM diagnosis renderer
src/style.css                                FACE design system tokens/styles
src/viz/WarRoom.tsx                          R3F war-room island
src/viz/bridge.ts                            pure event reducer: Tauri events -> scene state
src/viz/PlayerHost.tsx                       lazy Remotion player boundary
src/viz/compositions/                        Remotion intro/reveal/recap compositions
src/viz/dev.tsx + dev-viz.html               standalone visual QA loop
src-tauri/src/ir.rs                          canonical IR
src-tauri/src/ingest/                        Claude/Codex adapters + registry
src-tauri/src/store.rs                       SQLite/FTS5 persistence
src-tauri/src/featurizer.rs                  feature vector / profile computation
src-tauri/src/detectors.rs                   deterministic finding nomination
src-tauri/src/brain.rs                       Fugu-compatible diagnosis pipeline
src-tauri/src/commands.rs                    Tauri IPC commands
src-tauri/src/lib.rs                         daemon/tray/hotkey/window setup
src-tauri/src/scaffold.rs                    future-milestone stub helper
```

## Commands

Install dependencies:

```bash
pnpm install
```

Verify the slate:

```bash
pnpm build
pnpm test
cd src-tauri && cargo check
cd src-tauri && cargo test
cd src-tauri && cargo build
pnpm tauri build
```

Run the app in development:

```bash
pnpm tauri dev
```

Run the standalone war-room visual QA loop without Tauri:

```bash
pnpm dev
# open http://127.0.0.1:1420/dev-viz.html
```

## Environment

- `WARDEN_DB_PATH` — override SQLite database path.
- `SAKANA_API_KEY` — Fugu/OpenAI-compatible API key.
- `WARDEN_BRAIN_BASE_URL` — override Brain endpoint.
- `WARDEN_BRAIN_API_KEY` — override Brain API key.
- `WARDEN_BRAIN_DIAGNOSE_MODEL` — override diagnose/coach model.
- `WARDEN_BRAIN_VERIFY_MODEL` — override verifier model.
- `WARDEN_BRAIN_EFFORT` — reasoning effort where supported.

Do not commit real secrets. Use placeholder values only.

## Verification evidence

The M3-ready slate was freshly verified on 2026-06-23 from `/Users/karimbaba/WARDEN`:

- `pnpm build` passed.
- `pnpm test` passed: 6 files, 38 tests.
- `cd src-tauri && cargo check` passed.
- `cd src-tauri && cargo build` passed.
- `cd src-tauri && cargo test` passed: 69 Rust tests.
- `pnpm tauri build` passed and produced `src-tauri/target/release/bundle/macos/WARDEN.app`.
- Browser smoke of `dev-viz.html` mounted the R3F canvas and Remotion player and reached the reveal phase.

## M3 handoff

Start M3 from `SPEC.md` §9A, not by guessing. RADAR’s governing principle is: never teleport blind; locate visibly, then navigate, and degrade honestly when precision is unavailable.

Before implementing M3:

1. Write/approve a focused M3 RADAR spec.
2. Write an implementation plan.
3. Preserve M2’s safety boundary: no Forge apply, no voice, no screen Q&A, no extra adapters unless the M3 spec explicitly requires them.
4. Keep every RADAR confidence/position signal honest and inspectable.
