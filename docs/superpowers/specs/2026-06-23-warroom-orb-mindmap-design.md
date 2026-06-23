# WARDEN War-Room — Orb Mind-Map Redesign (V1)

- **Date:** 2026-06-23
- **Status:** Design approved (brainstorm), ready for implementation plan
- **Milestone context:** Refines the M2 FACE R3F war-room during M3. The R3F island
  (`src/viz/`) already exists and is being actively reworked by a parallel agent — **this
  spec is the source of truth that work should align to.**
- **Supersedes:** the current decorative scene where orb position (Fibonacci-by-index) and
  edges (k-nearest-neighbour) carry no meaning and nothing is clickable.

---

## 1. Problem & goal

The current war room renders orbs (findings + Fugu pipeline stages) on a Fibonacci sphere
with k-NN edges and no interaction. The orbs are honest about *what they are* but lie about
*where they are* and *what connects them* — position and links are decorative.

**Goal:** make the war room a navigable **mind-map of the user's agentic habits** where every
orb and every link maps to a real computed signal, and the scene is explorable like a video
game (hover, click-to-zoom, drill-in). Pretty **and** useful — never fake.

This is bound by the existing **honest-viz law** (CLAUDE.md): nodes/links map to real signals;
off-Fugu engines degrade gracefully (delta pulses + plain weight), never fabricate.

---

## 2. Two constellations, one engine

- **C1 — "How you work" (THIS spec).** Agent archetypes with their anti-pattern issue orbs.
  A persistent diagnostic mind-map of the user's habits.
- **C2 — "Live agents" (PARKED — M5 Live).** A separate view of the orchestrator + the
  subagents it spawns, working in real time, click an orb to watch what each is doing. **Not
  built here.** C1 must be built so its primitives are reusable by C2: the orb component, the
  hover-preview, the click-to-zoom camera rig, and the bridge model shape.

---

## 3. C1 data model — what an orb is

Two node families, plus links.

### 3.1 Agent hubs (roots)
- One hub per **harness present in the data** (Claude `◆` emerald, Codex `▲` violet, or any
  future harness — **data-driven, not hardcoded to 2**).
- **Hub size = that agent's total problem-load** (so a tool you fight with has a visibly
  bigger/denser sun). Default metric: sum of its issue-orb counts (confirm in planning;
  alternative: severity-weighted load).
- A hub with zero issues is a clean, solo sun.

### 3.2 Issue orbs (satellites)
- One orb per **(agent × detected pattern)**, aggregated across all of that agent's sessions.
- **Size = persistence count** — how many times that issue occurred for that agent.
- **Color = severity** (ramp below).
- **Link = to its own agent hub only.** No shared orbs, no cross-agent links, no pattern↔pattern
  edges. The same habit on both agents is **two independent orbs** (e.g. Codex context-bloat
  ×15 is large+red; Claude context-bloat ×2 is small) — they never merge.
- **Orbs exist only for real, detected issues.** Clean = no orb. The set is **uncapped** and
  driven by how many habits are noticed; nothing hardcoded to the current 7 detectors — new
  detectors light up new orbs for free.

### 3.3 Scope
- **Aggregate / persistent profile** — every session ever, rolled up. The map is stable and
  evolves slowly; it is NOT rebuilt per diagnosis run.
- (Deferred idea: a live per-run pulse could later animate *over* this persistent map. Not V1.)

---

## 4. Encodings — the honest-viz contract

| Channel | Encodes | Backing signal |
|---|---|---|
| Which hub an orb links to (+ link color) | **Agent identity** (Claude vs Codex) | session harness |
| Orb fill color | **Severity** | `Finding.severity` (mapped to a 1–5 ramp) |
| Orb size (diameter) | **Persistence count** | occurrence count per (agent × pattern) |
| Hub size | Agent's total problem-load | sum of its issue counts |
| Position | Which agent owns it (structural) + calm force-settle | links only |

