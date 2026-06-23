# WARDEN M3 — RADAR: Live Agent Constellation (Design Spec)

**Status:** Design approved (brainstorming). Next step: implementation plan via `writing-plans`.
**Date:** 2026-06-23 · **Branch:** `m3-radar` (off `m2-face`) · **Milestone:** M3.
**Supersedes for M3:** this is the canonical M3 spec. A separate exploratory doc
`2026-06-23-m3-near-ai-engine-swap.md` exists in the tree on a different topic and is **not** part of M3 RADAR.

---

## 1. Summary

RADAR is a **second live 3D constellation** in WARDEN's existing hotkey-summoned war-room overlay.
A top navigation bar — **Habits | Radar** — switches between the constellation that exists today
(anti-pattern "habits") and this new one.

RADAR renders **every AI coding agent currently open on the machine** as a forest of glowing globes:
main agents are planets, their subagents are orbiting moons (depth-N), connected by glowing links.
Each globe's **size** is how full that agent's context window is right now; its **color** is which tool
it is (Claude orange / Codex violet), **heating up** toward white-hot as the window fills. Globes bloom
in when an agent opens, tween continuously as context changes, and **collapse into themselves** when the
agent closes. Hovering shows a quick-glance card; clicking dives the camera in and opens a rich detail
panel showing the agent's context composition, live activity, child agents, and cost.

Everything maps to a **real, locally-recoverable signal** — confirmed by forensic research against this
machine's `~/.claude` and `~/.codex` data and the Claude Code binary (§4). No fabricated data.

## 2. Goals & Non-Goals

**Goals**
- A live, honest, cinematic constellation of currently-open Claude & Codex agents and their subagents.
- Globe size = live context-window occupancy; color = harness + fill heat; links = parent→child.
- Deterministic agent/subagent hierarchy reconstruction from local files, real-time.
- Hover preview + click-to-focus detail panel (context gauge + composition, live activity, children, identity+cost).
- All motion damped/tweened — appearances, growth, heat, and removal. No snapping, ever.
- Reuse the existing orb engine (layout, camera, previews, theme) behind a two-constellation shell + nav.

**Non-Goals (YAGNI / scope guards)**
- **Preview-only.** No writes to user projects (apply = M4). RADAR only reads local transcripts/registries.
- No exact `/context` itemization that requires network `countTokens` calls — composition stays **purely local** (§4.5, decision locked).
- M4 Forge · M5 Live · M6 Voice · M7 Adapters remain **stubbed** via `scaffold::not_in_slice()`. Do not implement.
- No new heavy persistence schema — the live forest is ephemeral, recomputed from files on each event (§5).
- No process control (we observe agents; we never start/stop/kill them).

## 3. Definitions

- **Agent** — a top-level (root) AI coding session: one Claude Code session, or one Codex orchestrator session.
- **Subagent** — a child agent spawned by a parent (Claude `Agent`/Task dispatch; Codex Desktop "explorer").
- **Forest** — the set of agent trees currently open on the machine; RADAR's data model.
- **Context occupancy** — tokens currently in an agent's context window (NOT lifetime tokens burned). Deflates on compaction.
- **Status** — `working` (actively generating), `idle` (open but quiet), `closed` (gone → imploded away).

## 4. Data sources & extraction (confirmed on this machine)

All signals below were verified by research against live local data and the Claude Code binary. This section
is the contract: it is what makes RADAR non-vague.

### 4.1 Claude — liveness (open / working / idle / closed)
| State | Signal |
|---|---|
| Open (live or idle) | A file `~/.claude/sessions/<PID>.json` exists `{pid, sessionId, cwd, startedAt, version, kind, entrypoint}` |
| Alive (not crashed) | `kill(pid, 0)` succeeds |
| Working vs idle | transcript `<sessionId>.jsonl` mtime `< ~5s` ⇒ working; older ⇒ idle |
| Closed | no `sessions/*.json` references the sessionId, **or** the file exists but `kill(pid,0)` fails (zombie) |

