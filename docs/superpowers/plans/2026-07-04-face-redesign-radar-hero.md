# FACE Redesign — "Radar is the Hero" Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn WARDEN into a tight, demoable product — a beautiful/viewable live-fleet RADAR (hero), one-click evidence-cited DIAGNOSIS, and a rebuilt DOSSIER that absorbs Habits — with no button overlap, no terminal-chat, no dead code.

**Architecture:** Frontend-only. Three phases: (A) chrome & IA foundation, then (B) Radar hero and (C) Dossier rebuild in parallel with **disjoint file ownership**. B owns radar/orb/camera + radar regions of `style.css`; C owns `src/viz/profile/**` + its own scoped stylesheet. Backends untouched — all data feeds already exist.

**Tech Stack:** Vite + React + TypeScript, React Three Fiber (`@react-three/fiber` + `drei`) for Radar, Remotion compositions, vanilla `style.css` phosphor tokens, Tauri `invoke`/`emit` IPC, vitest (jsdom per-file pragma).

## Global Constraints

- Phosphor palette tokens (verbatim): `--bg #020403`, `--green #76ff9d`, `--dim #1b6f3a`, `--acid #b8ff6b`, `--warn #ffd166`, `--red #ff5470`, verdict/amber `--ff5a37`. Harness colors: Claude emerald `#3dffa0`, Codex violet `#b98cff`, verdict-amber `#ff5a37`.
- **A11y:** always pair color with a glyph + label (color-blind). Never encode meaning in color alone.
- **Honest viz:** every visual maps to a real signal; no fabricated numbers/states. Degrade gracefully (blank API key → deterministic profile is a first-class surface).
- **No backend changes.** Reuse existing commands/events only. The only user-file write surface is Forge — untouched.
- **Motion is judged by CODE only** (never a browser). Buttons/flows/layout are verified live in the browser.
- Package manager **pnpm**. Gates: `pnpm build` (tsc+vite) and `pnpm test` (vitest) stay green. Commit frequently; **never push / open PR** without explicit instruction.
- Overlay is a native macOS window; controls live inside the React chrome (nothing bolted onto `index.html`).

---

## File structure

**Phase A (chrome/IA):** `src/viz/NavBar.tsx`, `src/viz/WarRoom.tsx`, `src/viz/chrome.tsx`, `src/main.ts`, `index.html`, `src/style.css` (chrome/layout regions), delete `src/diagnosis.ts`, edit `CLAUDE.md`.
**Phase B (radar):** `src/viz/radarLayout.ts` (+test), `radarLayoutVariants.ts`, `preview/radarCompare.tsx`, `radar-compare.html`, `RadarConstellation.tsx`, `CameraRig.tsx`, `useOrbCamera.ts`, `Orb.tsx`, `radarTheme.ts`, new `src/viz/cameraFit.ts` (+test), `style.css` (radar regions only).
**Phase C (dossier):** rebuild `src/viz/profile/ProfileScreen.tsx`; new `ScoreHero.tsx`, `BreakingSection.tsx`, `LeaksSection.tsx`, `DimensionsSection.tsx`, `RangeSection.tsx`, `TrajectorySection.tsx`, `EvidenceList.tsx`, `verdict.ts` (+test) — all under `src/viz/profile/`; new scoped `src/viz/profile/dossier.css`; keep `mount.tsx`, `heatmap.ts`, `types.ts`.

---

## Phase A — Chrome & IA foundation

*Do first; B and C hang off it. One subagent, sequential tasks, building loop.*

### Task A0: Green baseline
- [ ] `pnpm test` and `pnpm build` — both green. Record counts. Any regression after this is attributable.

### Task A1: NavBar → RADAR | DOSSIER + top-level view state
**Files:** Modify `src/viz/NavBar.tsx`, `src/viz/WarRoom.tsx`, `src/viz/profile/mount.tsx` (reuse `openProfile`/`closeProfile`/`toggleProfile`).
**Interfaces — Produces:** `WarRoom` view state `view: 'radar' | 'dossier'` (radar default); `NavBar` props `{ view, onView(v), counts }`.
- [ ] Replace `ConstellationTab = 'habits' | 'radar'` and the Radar/Habits tab pair with a `RADAR | DOSSIER` pill. Remove the Habits tab entirely.
- [ ] `onView('dossier')` calls `openProfile()`; `onView('radar')` calls `closeProfile()` and shows the war-room. Keep `PRIMARY = 'radar'`.
- [ ] Remove all `tab === 'habits'` branches in `WarRoom.tsx`/`chrome.tsx` (HabitsDial, habits-only FilterBar chips, habits empty-state) — Habits UI is leaving the war room.
- [ ] **Verify (browser):** clicking DOSSIER opens the profile overlay; RADAR returns to the fleet; no Habits tab anywhere.
- [ ] Commit: `refactor(face): nav becomes RADAR | DOSSIER, drop Habits tab`.