- **Severity ramp:** 1–2 phosphor `#76ff9d` → 3 `#ffd166` → 4 `#ff8a3d` → 5 `#ff5a37`.
- **Agent colors (single source of truth, `harnessTheme.ts`):** Claude `#3dffa0` `◆`,
  Codex `#b98cff` `▲`. Always pair color with glyph + label (color-blind a11y).
- **No second color job on an orb.** Agent is carried by the hub + link (and by the
  agent-colored sessions seen on drill-in), so the orb fill is free to mean only severity.
- **Degradation:** if a signal is missing (e.g. off-Fugu engine lacks orchestration tokens),
  the orb still renders from detector data; never invent counts/severity.

---

## 5. Layout

- **Two hub-and-spoke clusters in one shared 3D space**, so both agents compare at a glance.
- Each hub is a gravity anchor; its issue orbs orbit it. **Calm, damped force-settling** —
  a stable constellation, NOT the drifting float of the old scene.
- Faint per-agent "territory" halo behind each cluster reinforces the two-territory read.
- Heavier/denser/redder cluster = the tool you struggle with more. That comparison is the
  primary at-a-glance insight.
- Scales to many orbs via size encoding + the zoom interaction; add gentle grouping only if a
  single agent ever exceeds a legibility threshold (not needed at today's ≤7 patterns/agent).

---

## 6. Interaction model (V1)

### 6.1 Hover → preview
- A preview card **blooms out of the orb** (scales up from it) and lives in **screen space**,
  so it is a **constant size at any zoom level** (zooming scales the world, not the card).
- Content — issue: `name · agent · ×count · severity n/5 · one-line description`.
  Hub: `AGENT · N sessions · M issue types · worst habit`.

### 6.2 Click issue orb → dive + drill-in
- Camera smoothly **dives to center the orb**; other orbs dim back.
- A **detail panel blooms open** (centered focus panel growing from the orb as the camera
  dives in — chosen over welding to the orb's exact pixel, to avoid edge clipping).
- **Drill-in is real WARDEN data** (the orb IS the forensic readout):
  - severity meter (5 ticks)
  - rationale (`Finding.rationale`)
  - cost ledger: `est_cost_tokens · est_cost_minutes · frequency · confidence`
  - where: the session ids it occurred in (`Finding.evidence[].session_id`)
  - DO / STOP guidance (`Diagnosis.do_items` / `stop_items` relevant to the pattern)
  - read-only **fix-preview** button (`get_fix_preview`) — preview only, **apply is M4**.

### 6.3 Click hub → gentle zoom + agent summary
- Shallow zoom + dim others + an agent summary card (sessions, issue-type count, worst habit).

### 6.4 Camera
- Free zoom (scroll / control) scales the constellation; preview stays sized.
- Click empty space → fly back to the resting overview.

### 6.5 Deferred (post-V1, explicitly out of scope)
- **Hub-explode**: hub click pushes its issue orbs out into a clean readable ring.
- **Deep evidence drill**: clicking a session/evidence goes one level deeper into the actual
  quoted events / event timeline — reuse the existing `resolve_evidence` path and the
  `diagnosis.ts` evidence drill-down logic when built.

---

## 7. Data contract & backend wiring

The viz home view binds to the **aggregate profile + all stored findings**, NOT the per-run
candidate/verdict stream.

**Shape the viz needs** (per agent, a list of issues):
```
SceneModel {
  agents: [{ id, harness, label, glyph, color, sessions, eventCount, totalLoad }]
  issues: [{ agent, patternId, title, count, severity (1-5),
             estCostTokens, estCostMinutes, frequency, confidence, rationale,
             sessionIds: string[], evidence: EvidenceRef[] }]
}
```

**Sources (all real, non-stub commands):**
- `query_profile` → `{ session_count, event_count, finding_count, by_harness:[{harness,sessions,events}] }`
- `get_findings` → `Vec<Finding>` (pattern_id, title, severity, frequency, est_cost_tokens,
  est_cost_minutes, confidence, rationale, evidence[], verifier_verdict, status)
- `get_fix_preview(finding_id)` → read-only diff (on demand)
- `resolve_evidence(session_id, event_id)` → quote (deferred deep-drill)

**Planning item (must confirm against `store.rs` / `ir.rs`):** how to obtain the
**per-(agent × pattern) occurrence count** and the **agent attribution** of each finding.
Findings carry `session_ids` and severity/frequency/cost/evidence; agent comes from the
session's harness (or the `harnessByPattern` backstop main.ts already builds from verdicts).
Decide: derive the grouping on the frontend from `get_findings`, or add a small aggregate
command. Do not assume a `harness`/`count` field exists on `Finding` until verified.

**Replaces:** the per-run `candidates_nominated` → `finding_verdict` → `diagnosis_ready`
animation as the *home view*. That live path may be repurposed later for the deferred
"pulse over the map" idea; it is not the V1 home view.

---

## 8. Component / file plan (`src/viz/`)

- **`bridge.ts`** — add the aggregate `SceneModel` builder (group findings by agent × pattern,
  compute counts/severity/load) and expose it to the scene. Keep the existing per-run state
  separate (it serves the cinematic reveal, not the home map). Unit-tested.
- **`WarRoom.tsx`** — render hubs + satellites as R3F meshes (reuse the existing orb/cell
  visual language: phosphor cores, bloom). Positions from a layout module, not array index.
- **New units (each one clear purpose, independently testable):**
  - `orbLayout.ts` — pure: maps `SceneModel` → positions (anchored hubs + damped force/orbit).
  - `OrbPreview` — screen-space hover card (drei `Html` or a DOM overlay synced to the orb's
    projected position; **must not scale with world zoom**).
  - `OrbDetail` — the drill-in panel (consumes the issue/hub model; renders the real fields).
  - `useOrbCamera` — dive-to-orb / reset / free-zoom camera rig.
- **`harnessTheme.ts`** — reused as-is for agent color/glyph/label; add the severity ramp tokens
  if not already centralized.
- **Cleanup:** remove the decorative k-NN edges and Fibonacci index placement; remove the
  `FrameProbe` / `diag()` TEMP diagnostics flagged in the current file once the island is
  verified in the packaged overlay.

---

## 9. Non-goals (V1)

- **No real Obsidian vault integration.** "Mind map" is the UX/interaction metaphor (node =
  real entity, link = real relation, position carries meaning) — there is no sync to the
  user's Obsidian. *(Assumption — flag at review if wrong.)*
- **No fix apply** — fix preview is read-only (apply = M4).
- **C2 "Live agents"** — deferred to M5.
- **Hub-explode** and **deep evidence/event-timeline drill** — deferred.
- **No writes to user projects, ever** (project rule).

---

## 10. Testing

- **bridge / aggregation (vitest+jsdom, like `bridge.test.ts`):** grouping by agent × pattern,
  count = occurrences, severity mapping, hub load = Σ counts; empty profile → only hubs (or
  nothing); a pattern on both agents → two distinct issue entries.
- **Honest-viz invariants:** assert no orb without a backing finding; no link other than
  issue→its-agent; orb count == distinct detected (agent×pattern) issues.
- **Interaction:** preview is constant-size across zoom (screen-space invariant); camera reset
  returns to overview; click issue opens its own data (no cross-wiring).
- Keep `mount.test.ts` green (island mounts once into `#war-room-root`).

---

## 11. Open questions for spec review

1. **Obsidian** — metaphor only (assumed), or real vault sync desired later?
2. **Hub size metric** — Σ issue counts (assumed) vs severity-weighted load?
3. **Backend grain** — confirm whether `get_findings` is groupable to (agent × pattern) counts
   on the frontend, or a small aggregate command is warranted (resolve in writing-plans).
