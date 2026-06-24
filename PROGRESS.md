# WARDEN — Build Progress Ledger

> Source of truth for milestone status + verification. Keep this current so every future session can start from the repo, not from memory.

## Status

| M | Name | State | Spec / plan | Verification |
|---|------|-------|-------------|--------------|
| M0 | Spine | ✅ done | `SPEC.md` | Rust tests green; historical commit `d87497d` |
| M1 | Brain | ✅ done | `SPEC.md` | Rust tests green; historical commit `7ac9b10` |
| M2 | Face | ✅ verified / M3-ready | `docs/superpowers/specs/2026-06-22-m2-face-design.md` + `docs/superpowers/plans/2026-06-22-m2-face.md` | `pnpm build`, `pnpm test`, `cargo check`, `cargo build`, `cargo test`, `pnpm tauri build` all green on 2026-06-23 |
| M3 | RADAR | ⬅️ next | see `SPEC.md` §9A | not started |
| M4 | Forge apply | ⬜ future / stubbed | `SPEC.md` | `scaffold::not_in_slice()` |
| M5 | Live | ⬜ future / stubbed | `SPEC.md` | `scaffold::not_in_slice()` |
| M6 | Voice | ⬜ future / stubbed | `SPEC.md` | `scaffold::not_in_slice()` |
| M7 | Adapters | ⬜ future / stubbed | `SPEC.md` | `scaffold::not_in_slice()` |

## What M2 now provides

- macOS Tauri v2 accessory app with tray menu and hidden pre-warmed overlay.
- Guarded global hotkey: ⌘⇧Space summons the overlay; Esc/blur dismisses it and restores click-through.
- Startup transcript backfill plus live FSEvents tailing for Claude Code and Codex.
- Canonical IR/store path remains intact: adapters normalize to `ir.rs`, store persists SQLite/FTS5 state, featurizer/detectors nominate findings.
- Env-swappable Brain engine defaults to Fugu and emits honest pipeline events.
- R3F war-room island mounted once behind the terminal, driven only by real events: nominated candidates, Fugu deltas/usage, finding verdicts, diagnosis-ready.
- Remotion live intro/reveal overlays are lazy-loaded through `PlayerHost`, keeping Remotion off the summon hot path.
- Diagnosis readout renders ranked holes, severity/frequency/cost/confidence, do/stop guidance, evidence drill-down, read-only fix preview, and harness identity.
- Harness theme is a single source of truth in Rust and web: Claude emerald ◆, Codex violet ▲, unknown neutral, verdict amber.
- M3/M4/M5/M6/M7 commands remain explicit stubs; M2 does not implement RADAR, applying fixes, voice, screen Q&A, or extra adapters.

## Verification log

2026-06-23 — final M2/M3-ready slate verified from `/Users/karimbaba/WARDEN` on branch `m2-face`:

- `pnpm build` — exit 0. `tsc && vite build`; 137 modules transformed. Vite warning only: main chunk >500 kB; Remotion remains split into lazy `PlayerHost` chunk.
- `pnpm test` — exit 0. 6 test files, 38 tests passed.
- `cd src-tauri && cargo check` — exit 0.
- `cd src-tauri && cargo build` — exit 0.
- `cd src-tauri && cargo test` — exit 0. 69 Rust tests passed.
- `pnpm tauri build` — exit 0. Built `/Users/karimbaba/WARDEN/src-tauri/target/release/bundle/macos/WARDEN.app`.
- Browser smoke at `http://127.0.0.1:1420/dev-viz.html` — canvas and Remotion player mounted; scripted mock loop reached reveal phase with real-shaped candidate/verdict/diagnosis events. Visual inspection found the green/black direction coherent and cinematic, with the caveat that the preview is intentionally non-interactive and sparse outside the overlay/diagnosis surfaces.

## Known caveats / next decisions

- The production bundle has a Vite chunk-size warning for the main Three/R3F chunk. It is not a failing build, but M3 should consider manual chunks or route-level splitting if startup weight becomes measurable.
- The standalone `/dev-viz.html` preview is a visual QA loop, not the actual terminal overlay. It intentionally uses mock events with real event shapes.
- `docs/superpowers/specs/2026-06-23-m2-forensic-reconstruction-design.md` is a forward-looking forensic reconstruction spec layered on top of M2. It is not part of the verified M2 baseline unless Karim chooses to adopt it before/alongside M3.
- Do not start M3 until its RADAR spec/plan is written and approved.
