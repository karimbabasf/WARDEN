# FACE Redesign — "Radar is the Hero"

**Date:** 2026-07-04
**Status:** Approved (design) — proceeding to plan
**Scope:** Frontend / presentation only. **Zero backend changes.** All Rust (`radar`, `dossier`, `habits`, `brain`, detectors) is strong, tested, and honest per four-agent discovery; the entire weakness is presentational and product-focus.

## 1. Goal

Turn WARDEN from an unfocused four-surface tech demo into a **tight, demoable product with one clear hero**. Success = a running app that reads as a real product: a beautiful, viewable live-fleet **RADAR** (the hero), a one-click evidence-cited **DIAGNOSIS**, and a polished retrospective **DOSSIER** (which absorbs Habits). No overlapping buttons, no terminal/chat costume, no dead code.

The product tells a clean three-beat story:

> **RADAR** — your agents, right now → **DIAGNOSIS** — what's wrong with how you drive them → **DOSSIER** — who you are as an operator, and what you're fixing.

## 2. Problems being fixed (from discovery)

| # | Problem | Root |
|---|---|---|
| 1 | No hero; four overlapping surfaces | Product focus |
| 2 | Habits and Radar share the *same* 3D constellation → indistinguishable | `WarRoom.tsx` renders both tabs through one scene |
| 3 | "Legacy chat" = the terminal-costumed `Console` ask-bar (`chrome.tsx`), live but ugly | Terminal visual metaphor |
| 4 | Button overlap, top-left | `#open-profile` bolted into `index.html` *outside* React's z-index system; every control hand-pixel-positioned |
| 5 | Radar "won't view nicely" | No camera auto-fit / composed angle / reframe (geometry already fixed → Fibonacci sphere, uncommitted) |
| 6 | Habits value invisible | Buried metric, decorative ring, no narrative, unfinished payoff |
| 7 | Dossier "cinematic pass deferred" | Wall of 7 identical boxes, no hero, evidence at 11px |
| 8 | Dead `src/diagnosis.ts`; stale `CLAUDE.md` repo-map | Cruft |

## 3. Target information architecture

Two top-level destinations + one drill-in, one coherent chrome.

```
NavBar:  [ RADAR ]  [ DOSSIER ]           ← replaces [ Radar ] [ Habits ]
            │           │
            │           └─ full-page retrospective (absorbs Habits)
            │
            └─ live 3D war-room (default on summon)
                  │
                  └─ click an agent  ──►  DIAGNOSIS drill-in (evidence + fix-preview)
                     or "Diagnose my workflow" button
```

- **Habits tab is removed.** Its backend keeps running; its value (streak progress, catch→prove→erase) surfaces inside DOSSIER §2.
- **`#open-profile` raw button is deleted.** DOSSIER becomes a real NavBar destination. This kills the top-left overlap at its root.
- **Terminal `Console` + free-text ask is deleted.** Diagnosis is triggered by (a) clicking an agent orb, (b) one "Diagnose my workflow" button. The pipeline stream is restyled as an elegant progress reveal, not a scrolling terminal.

## 4. Surface designs

### 4.1 RADAR (hero)

Data feed unchanged (live radar snapshots → `bridge.ts` → `RadarConstellation`).

1. **Geometry** — commit the finished Fibonacci-sphere layout (`radarLayout.ts`, 24 tests green). Isotropic → no edge-on line collapse. Keep the `layoutFn` injection seam and the `radarCompare` harness as dev tooling.
2. **Camera choreography (the "viewable" fix)** — the missing half:
   - **Auto-fit-to-bounds**: compute the live node bounding sphere and frame it on summon and on fleet change (no more fixed `OVERVIEW_DIST` constant that under/over-shoots).
   - **Composed hero angle**: a deliberate default azimuth/elevation that reads as an intentional shot, not a random orbit position.
   - **Gentle idle auto-orbit**: slow drift when idle (paused on interaction), for cinematic life. *(Motion judged by code — HR2.)*
   - **Reframe control**: a always-available "fit" affordance to recover a good view from any orbit state.
3. **Hierarchy legibility** — harness color+glyph+label (a11y: never color alone), orchestrator→subagent tethers, readable folder-family grouping, depth cueing so the sphere doesn't read as a featureless ball.

### 4.2 DIAGNOSIS (drill-in)

- Entry: click agent orb, or one prominent **"Diagnose my workflow"** button (runs `run_diagnosis`, existing command).
- The `PipelineRail` stays but restyled: a composed multi-stage progress reveal (Diagnostician → Coach → Verifier), not terminal text.
- Keep the real, existing forensic readout: ranked holes, severity, cost ledger, evidence drill-down (`resolve_evidence` fallback), read-only fix-preview + Forge apply/revert. Restyle to match the product, remove terminal affordances.

### 4.3 DOSSIER (retrospective, full rebuild)

Full-page surface, restyled from scratch (replaces the "cinematic pass deferred" `ProfileScreen.tsx`). Backend feeds unchanged: `build_profile` / `get_profile` / `get_activity_heatmap` / `get_efficiency_score`; live `habits_refreshed` for §2. Windows: 2wk · 30d · 3mo · 6mo · all.

Top→bottom, with real hierarchy (not 7 identical boxes):