### Task A2: Delete the bolted-on `#open-profile` (fixes the overlap at root)
**Files:** Modify `index.html` (remove `<button id="open-profile">`), `src/main.ts` (remove its wiring; keep the guarded `D` hotkey → `toggleProfile()`).
- [ ] **Verify (browser):** no raw button in the top-left; DOSSIER opens via NavBar + `D` only; top-left corner no longer stacks two controls.
- [ ] Commit: `fix(face): remove out-of-React DOSSIER button (top-left overlap)`.

### Task A3: Unify floating-control layout — safe-area anchor zones
**Files:** Modify `src/style.css` (chrome/layout regions).
- [ ] Define four anchor zones as utility classes driven by CSS safe-area vars — `.wd-anchor-tl / -tc / -tr / -bc` — each with consistent insets (`--top-safe`, `--side-margin`, bottom inset). Route the HUD, side-toggle, nav, filterbar, inspector, ledger through these zones instead of ad-hoc per-element `top/left/bottom` offsets.
- [ ] Resolve the known bottom-edge collision: the (now-removed) console dock and `.wd-filterbar` no longer share the bottom; ensure `.wd-filterbar` centers cleanly.
- [ ] **Verify (browser):** `preview_resize` mobile/tablet/desktop — no overlaps at any width; grow the harness-chip strip (many agents) and confirm no collision with nav/toggle.
- [ ] Commit: `refactor(face): coherent safe-area anchor zones for chrome`.

### Task A4: Kill the terminal Console + free-text ask → click-to-diagnose + one button
**Files:** Modify `src/viz/chrome.tsx` (remove `Console`, `DEFAULT_QUERY`, `.wd-console` form; restyle `PipelineRail` → progress reveal; add a "Diagnose my workflow" button), `src/viz/WarRoom.tsx` (keep `onAsk` wiring; button + orb-click both trigger it), `src/style.css` (drop `.wd-console`/`.wd-ask` terminal styling; add progress-reveal styling).
**Interfaces — Consumes:** existing `onAsk(query?)` → `invoke('run_diagnosis', ...)`.
- [ ] Replace the terminal ask form with a single **"Diagnose my workflow"** button (runs the pipeline with the implicit workflow query — no user-typed prompt, no `▸`, no prefilled text).
- [ ] Ensure clicking an agent orb exposes a clear **Diagnose** affordance in its inspector.
- [ ] Restyle `PipelineRail` as a composed 3-stage progress reveal (Diagnostician → Coach → Verifier) — no scrolling terminal transcript.
- [ ] **Verify (browser):** no text input / terminal chrome anywhere; button runs the real pipeline; stages animate as a progress reveal. **Motion read in code.**
- [ ] Commit: `feat(face): diagnosis is click-to-run, terminal ask removed`.

### Task A5: Delete dead code + truth up docs
**Files:** Delete `src/diagnosis.ts`; modify `CLAUDE.md` repo-map.
- [ ] Confirm nothing imports `diagnosis.ts` (grep `from ['\"].*diagnosis['\"]`), delete it.
- [ ] `CLAUDE.md`: remove the stale `#terminal/#screen/#prompt/#command/#hud-*/#status` DOM references; note nav is `RADAR | DOSSIER` and Habits folded into Dossier.
- [ ] **Verify:** `pnpm build` green (no broken import); `pnpm test` green.
- [ ] Commit: `chore(face): delete dead diagnosis.ts, truth up repo-map`.

---

## Phase B — Radar hero

*After A. Owns radar/orb/camera files + radar regions of `style.css`. One subagent, building loop. Uses `r3f-craft` + `frontend-motion` skills.*

### Task B1: Lock the finished sphere geometry
**Files:** the already-modified `radarLayout.ts` (+ `.test.ts`), `radarLayoutVariants.ts`, `preview/radarCompare.tsx`, `radar-compare.html`, `RadarConstellation.tsx`, `CameraRig.tsx`, `useOrbCamera.ts`.
- [ ] `pnpm test` — the 24 layout tests (3-axis-span assertions) + full suite green. `pnpm build` green.
- [ ] Commit the Fibonacci-sphere layout + `layoutFn` seam + compare harness: `feat(radar): isotropic Fibonacci-sphere layout (no edge-on line collapse)`.

