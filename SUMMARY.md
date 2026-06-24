# WARDEN — M0-M2 Stabilization Summary

Date: 2026-06-23
Branch: `m2-face`
Target: clean, verified slate for M3 RADAR

## Intent understood

WARDEN is the agent that watches your agents. It is a macOS Apple Silicon-first Tauri v2 daemon with a Rust core and web overlay. It watches local AI-coding harness transcripts, normalizes them into one IR, learns repeated workflow holes, and explains them through a cinematic but evidence-grounded FACE.

The immediate job was not to start M3. It was to make M0-M2 trustworthy enough that M3 can begin without dragging instability forward.

## What exists now

### M0 — Spine

The spine is present: canonical IR, Claude ingestion, SQLite/FTS5 store, feature/profile computation, and source/raw references.

Key files:

- `src-tauri/src/ir.rs`
- `src-tauri/src/ingest/claude_code.rs`
- `src-tauri/src/store.rs`
- `src-tauri/src/featurizer.rs`

### M1 — Brain

The diagnosis pipeline is present: deterministic detectors nominate findings, Brain runs a Fugu-compatible Diagnostician/Coach/Verifier path, and detector-only degradation exists when API access is absent or the model path fails.

Key files:

- `src-tauri/src/detectors.rs`
- `src-tauri/src/brain.rs`
- `src-tauri/src/commands.rs`

### M2 — Face

M2 is implemented and verified:

- always-on macOS accessory app
- tray menu
- pre-warmed hidden overlay
- ⌘⇧Space summon
- Esc/blur dismissal
- startup backfill
- live FSEvents tailing
- Codex adapter
- env-swappable Brain
- R3F war-room island
- anime.js terminal/reveal transitions
- lazy Remotion intro/reveal player
- evidence-cited diagnosis readout
- read-only fix preview
- harness differentiation with glyph+label accessibility

Key files:

- `src-tauri/src/lib.rs`
- `src-tauri/src/scheduler.rs`
- `src-tauri/src/ingest/codex.rs`
- `src-tauri/src/config.rs`
- `src/main.ts`
- `src/diagnosis.ts`
- `src/style.css`
- `src/viz/WarRoom.tsx`
- `src/viz/bridge.ts`
- `src/viz/PlayerHost.tsx`
- `src/viz/compositions/*`

## FACE design direction

The UI is intentionally green/black and cinematic, but the spectacle is tied to function:

- War-room candidate nodes come from real `candidates_nominated` events.
- Confirmed/refuted flares come from real `finding_verdict` events.
- Stage glow and token pulses come from real `fugu_usage` and `fugu_delta` events.
- Off-Fugu engines degrade to plain token weight rather than faking orchestration.
- Diagnosis evidence renders stored quotes or resolves raw events read-only.
- Fix preview renders a diff but disables apply; applying is M4.
- Harness color is never the only signal; glyph + label accompany every harness badge.

The signature visual is the hot-white R3F war-room core with amber verdict flares and a Remotion slam-in reveal. The terminal readout remains the durable UX surface: readable evidence, severity, cost, and action guidance.

## Files changed in this cleanup pass

- `.gitignore` — ignored local `.claude/` worktree artifacts so the root status is clean and intentional.
- `CLAUDE.md` — corrected stale M2/M3 status and outdated repo map notes.
- `PROGRESS.md` — replaced stale pending-M2 ledger with a verified M3-ready ledger.
- `README.md` — added a clean project entry point and run/verify instructions.
- `SUMMARY.md` — this handoff summary.
- `docs/superpowers/specs/2026-06-23-m2-forensic-reconstruction-design.md` — forward-looking M2 forensic-reconstruction enhancement spec (not part of the verified M2 baseline; see caveats).

No production code path was changed in this cleanup pass. The codebase already contained the M2 implementation; the cleanup made the repo’s documentation and handoff match reality.

## Verification run

Fresh commands executed from `/Users/karimbaba/WARDEN`:

```text
pnpm build
  exit 0
  tsc && vite build
  137 modules transformed
  warning: main chunk >500 kB; build still passed

pnpm test
  exit 0
  6 test files passed
  38 tests passed

cd src-tauri && cargo check
  exit 0

cd src-tauri && cargo build
  exit 0

cd src-tauri && cargo test
  exit 0
  71 tests passed

pnpm tauri build
  exit 0
  built /Users/karimbaba/WARDEN/src-tauri/target/release/bundle/macos/WARDEN.app
```

Browser/visual smoke:

```text
http://127.0.0.1:1420/dev-viz.html
  R3F canvas mounted
  Remotion player mounted
  scripted mock loop reached reveal phase
```

Visual inspection confirmed the intended green/black cinematic direction and visible reveal/legend. The preview is intentionally sparse and non-interactive because it is a standalone visual loop, not the full overlay workflow.

## Remaining caveats

- The Vite build warns that the main chunk is large. Remotion is already split into the lazy `PlayerHost` chunk; the remaining weight is the R3F/Three/postprocessing path. This is acceptable for M2 but should be measured in M3 if summon latency regresses.
- The browser preview is not a substitute for a real Tauri hotkey/overlay interaction test on macOS. The full app bundle builds, and the wiring is present, but a human-visible hotkey latency measurement is still a useful M3 preflight.
- `docs/superpowers/specs/2026-06-23-m2-forensic-reconstruction-design.md` exists as a forward-looking enhancement spec. It is not required for the M2 baseline unless Karim chooses to adopt it.

## M3 starting line

Begin M3 from `SPEC.md` §9A: RADAR. The principle is visible location before navigation. The implementation should bind logical sessions to real OS processes/windows/panes with confidence, show the user where an agent is, and only then warp or degrade honestly.

Do not implement M4 apply, M5 live interjection, M6 voice, or M7 adapters as part of M3 unless a new approved spec expands scope.