1. **Header** — `DOSSIER`, operator identity, window toggle, close. Detector-only badge explained (what it means + that setting an API key upgrades it), not just labeled.
2. **① THE SCORE (hero)** — efficiency headline (0–100) as the visual anchor + a one-line generated verdict ("strongest at X, you lose most to Y"). 7 rubric families as a compact secondary breakdown.
3. **② WHAT YOU'RE BREAKING (Habits, folded in)** — the section with a pulse. Each active anti-pattern: streak bar (`credits/K`), *"N/K clean sessions · M more to erase,"* the erase-reward state on `fixed`. Live via `habits_refreshed`. This is the catch→prove→erase loop made visible.
4. **③ WHERE YOU LOSE (ranked leaks)** — top 5, real token/minute cost, **evidence promoted to first-class** (prominent, clickable citations — "by Proof" made visible), not 11px toggles.
5. **④ WHO YOU ARE (dimensions)** — strengths vs holes *contrasted*; punchy claims with status (asserted/emerging), not 7 hedged paragraphs.
6. **⑤ YOUR RANGE** — project archetypes + activity heatmap.
7. **⑥ TRAJECTORY** — graceful sparse state (*"still gathering — 2 of 3 weeks"*) instead of a broken-looking empty row.

Cold-start: a real "generate your dossier" moment with anticipation, not a debug `<ol>` of stage strings.

## 5. Chrome & cleanup

- NavBar → `RADAR | DOSSIER`.
- Delete `#open-profile` from `index.html`; wire DOSSIER open/close through React nav + keep the `D` hotkey (guarded).
- Introduce **one coherent layout system** for floating chrome (a small set of safe-area anchor zones: top-left / top-center / top-right / bottom-center) so controls never collide as content grows; retire ad-hoc per-element `top/left/bottom` pixel offsets where they cause collisions.
- Delete dead `src/diagnosis.ts`.
- Truth up `CLAUDE.md` repo-map (remove references to non-existent `#terminal`/`#screen`/`#prompt`/`#command`/`#hud-*`/`#status`; note Habits folded into Dossier).

## 6. Non-goals (YAGNI)

- No backend/Rust changes beyond what a frontend needs (no new commands; reuse existing).
- No new persistence, no new engine calls.
- Not touching M5/M6/M7 stubs, Forge internals, adapters, or ingest.
- Not re-solving Radar geometry (already solved). Camera + legibility only.
- No auth, no network surfaces added.

## 7. Data contracts (all existing — for reference)

- **Radar**: live snapshot model → `bridge.ts` → `RadarConstellation` (`layoutFn` seam).
- **Diagnosis**: `run_diagnosis`; events `fugu_delta`/`fugu_usage`/`candidates_nominated`/`finding_verdict`/`diagnosis_ready`; `resolve_evidence`.
- **Dossier**: `build_profile`/`get_profile`/`get_activity_heatmap`/`get_efficiency_score`; `dossier_progress` event; `Profile` struct (efficiency, 7 dimensions, ranked_leaks, archetypes, trajectory, detector_only).
- **Habits (folded)**: `set_habits_window`; events `habits_refreshed` (windowed `OrbIssue[]` w/ `credits`/`streak_k`/`fixed`/`last_credit_at`), `habits_diagnosed`.

## 8. Testing & verification

- **FE unit**: `pnpm test` (vitest) stays green; keep/extend `radarLayout.test.ts`; add tests for new camera-fit math (pure functions) and any Dossier data-shaping helpers.
- **Typecheck+bundle**: `pnpm build` (tsc + vite) green.
- **Motion**: judged by **code only** (HR2) — camera choreography, idle orbit, streak/erase transitions read in source, not a browser.
- **Buttons/flows/layout**: verified live via the vite dev server + browser QA harnesses (`radar-compare.html`, a Dossier preview harness) — screenshots/DOM inspection for overlap, nav, Dossier sections.
- **Baseline**: establish green `pnpm build` + `pnpm test` before edits so regressions are attributable.

## 9. Security

No new write surfaces. The **only** sanctioned user-file write remains Forge (guardrail block in `~/.claude/CLAUDE.md`), untouched. Habits' `durable_erase_fixed` (AllTime-gated) already routes through Forge — unchanged. Everything else renders previews/diffs only. No secrets in the frontend; engine keys stay server-side in Rust. No new network calls.

## 10. Phasing (for the plan)

- **Phase A — Chrome & IA foundation** (first; everything hangs off it): NavBar `RADAR|DOSSIER`, delete `#open-profile`, unify layout anchors, kill terminal Console + free-text ask → click-to-diagnose + button + restyled pipeline, delete `diagnosis.ts`, truth up docs.
- **Phase B — Radar hero**: commit sphere fix, camera choreography, hierarchy legibility. (Owns radar/orb/camera files + radar CSS.)
- **Phase C — Dossier rebuild + Habits fold-in**: full 6-section restyle, hero score, evidence-forward, habits streaks section, graceful trajectory, cold-start. (Owns `src/viz/profile/**` + its own scoped styles → disjoint from B to allow parallelism.)

B and C run in parallel after A, with **disjoint file ownership** (B: `style.css` radar regions; C: profile-scoped styles) to avoid conflicts. Final Opus verification pass at the end.