### Task B2: Camera auto-fit-to-bounds (pure math + wiring)
**Files:** Create `src/viz/cameraFit.ts` + `src/viz/cameraFit.test.ts`; modify `useOrbCamera.ts`/`CameraRig.tsx`.
**Interfaces — Produces:** `fitDistanceForBounds(points: {x,y,z}[], fovRadians: number, aspect: number, margin?: number): { target: [number,number,number], distance: number }` — target = centroid, distance = smallest that contains the bounding sphere in frame (account for the narrower of vertical/horizontal FOV).
- [ ] **Step 1 (failing test):** points spanning a known radius → distance ≥ radius/tan(fov/2)·margin; target ≈ centroid; empty array → sane default (`OVERVIEW_DIST`).
- [ ] **Step 2:** run, verify fail.
- [ ] **Step 3:** implement `fitDistanceForBounds` (centroid, max radius from centroid, `dist = (r*margin)/sin(min(vfov,hfov)/2)`).
- [ ] **Step 4:** run, verify pass.
- [ ] **Wire:** frame via this on summon + when node count/bounds change materially (not every tick — debounce/threshold). Replace the fixed overview constant as the *initial* pose source.
- [ ] **Verify (browser):** a busy multi-folder fleet lands fully framed; `radar-compare.html` at 40 agents fits.
- [ ] Commit: `feat(radar): camera auto-fit to live fleet bounds`.

### Task B3: Composed hero angle + idle auto-orbit + reframe control
**Files:** Modify `CameraRig.tsx`/`useOrbCamera.ts`; add a "reframe/fit" control to `chrome.tsx` (radar view).
- [ ] Default pose = a deliberate azimuth/elevation (¾ view), not axis-aligned.
- [ ] Idle slow auto-orbit (small angular velocity), **paused on any pointer/gesture interaction, resumes after an idle delay**.
- [ ] Reframe button recalls `fitDistanceForBounds` + eases to the hero pose.
- [ ] **Verify:** motion **read in code** (orbit velocity, pause/resume, easing); browser only to confirm the reframe button recovers a good view.
- [ ] Commit: `feat(radar): composed hero angle, idle orbit, reframe control`.

### Task B4: Hierarchy legibility
**Files:** Modify `Orb.tsx` (billboarded label + harness glyph), `RadarConstellation.tsx` (orchestrator→subagent tethers, folder-family grouping/depth cue), `radarTheme.ts` (reuse colors/glyphs).
- [ ] Each orb: harness **color + glyph + short label** (a11y). Depth/size cue so the sphere isn't a featureless ball.
- [ ] Draw faint tethers orchestrator→subagents; visually group folder families.
- [ ] **Verify (browser):** hierarchy readable at a glance; label/glyph present regardless of color; screenshot for the demo.
- [ ] Commit: `feat(radar): legible hierarchy — glyph+label, tethers, family depth`.

---

## Phase C — Dossier rebuild + Habits fold-in

*After A, parallel to B. Owns `src/viz/profile/**` + `dossier.css` only. One subagent, building loop. Uses `frontend-design` + `ui-ux-pro-max` + `frontend-motion`.*

Data (existing): `build_profile`/`get_profile`/`get_activity_heatmap`/`get_efficiency_score`; `dossier_progress` event; `Profile{ efficiency{headline,families[7],session_count}, dimensions[7]{key,title,narrative,claims[]{confidence,status,evidence[]}}, ranked_leaks[≤5]{rank,title,est_cost_tokens,est_cost_minutes,evidence[]}, archetypes[], trajectory[], detector_only }`. Habits (live): `set_habits_window`, event `habits_refreshed` → `OrbIssue[]` with `credits/streak_k/fixed/last_credit_at`.

### Task C1: Dossier shell + scoped styles + header + window toggle
**Files:** Rebuild `src/viz/profile/ProfileScreen.tsx`; create `src/viz/profile/dossier.css`; keep `mount.tsx`.
- [ ] Full-page surface; sticky header: `DOSSIER · Profile by Proof`, operator identity, close (✕), 5-way window toggle (`2wk/30d/3mo/6mo/all`). Detector-only mode gets a one-line **explainer** ("deterministic — set `WARDEN_BRAIN_API_KEY` for narrative"), not a bare badge.
- [ ] All styling in `dossier.css` (scoped, disjoint from `style.css`), phosphor tokens.
- [ ] **Verify (browser):** shell renders; window toggle re-fetches (`get_profile` → `build_profile` if stale); loading state is a graceful skeleton, not a debug list.
- [ ] Commit: `feat(dossier): new shell — header, window toggle, scoped styles`.

