# RADAR Presence Polish — Design

- **Date:** 2026-06-24
- **Status:** Approved (design); spec under review
- **Milestone:** RADAR V1 continuation (pre-M4 polish)
- **Author:** design agent + Karim

## Context

RADAR (live-presence) is built and rendering, but four user-facing rough edges remain
before we move on to M4 (Forge/apply). This spec covers a focused polish pass across the
Rust radar backend and the R3F war-room frontend. It continues the in-flight "RADAR V1"
working-tree changes (uncommitted on `dev`) rather than starting fresh.

The current working tree differs from CLAUDE.md / memory in ways confirmed by discovery:
`ActivationPolicy` is `Regular` (not `Accessory`), blur **pauses** animation (does not hide),
click-through is removed, and root agents are named by their first task prompt (not the folder).
This spec treats the **code** as ground truth.

## Goals

1. **Cleaner naming** — root agents named by their project folder; subagents named
   `subagent N` and visibly tethered to the parent that spawned them.
2. **Reliable subagent lifecycle** — subagents link to their parent live, and when a
   subagent finishes it is detected as **terminated** and imploded away exactly once —
   never left hanging at "idle," never resurrected.
3. **Accurate working/idle detection** — "working" means generating right now; idle is
   alive-but-quiet; sharpen thresholds against real local data.
4. **Persistent, non-intrusive window** — opens maximized as a normal window (not
   always-on-top, not a fullscreen Space); keeps animating when unfocused / on another
   screen; pauses only when minimized; habits globes never collapse on click-out.
5. **Dramatic glow contrast** — working / filter-matching globes blaze with bloom; idle /
   filtered-out globes fall to dim embers. Strictly the existing phosphor + harness palette.

## Non-goals

- No M4 apply work. No M5–M7. No new harness adapters.
- No new color hues. Contrast comes from illumination vs dullness only.
- No always-on-top / floating-over-apps behavior (explicitly rejected by Karim).
- We will not depend on or copy any external reference project.

---

## Workstream 1 — Naming

**Current:** `identity()` ([radar/mod.rs:685](../../../src-tauri/src/radar/mod.rs)) derives a
root label from `first_task()` (first cleaned user prompt). The project-folder basename is
already carried separately as `RadarAgent.cwd`. Claude subagents use `meta["description"]`.

**Change:**
- **Root (depth 0):** label = `cwd` basename (e.g. `WARDEN`). Falls back to the existing
  task/nickname chain only when `cwd` is absent.
- **Same-folder collision:** when two live roots share a folder name, append a stable
  ordinal suffix — `WARDEN`, `WARDEN ②` — ordered by `started_at` so the labels are
  deterministic across recomputes.
- **Subagent (depth ≥ 1):** label = `subagent N`, where N is a **per-parent ordinal by
  spawn order** (1, 2, 3 under each parent). The real role/task/description is preserved
  and surfaced in `RadarDetailPanel` and the hover tooltip, not as the headline.
- Same rules for Claude and Codex.

**Acceptance:**
- A live Claude/Codex root rooted at `~/WARDEN` shows as `WARDEN`.
- Two roots in the same folder show as `WARDEN` and `WARDEN ②`, stable across refreshes.
- A parent with three subagents shows `subagent 1/2/3`; detail panel still shows each
  subagent's task/role.

---

## Workstream 2 — Subagent → parent linking (fix the fragility)

