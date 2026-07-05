# WARDEN

**The agent that watches your agents.**

You run Claude Code, Codex, and a growing pile of terminal agents every day. You don't see how they actually behave — where they loop, where they burn tokens re-reading the same files, where they teleport blind, where they quietly stall. WARDEN does.

WARDEN is a macOS desktop daemon that sits over your local AI-coding agents, reads their transcripts as they're written, and tells you — with evidence — where your agentic workflow is leaking. Summon it with a global hotkey and a cinematic war-room overlay shows the live state of your fleet, the recurring holes in how you drive it, and a ranked diagnosis you can act on.

It reads. It never writes to your projects.

---

## What it does

WARDEN turns raw agent transcripts into a diagnosis in five stages:

1. **Watch** — File adapters tail the transcript logs your agents already produce: Claude Code (`~/.claude/projects/**/*.jsonl`) and Codex (`~/.codex/sessions/**/rollout-*.jsonl`). Startup backfills history; `FSEvents` streams new writes live using byte-offset watermarks, so nothing is double-counted and no session is dropped on schema drift.
2. **Normalize** — Every harness, whatever its log format, maps into one canonical Rust intermediate representation. Sessions, events, and token usage land in a local SQLite + FTS5 store. Adding a new agent is one adapter file with zero downstream changes.
3. **Detect** — Deterministic detectors scan the normalized history and nominate candidate problems: the patterns that keep costing you time and tokens.
4. **Diagnose** — A Diagnostician → Coach → Verifier reasoning pipeline (GLM-5.2 via NEAR AI, OpenAI-compatible) takes each candidate, confirms or rejects it, and produces a coached, evidence-cited finding with a do/stop recommendation. No API key, or a failed call, degrades gracefully to detector-only output rather than faking it.
5. **Show** — A global hotkey (**⌘⌥⌃M**) summons a transparent, pre-warmed overlay: a live R3F war-room, a RADAR view of your active agent forest, and a forensic diagnosis panel — ranked holes, a severity meter, a frequency/confidence/cost ledger, evidence you can drill into, and a read-only fix preview. Press again, blur, or `Esc` to dismiss; the daemon stays alive and click-through in the background.

Every session and finding is tagged with the harness that produced it — Claude in emerald, Codex in violet — and color is always paired with a glyph and a label, so the identity survives on a color-blind display or a screenshot.

---

## How it's built

WARDEN is a single Tauri v2 application: a Rust core and a web overlay, talking over Tauri IPC.

- **Rust core** (`src-tauri/`) — one crate, layered `ingest → store → (featurizer · detectors · brain · forge · habits · radar) → commands · scheduler · lib`. Tauri itself is confined to `lib.rs` and `commands.rs`; everything below is plain, testable Rust. OS-specific code lives behind a `platform/` seam (macOS today), so a future port is one adapter file. Production `unwrap()` is denied by Clippy.
- **Web overlay** (`web/`) — a feature-sliced island in TypeScript. Imports point one direction only (`app → views → modules → shared`), enforced in CI by `pnpm check:arch`. React Three Fiber (three.js) drives the war-room; Remotion renders the intro and reveal cinematics; a vanilla-TS router wires real Tauri events into the scene.
- **Honest visualization** — every node, pulse, and flare in the war-room maps to a *real* signal: candidate counts, token deltas, verifier verdicts. When the engine doesn't emit a signal, the view degrades to something plain and true. It never invents motion to look busy.

Data flows one way: web calls Rust through `invoke`; Rust emits events to the web through `app.emit`.

```
Claude / Codex transcripts
        │  file adapters (backfill + live FSEvents)
        ▼
   canonical IR  ──►  SQLite + FTS5
        │
        ├─► detectors ──► candidate findings
        │                      │
        │                      ▼
        │            Diagnostician → Coach → Verifier  (GLM-5.2 / NEAR AI)
        │                      │
        ▼                      ▼
     RADAR  ◄───────────  evidence-cited diagnosis
        │
        ▼
  Tauri overlay: war-room · RADAR · diagnosis panel   (⌘⌥⌃M)
```

---

## Getting started

Requires macOS (Apple Silicon), a recent Rust toolchain (stable ≥ 1.85, pinned in `src-tauri/rust-toolchain.toml`), Node, and `pnpm`.