Watch `~/.claude/sessions/` via FSEvents: file create ⇒ globe blooms; file delete / dead PID ⇒ globe implodes.
This lets RADAR react to a **small** directory instead of scanning every transcript. (Optional future: a
`SessionStart`/`SessionStop` hook push model.)

### 4.2 Claude — hierarchy (parent → subagent, depth-N)
- Each subagent writes its own transcript `~/.claude/projects/<proj>/<session>/subagents/agent-<id>.jsonl`
  plus a sidecar `agent-<id>.meta.json` `{agentType, description, toolUseId}`.
- Link is **deterministic**: `meta.toolUseId` == the parent assistant record's `tool_use` (name `Agent`/Task) `id`.
- Subagent records carry `isSidechain: true` and `agentId: <id>`. Sub-subagents nest recursively under
  `…/subagents/agent-<id>/subagents/…` — same linkage applies at every depth.
- **Labels:** subagent = `agentType` + `description` (e.g. "Explore · Map orb frontend"); main = `cwd` basename + last prompt.
- **Backend gap to close (#1 build item):** today's Claude adapter only scans project-root `*.jsonl`; it does
  **not** descend into `subagents/`. `Event::SubagentSpawn` exists in the IR but `child_session` is always `None`.

### 4.3 Codex — liveness & "done"
- A live session is a rollout file under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
- On finish, Codex **moves the file to `~/.codex/archived_sessions/`** — that move **is** the done/implode signal.
  The watcher must treat a `sessions/` delete + matching `archived_sessions/` arrival (same UUID) as "agent ended",
  not "data lost" (don't drop the watermark).

### 4.4 Codex — hierarchy (Codex Desktop only)
- Codex **Desktop** (the native app) spawns parallel subagents. Each writes a **normal-looking** rollout file;
  the hierarchy lives in `session_meta.payload`:
  | Field | Meaning |
  |---|---|
  | `thread_source` | `"subagent"` (child) vs `"user"` (root) |
  | `parent_thread_id` | UUID of the parent rollout (children only) |
  | `agent_role` | e.g. `"explorer"` |
  | `agent_nickname` | display name (scientist names: Hilbert, Dirac, Nash, Erdős…) |
  | `multi_agent_version` | `"v1"` on both parent and children (multi-agent-capable session) |
- Link = post-ingest pass grouping `parent_thread_id → session_meta.id`.
- **Honest caveat:** subagents appear only for `originator: "Codex Desktop"`. VS Code Codex (`codex_vscode`)
  sessions have no children and render as **flat solo globes** — true to the data, never faked.

### 4.5 Context size & composition
- **Size (exact, from transcript):**
  - Claude: `usage.input_tokens + cache_creation_input_tokens + cache_read_input_tokens` (last assistant turn).
  - Codex: `payload.info.last_token_usage.input_tokens`.
  - Max window: Claude via model-id lookup (`message.model`); Codex via `task_started.model_context_window`.
  - Fill % = size / max. Globe deflates whenever occupancy drops — e.g. a context compaction (Codex emits
    `context_compacted`; for Claude it surfaces directly as a fall in the size metric on the next turn).
- **Composition — purely local, honest buckets (decision locked):**
  - **Exact (API-anchored, in the transcript):** total; the cache-stable (`cache_read`) vs freshly-written
    (`cache_creation`) vs fresh-input (`input`) vs `output` split; per-turn delta.
  - **Estimated (local tokenization, calibrated to the exact totals):**
    - *Preamble* (system + tools + memory, one block) ≈ `turn1_total − tokenize(first_user_message)`.
    - *Conversation* (messages), *Tool outputs / file reads* (tokenize large `tool_result` blocks), *Thinking*.
    - Tokenizer: `tiktoken` (cl100k/o200k) as an approximation, **ratio-calibrated per session** against the
      transcript's exact per-turn totals. All estimated buckets are **labeled "est."** in the UI.
  - **Explicitly out of scope (would require leaving the machine):** exact System-vs-Tools-vs-MCP-vs-Skills
    itemization — `/context` gets those via live `countTokens` API calls; no offline Claude tokenizer exists and
    the assembled system prompt is never written to disk. We show the preamble as one honest block, not itemized.

## 5. Backend architecture

A new **radar collector** (e.g. `src-tauri/src/radar.rs`, coordinated by the existing `scheduler.rs`) owns the
live forest. It is event-driven off FSEvents and the existing byte-watermark tailing spine.

**Watchers**
- `~/.claude/sessions/` — liveness registry (create/delete) + `kill(pid,0)` for crash detection.
- `~/.claude/projects/**/subagents/` — **new** Claude subagent ingest path (transcripts + `meta.json`).
- `~/.codex/sessions` & `~/.codex/archived_sessions` — already watched; add `parent_thread_id` linkage + archive-move handling.

**Per-event pipeline**
1. Ingest new bytes (existing adapters, extended per §4.2 / §4.4).
2. Recompute affected agents' snapshots: identity, harness, model, context size, fill %, composition buckets,
   status, children, recent activity (last N events), estimated cost.
3. Update the in-memory forest (keyed by session id) and emit a **`radar_state`** event (Rust→web) carrying the
   forest + per-agent snapshots; send **deltas** on change so the FACE can tween smoothly.

**IR / store changes (minimal)**
- Claude adapter: `detect()` descends into `subagents/`; read `meta.json`; populate `SubagentSpawn.child_session`.
- Codex adapter: post-ingest linkage `parent_thread_id → id`; archive-move handling (ended, watermark preserved).
- IR: carry parent linkage for Codex (e.g. `RunScope.parent_session_id`).
- No new persistence required for the forest — it is computed from files + watermarks on demand. (We may add a
  tiny `total_token_usage` / `model_context_window` read for accuracy; both already in the files.)

**Honest-viz / fallbacks**
- Unknown harness ⇒ neutral globe + glyph, never fake hierarchy.
- VS Code Codex ⇒ flat solo globe (no children exist).
- Off-Fugu engines lack orchestration tokens ⇒ degrade to plain weight, per existing conventions.

## 6. Frontend architecture

The orb engine is already generic. RADAR is a **second constellation behind a nav**, reusing rendering, camera,
hover/click previews, and theme.

**Reused unchanged:** `orbTypes`, `orbLayout` (math), `useOrbCamera`, `harnessTheme` (extended), `preview/`.
**New / parametrized:**
- Top **nav bar** (Habits | Radar) in the overlay; switching swaps the active constellation via state with a
  smooth cross-fade (one overlay, two scenes).
- `bridge.ts` + `WarRoom.tsx` parametrized by `constellation: 'habits' | 'radar'`; a `radar` reducer consumes
  `radar_state`.
- `RadarOrb` type: `{ id, harness, role, nickname?, parentId?, model, contextTokens, maxTokens, fillPct,
  status, children[], estCost, composition }`.
- `radarLayout` — depth-N hierarchy: planet → orbiting moons → sub-moons, with parent→child links. Reuses the
  existing sizing/orbit math, extended past depth-1.
- Radar **palette**: Claude orange, Codex violet (Radar-only; separate from Habits' colors). Always paired with a glyph (a11y).

## 7. Visual law

- **Size** = `rankBoost · √(contextTokens)` — context occupancy, with a hierarchy boost so main agents read as
  noticeably larger than their subagents.
- **Color** = harness hue **heated by fill**: empty = deep dim ember → filling = brighter → near-full = blazing
  white-hot core. Harness identity stays legible at every fill level.
- **Links** = glowing parent→child edges, depth-N, matching the Habits constellation's look.

## 8. Motion & lifecycle (smoothness is a hard requirement)

Every visual change is damped/tweened — **nothing snaps**:
- **Spawn** — a new subagent emerges from its parent and travels out along the link; a root blooms in.
- **Grow / shrink & heat** — size and color tween continuously as context changes (incl. visible deflation on compaction).
- **Implode** — on close/archive, the globe **collapses into itself** and winks out (signature removal animation).
- **Idle vs working** — working = brighter + faint active shimmer; idle = dimmed + slow breathing (at-a-glance "who's thinking").

## 9. Interaction

- **Hover** — screen-space card (constant pixel size at any zoom): label, harness glyph, model, fill %, # children, status.
- **Click** — camera dives to focus the globe (`useOrbCamera`), detail panel opens with four sections:
  1. **Context gauge + composition** — gauge (size / max, fill %, heat-matched); composition shown two ways:
     an **exact** lens (cache-stable / fresh / output) and a **semantic** lens (Preamble · Conversation ·
     Tool-outputs/files · Thinking, marked "est."); size sparkline where compaction dips show.
  2. **Live activity feed** — recent tool calls / messages / the tool in-flight, tailing live.
  3. **Children roster** — subagents (type/nickname, context %, status); click a row to fly to that globe.
  4. **Identity + cost** — label, harness, model, uptime, estimated $ from token usage.

## 10. Build order (within M3)

1. **Backend spine** — liveness watcher; Claude `subagents/` ingest + meta linkage; Codex `parent_thread_id`
   linkage + archive-move handling; per-agent snapshot + local composition; `radar_state` event.
2. **Constellation** — nav + two-constellation shell; depth-N `radarLayout`; size/heat/links; all tweened motion
   (spawn / grow / heat / implode).
3. **Detail panel + hover + camera focus** — the four-section panel, hover card, focus dive.
4. **Polish** — idle/working states, honest-viz fallbacks, performance for many globes, tests.

## 11. Testing strategy

- **Rust (golden/unit):** subagent `subagents/`+`meta.json` linkage → correct tree; Codex `parent_thread_id`
  linkage; liveness partition (mock `sessions/` + PIDs, `kill -0`); archive-move handling; exact context-size &
  cache/fresh math from `usage` fields; composition calibration math.
- **TS (unit):** `radarLayout` depth-N positions deterministic; radar bridge reducer over `radar_state`;
  lifecycle/tween state machine (spawn→grow→implode); heat color mapping vs fill; hover/detail data shaping.
- **Honest-viz:** estimated buckets are labeled; flat agents (VS Code Codex / unknown harness) never get fake children.

## 12. Risks & deferred

- **Tokenizer drift** for estimated buckets — mitigated by per-session calibration to exact API totals; estimates labeled.
- **Codex model id** not in transcript — label by provider + context window; exact model name deferred.
- **Sub-subagent depth** — render full depth; cap *visual* density only if needed.
- **Performance** with many globes — LOD / instancing if profiling demands.
- **Claude crash cleanup** — handled by `kill(pid,0)` (stale `sessions/*.json` treated as closed).
- **Liveness registry is version-dependent** (confirmed on Claude Code v2.1.181). If `~/.claude/sessions/` is
  absent on some version, fall back to transcript-mtime-only liveness (open = recent mtime; closed = stale beyond a threshold).
- **Exact `/context` itemization** — deferred (could become an optional, key-gated toggle later).

## 13. Decision log (locked)

| Decision | Choice |
|---|---|
| Scope | Open agents (working **and** idle); remove on close/archive |
| Update model | Event-driven (FSEvents) + smooth tween; **no snapping** |
| Globe size | Live **context-window occupancy** (not lifetime tokens) + hierarchy boost |
| Color | Harness hue **heating up** with fill (ember → white-hot) |
| Palette | Radar-only: Claude **orange**, Codex **violet** (separate from Habits) |
| Hierarchy | **Links**, depth-N, mirroring Habits |
| Liveness | `~/.claude/sessions/<pid>.json` + `kill -0`; Codex archive-move = done |
| Detail panel | All four: context gauge+composition · live activity · children · identity+cost |
| Composition | **Purely local** honest buckets (exact where API-anchored, calibrated estimates labeled) |
| Codex shape | Embrace real hierarchy (Codex Desktop trees); VS Code Codex = flat solo globes |
| Removal anim | **Collapse into self** + wink out |
| Constraint | Preview-only; M4–M7 stubbed; no project writes |