**Current:** `parent_id`, `depth`, `child_count` are computed and emitted in `radar_state`;
the frontend draws parent→child links (`RadarLinks`,
[RadarConstellation.tsx:366](../../../src/viz/RadarConstellation.tsx)). Linking is resolved
in `radar/hierarchy.rs` and persisted via `store.link_child_session`
([store.rs:117](../../../src-tauri/src/store.rs)). Discovery's hypothesis for the reported
breakage: the Claude path relies on `Event::SubagentSpawn.child_session` captured at
**parse time**, so a subagent spawned *live* (after the parent's initial ingest) never
re-links.

**Approach — diagnose first, then fix (systematic-debugging):**
1. **Reproduce against real transcripts.** Inspect actual `~/.claude/projects/**` records
   for the markers that identify a subagent and its parent (`isSidechain`, `parentUuid`,
   `userType`, the `Task`/`Agent` tool-call `call_id`, and the `subagents/agent-<id>.meta.json`
   sidecar `tool_use_id`). Confirm whether `child_session` is populated for a live spawn and
   whether `tool_use_id` ↔ `call_id` actually match on this machine.
2. **Fix the confirmed cause.** Most likely: re-derive the Claude link on **every recompute**
   from the parent's `Task` tool-call `call_id` ↔ subagent meta `tool_use_id`
   (`hierarchy::link_claude_subagents`), instead of trusting the parse-time `child_session`.
   Persist via `link_child_session`; downstream emission already works.
3. Keep the Codex path (`parent_thread_id` → parent `external_id`) — verify it still resolves.

**Acceptance:**
- Spawning a subagent in a live Claude session makes it appear **tethered to its parent**
  within ~2s, no app restart.
- Backend golden/unit test: a fixture with a parent `Task` call + subagent meta resolves to
  the correct `(child, parent)` pair via the live re-derivation path.

---

## Workstream 3 — Subagent termination & correct implode

**This is the highest-risk item; Karim emphasized it twice. A finished subagent must implode
once and stay gone.**

**Current:** `AgentStatus` is `{Working, Idle, Closed}` ([radar/liveness.rs](../../../src-tauri/src/radar/liveness.rs)).
A Claude subagent has no PID of its own, so liveness can't see it finish — its transcript
goes quiet and it sits at **Idle indefinitely** until the parent dies and the whole subtree
is dropped. There is no per-subagent termination signal today.

**Approach:**
- **Primary signal (permanent fact):** the parent's transcript records a `Task`/`Agent`
  tool **call** with a `call_id`; when the subagent finishes, the parent logs a matching
  **tool result** for that `call_id`. Presence of that result ⇒ subagent **Terminated**.
  Because the result is permanent in the transcript, every recompute re-derives "terminated"
  **identically** → idempotent, no resurrection.
- **Backstop (timeout):** if a subagent is quiet while its parent is still alive and no
  matching result arrives within `radar_subagent_terminate_ms` (config, generous default),
  mark it Terminated. To keep the backstop sticky, **persist `Session.ended_at`** (column
  already exists) when we first decide terminated, and treat a non-null `ended_at` as
  authoritative thereafter.
- **New status `Terminated`** distinct from `Idle` (alive, quiet) and `Closed` (parent/process
  gone). Wire value `"terminated"`. **Emit/drop choreography (explicit):** on the recompute
  that first detects termination, emit the agent once with `status:"terminated"` so the frontend
  has a clear transition to animate; exclude it from all subsequent recomputes. The frontend
  implodes on the `terminated` status (and also implodes any node that simply disappears, e.g.
  a closed parent's subtree), then graveyards the id. We do not rely on absence-detection alone,
  which is racy when payloads coalesce.
- **Frontend implode correctness (`radarLifecycle.ts`):**
  - Play the implode tween exactly once, then drop the node.
  - Maintain a per-session **graveyard set** of terminated/closed ids; reconciliation must
    never re-spawn an id in the graveyard, so a stray/duplicate payload cannot resurrect a
    globe that already imploded.
  - When a parent terminates/closes, its still-present children implode too (subtree
    collapse), each animating out rather than vanishing instantly.

**Approaches considered:** pure idle-timeout (rejected — a long tool call looks dead);
result-matching only (chosen as primary); result-matching + timeout backstop + persisted
`ended_at` (adopted, for robustness).

**Acceptance:**
- A subagent that completes its Task implodes within a couple seconds and does **not**
  reappear on subsequent recomputes (verified by a backend test asserting a session with a
  matching tool-result is classified `Terminated` and excluded, and re-classified identically
  on a second pass).
- Frontend test/manual: an imploded subagent is not re-added when the next `radar_state`
  payload arrives.

---

## Workstream 4 — Liveness fine-tuning (working vs idle accuracy)

**Current:** Claude prefers registry `status` (`busy`→Working) with transcript-mtime
fallback; dead PID→Closed. Codex uses file location + mtime
([radar/liveness.rs](../../../src-tauri/src/radar/liveness.rs)).

**Change:**
- Keep registry-status-first for Claude; tighten the mtime "working" window
  (`radar_working_ms`) so Working means *generating now*, and ensure Idle is never confused
  with Terminated (workstream 3 owns "finished").
- **Verify against the live machine**: probe live globe count and each agent's status; sharpen
  thresholds from observed data rather than guessing. Use the existing radar probe / a small
  read-only diagnostic; do not write to user projects.

**Acceptance:** on a machine with known-running and known-idle agents, statuses match reality;
no false "working" on a quiet session, no false "idle" on an actively generating one.

---

## Workstream 5 — Window behavior

**Current:** overlay is 960×640 centered, `alwaysOnTop:false`, `visible:false`
([tauri.conf.json:14](../../../src-tauri/tauri.conf.json)); summon shows + focuses
([lib.rs:37](../../../src-tauri/src/lib.rs)); `onFocusChanged`→`warden_dismiss`
([main.ts:131](../../../src/main.ts)) **pauses** animation on blur via
`activeFor`/`frameloopFor` (`WarRoom.tsx`); no minimize handler exists.

**Change:**
- **Open maximized** filling the screen as a normal window — `set_maximized(true)` on first
  summon (or config). Not `set_fullscreen` (that creates a separate Space and switches away
  on click-out). Keep `alwaysOnTop:false` — no floating over other apps.
- **Draggable like a native macOS window:** with `decorations:false` there is no native
  titlebar, so add a `data-tauri-drag-region` strip (the top chrome / header bar) to move the
  window — including from screen to screen. Interactive controls (nav tabs, legend chips,
  window buttons) must sit *above* the drag region (their own handlers / `pointer-events`) so
  they aren't swallowed by it. Dragging while maximized un-maximizes to a normal movable window
  (standard macOS "zoom" behavior); `resizable` stays `true`.
- **Remove blur→pause**: delete the `onFocusChanged → warden_dismiss` path; decouple animation
  from focus so moving to another screen/app keeps orbs flying and agents appearing live.
- **Pause only on minimize**: gate the frameloop on minimize / `document.hidden`, not `blurred`.
  Update `activeFor` to ignore the `blurred` input. If `document.hidden` proves unreliable for
  minimize under Tauri, add a Rust `WindowEvent` minimize→`window_minimized` event consumed by
  the bridge.
- **Habits globes never collapse on click-out**: this is the same root cause (dismiss-on-blur
  shrinking the forest); after the fix, verify habits globes stay full-size and animating with
  no Radar→Habits toggle needed.
- Live ingest / `radar_state` subscription already run regardless of focus — confirm unchanged.

**Acceptance:**
- Launch → window fills the screen, normal window, does not float over other apps.
- Click to another app/screen → orbs keep moving, new agents still pop in; habits globes stay
  put.
- Minimize → animation stops; restore → it resumes.
- Grab the header strip → drag the overlay to another monitor; nav tabs and legend filters
  still click (not swallowed by the drag region).

---

## Workstream 6 — Glow contrast (habits filter + radar working/idle)

**Current (too weak):** habits dull nodes only reach 42% brightness
(`dimScale` floor `0.42`, [Orb.tsx:36](../../../src/viz/Orb.tsx)); radar idle loses only 0.28
of glow with no color crush ([RadarConstellation.tsx](../../../src/viz/RadarConstellation.tsx));
bloom threshold `~0.27` ([WarRoom.tsx:306](../../../src/viz/WarRoom.tsx)).

**Change (approved direction — see the rendered mockup):**
- **Habits filter:** matching nodes keep full color + bloom, with a small emissive lift;
  non-matching get crushed — color floor `0.42 → ~0.18`, lower opacity, and pushed *under* the
  bloom threshold so they read as near-dark embers. `targetDim` semantics unchanged; only the
  crush depth and the lit-side lift change.
- **Radar:** Working = full heat + bloom + quick shimmer; Idle = color-crushed + dimmed + slow
  breath (still faintly alive — **not** crushed as hard as a filtered-out habit, per the
  decided default); Terminated = implode (workstream 3).
- Strictly the existing palette — emerald `#3dffa0`, violet `#b98cff`, amber `#ff5a37`,
  green `#76ff9d`, bg `#020403`. No new hues.
- Tune values together (crush floor, opacity, bloom threshold/intensity, emissive lift) so the
  contrast is obvious at a glance without looking childish.

**Acceptance:** with a filter active, matching globes are unmistakably brighter than the rest;
on radar, a working agent visibly blazes next to idle ones. Verified live in the running app
(screenshots before/after).

---

## Cross-cutting: testing & verification

- **Backend (Rust):** TDD for naming (folder + ordinal + `subagent N`), live subagent
  re-linking, and the termination classifier (incl. idempotency / no-resurrection and the
  timeout backstop). `cd src-tauri && cargo test`.
- **Frontend:** typecheck + bundle (`pnpm build`); verify visual behaviors (window
  keep-animating, habits no-collapse, glow contrast, implode-once) in the running app with
  screenshots. No writes to user projects at any point (M2/M3 are preview/observe only).
- **Diagnose-before-fix** for workstreams 2 and 4 (systematic-debugging): reproduce against
  real local transcripts before changing code.

## Primary files to touch

- Backend: `radar/mod.rs` (naming, assemble), `radar/hierarchy.rs` (live re-link),
  `radar/liveness.rs` (status incl. `Terminated`), `ingest/claude_code.rs`,
  `ingest/codex.rs`, `store.rs` (`ended_at`), `scheduler.rs`, `config.rs`, `commands.rs`,
  `lib.rs`, `tauri.conf.json`.
- Frontend: `WarRoom.tsx` (`activeFor`/`frameloopFor`, bloom), `main.ts` (blur/minimize),
  `Orb.tsx` (`dimScale` crush + emissive lift), `RadarConstellation.tsx` (working/idle glow,
  links), `radarLifecycle.ts` (implode + graveyard), `RadarDetailPanel.tsx` (subagent task/role),
  `emphasis.ts` (only if filter shape changes).