```bash
pnpm install          # install web dependencies
pnpm tauri dev        # run the full app
```

Copy `.env.example` to `.env` and add your engine key to get live diagnosis (see [Configuration](#configuration)). Without one, WARDEN still ingests, detects, and renders — it just skips the LLM diagnosis step.

### Dev previews without Tauri

The war-room and its pieces run standalone in the browser for fast visual iteration:

```bash
pnpm dev
# then open one of:
#   http://127.0.0.1:1420/dev-viz.html      full war-room QA loop
#   http://127.0.0.1:1420/dev-warroom.html  war-room in isolation
#   http://127.0.0.1:1420/radar-lab.html    RADAR sandbox
#   http://127.0.0.1:1420/orbs.html         habit-orb lab
```

### Build & verify

```bash
pnpm build                      # tsc + vite build (frontend typecheck + bundle)
pnpm test                       # vitest
pnpm check:arch                 # frontend import-boundary check
(cd src-tauri && cargo check)   # fast Rust typecheck
(cd src-tauri && cargo clippy)  # lint (denies production unwrap)
(cd src-tauri && cargo test)    # Rust unit + golden tests
pnpm tauri build                # full macOS app bundle (the real e2e gate)
```

`pnpm tauri build` produces `src-tauri/target/release/bundle/macos/WARDEN.app`.

---

## Configuration

Engine and storage are configured through the environment (see `.env.example`). Never commit real secrets.

| Variable | Purpose |
|---|---|
| `WARDEN_DB_PATH` | Override the SQLite database path. |
| `WARDEN_BRAIN_BASE_URL` | Diagnosis engine endpoint (OpenAI-compatible). |
| `WARDEN_BRAIN_API_KEY` | Engine API key (`OPENAI_*` used as fallback). |
| `WARDEN_BRAIN_DIAGNOSE_MODEL` | Diagnostician/coach model (default `z-ai/glm-5.2`). |
| `WARDEN_BRAIN_VERIFY_MODEL` | Verifier model. |
| `WARDEN_BRAIN_EFFORT` | Reasoning effort, where the engine supports it. |

---

## Project layout

```text
src-tauri/            Rust core (single crate)
  src/ir.rs           canonical intermediate representation
  src/ingest/         Adapter trait + registry + claude_code.rs / codex.rs
  src/store.rs        rusqlite + FTS5 persistence (byte-offset watermarks)
  src/featurizer.rs   feature vectors / operator profile
  src/detectors.rs    deterministic finding nomination
  src/brain.rs        GLM-5.2 diagnose → coach → verify pipeline
  src/radar.rs        live agent-forest model
  src/forge.rs        fix preview (apply is future work)
  src/habits.rs       living-habits streaks
  src/scheduler.rs    watch / radar / habits task drivers
  src/platform/       OS seam (macOS adapter)
  src/commands.rs     Tauri IPC commands
  src/lib.rs          daemon, tray, hotkey, window setup
web/                  TypeScript overlay (feature-sliced)
  index.html          overlay DOM shell
  main.ts             Tauri event router
  viz/                war-room, RADAR, diagnosis, cinematics, shared theme/state
docs/                 specs, plans, architecture notes, idea captures
scripts/              build / arch-check tooling
```

Deeper references live in [`ARCHITECTURE.md`](ARCHITECTURE.md) (full codemap + import rules), [`SPEC.md`](SPEC.md) (product spec), and [`CLAUDE.md`](CLAUDE.md) (working guide).

---

## Status

WARDEN is under active development. The spine, brain, overlay, and live RADAR are built and verified; the fleet is watched, diagnosed, and rendered end to end.

| Stage | State |
|---|---|
| Ingest + canonical IR + store | Done |
| Detectors + GLM-5.2 diagnosis pipeline | Done |
| Overlay: war-room, diagnosis, evidence, fix preview | Done |
| RADAR: live agent-forest view | Done |
| Forge (write fixes back to your project) | Next |
| Live view, voice, more adapters | Planned |

One rule holds across every stage: **WARDEN is read-only.** It previews fixes as diffs; it never writes to your projects. Writing is a future, explicitly-gated milestone — not a thing that can happen by accident today.

---

## License

Private. © Karim. All rights reserved.