### Task C2: § THE SCORE (hero) + verdict helper
**Files:** Create `ScoreHero.tsx`, `verdict.ts` (+ `verdict.test.ts`).
**Interfaces — Produces:** `verdictLine(efficiency): string` — from families, name strongest + weakest → "strongest at {X}, you lose most to {Y}".
- [ ] TDD `verdictLine`: given families with known scores → correct strongest/weakest names; empty → safe fallback string.
- [ ] `ScoreHero`: large 0–100 headline (real `headline*100`), the verdict line, 7-family compact bars beside it. This is the eye's first anchor.
- [ ] **Verify (browser):** score dominates; verdict reads true.
- [ ] Commit: `feat(dossier): efficiency score hero + verdict`.

### Task C3: § WHAT YOU'RE BREAKING (Habits fold-in)
**Files:** Create `BreakingSection.tsx`; subscribe to `habits_refreshed`; call `set_habits_window` on the Dossier window change (map Dossier windows → habits windows).
**Interfaces — Consumes:** `OrbIssue` streak fields.
- [ ] Render each active anti-pattern as a row: title, streak bar `credits/streak_k`, **"N/K clean sessions · M more to erase"** (M = `max(0, K−credits)`), cost. On `fixed` → an **erase-reward** state (satisfying resolve).
- [ ] Empty state: "nothing in remediation — WARDEN will surface habits to break as it finds them."
- [ ] **Verify:** browser for rows/labels; **erase-reward motion read in code.**
- [ ] Commit: `feat(dossier): "what you're breaking" — habit streaks folded in`.

### Task C4: § WHERE YOU LOSE (leaks, evidence-forward)
**Files:** Create `LeaksSection.tsx`, `EvidenceList.tsx`.
- [ ] Top-5 leaks: rank, title, real token/minute cost. **Evidence promoted** — a visible, clickable citation list (`EvidenceList`) resolving via `resolve_evidence` fallback, not an 11px toggle.
- [ ] **Verify (browser):** evidence is prominent and drills in.
- [ ] Commit: `feat(dossier): ranked leaks with first-class evidence`.

### Task C5: § WHO YOU ARE (dimensions contrasted)
**Files:** Create `DimensionsSection.tsx`.
- [ ] Present the dimensions as **strengths vs holes contrasted** (two columns / clear visual split), punchy claims with `asserted/emerging` status chips + confidence — not 7 identical paragraphs.
- [ ] **Verify (browser):** scannable; strengths/holes visually distinct.
- [ ] Commit: `feat(dossier): dimensions as contrasted strengths/holes`.

### Task C6: § YOUR RANGE (archetypes + heatmap)
**Files:** Create `RangeSection.tsx`; reuse `heatmap.ts`.
- [ ] Archetype cards (name, session count, note, project chips) + the activity heatmap (real token contribution grid) with a clean tooltip.
- [ ] **Verify (browser):** heatmap renders real cells; cards read well.
- [ ] Commit: `feat(dossier): range — archetypes + activity heatmap`.

### Task C7: § TRAJECTORY (graceful sparse) + cold-start
**Files:** Create `TrajectorySection.tsx`; cold-start build experience in `ProfileScreen.tsx`.
- [ ] Trajectory rows (v1 = "outcome") with sparkline + direction glyph. **Graceful sparse state**: when `Insufficient`, show "still gathering — needs 3+ weeks; here's what we have" with visual grace, never a broken-looking empty muted row.
- [ ] Cold-start (no cached profile): an inviting "generate your dossier" moment; while building, an anticipatory progress from `dossier_progress` (not a raw `<ol>` of stage strings).
- [ ] **Verify (browser):** both sparse and populated states read intentionally; cold-start feels like a reveal.
- [ ] Commit: `feat(dossier): graceful trajectory + cold-start reveal`.

---

## Final verification (ONE Opus pass at the very end)
- [ ] `pnpm build` + `pnpm test` green.
- [ ] Live app (`pnpm tauri dev` or dev harnesses): summon → RADAR frames nicely & reads as hierarchy; click agent → DIAGNOSIS runs with progress reveal + evidence; NavBar → DOSSIER shows all 6 sections with the score hero + breaking streaks + evidence-forward leaks; no button overlap at any size; no terminal/chat anywhere.
- [ ] Motion (camera, idle orbit, erase-reward, progress reveal) reviewed in **code**.
- [ ] Screenshots of RADAR + DOSSIER for the showcase.

## Self-review notes
- **Spec coverage:** every spec §4/§5 item maps to a task (nav→A1, overlap→A2/A3, terminal→A4, dead code→A5, geometry→B1, viewable→B2/B3, legibility→B4, dossier §1–6→C1–C7, habits fold→C3). ✓
- **Type consistency:** `fitDistanceForBounds`, `verdictLine`, `view:'radar'|'dossier'`, `OrbIssue` streak fields used consistently. ✓
- **Parallel safety:** B touches `style.css` radar regions; C touches only `dossier.css` + `profile/**` → disjoint. A completes before both. ✓
