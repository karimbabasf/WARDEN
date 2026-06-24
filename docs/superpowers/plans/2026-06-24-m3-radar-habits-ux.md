# M3 RADAR / Habits UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Habits/Radar constellations decodable at scale, the legend an interactive color-only filter, the Radar navigable (tamed zoom + fly-to framing), and the window a persistent resizable app.

**Architecture:** New logic lands as **small pure modules** (`harnessColors`, `emphasis`, `cameraFraming`, plus rewritten `orbLayout` / extended `radarLayout`) that return data; existing React components render that data and own only animation refs. Props-drilling from `WarRoom.tsx` is kept (no global store). Rust window changes flip the overlay from a blur-dismissed HUD to a persistent window.

**Tech Stack:** React 18 + React-Three-Fiber + three.js + drei (`OrbitControls`), Vitest, Tauri v2 (Rust).

**Spec:** `docs/superpowers/specs/2026-06-24-m3-radar-habits-ux-design.md`

## Global Constraints
- **Honest viz:** nodes/flares map to real signals only. Flat/`codex_vscode`/unknown agents stay solo + neutral; `estimated == null` shows "—". Never fabricate.
- **Preview-only:** no writes to user projects.
- **Pure layout/filter/framing functions** — deterministic, no RNG, unit-tested.
- **Color-only emphasis:** the dim channel must affect only color saturation/brightness — never scale, position, or the select/hover boost.
- **No flaky/snappy motion:** all transitions eased and damped (filter crossfade ~300ms; camera fly-to ~700ms expo).
- **a11y:** every color paired with glyph + label; legend chips keyboard-focusable with `aria-pressed`.
- **Package manager:** pnpm. Platform: macOS Apple Silicon.
- **Verify gate per task:** the task's own unit test passes; integrative `pnpm build` + `cargo test` run at phase boundaries (not inside parallel tasks).

---

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `src/viz/harnessColors.ts` (new) | 1 | Single source of truth: harness → `{hue, glyph, label}` |
| `src/viz/harnessTheme.ts` / `radarTheme.ts` (mod) | 1 | Import harness hue/glyph/label from `harnessColors` |
| `src/viz/emphasis.ts` (new) | 2 | `EmphasisFilter`, `severityBucket`, `matchesFilter`, `targetDim` |
| `src/viz/cameraFraming.ts` (new) | 3 | `Bounds`, `subtreeBounds`, `frameDistance` |
| `src/viz/orbLayout.ts` (rewrite) | 4 | Habits: harness zones on arc + collision-free packing + severity fan |
| `src/viz/radarLayout.ts` (extend) | 5 | Radar: harness sectors + multi-shell siblings + subtree-scaled roots |
| `src-tauri/tauri.conf.json`, `src/lib.rs`, `src/commands.rs` (mod) | 6 | Persistent window: no blur-dismiss, minimize/hide, Regular policy |
| `src/viz/Orb.tsx`, `RadarConstellation.tsx` (mod) | 7 | Animate dim smoothly (color-only) toward target from props |
| `src/viz/CameraRig.tsx` (mod) | 8 | Tame zoom (min-dist + FOV taper), fly-to `focusBounds`, constrained pan |
| `src/viz/WarRoom.tsx` (mod) | 9 | `emphasisFilter` + `focusStack` state; per-node dim; compute `focusBounds`; wire all |
| `src/viz/chrome.tsx` (mod) | 10 | Interactive legend chips + breadcrumb + window controls |
| `src/style.css` (mod) | 11 | Layout tokens, z-index scale, `--top-safe`, responsive clamps, chip/control styles |
| — | 12 | Full verification + PROGRESS/memory |

**Phasing (dependency-respecting):**
- **Phase 1 (parallel, disjoint files):** Tasks 1–5 + Task 6 (Rust). Each runs only its own unit test.
- **Phase 2 (parallel, disjoint files; needs Phase 1 contracts):** Tasks 7, 8.
- **Phase 3 (cross-cutting, orchestrator-owned, sequential):** Tasks 9, 10, 11.
- **Phase 4:** Task 12 verification.

---

## Task 1: Harness color single source of truth

**Files:**
- Create: `src/viz/harnessColors.ts`
- Modify: `src/viz/harnessTheme.ts`, `src/viz/radarTheme.ts` (replace hard-coded claude/codex hues + glyph + label with imports; keep severity ramp and heat curve untouched)
- Test: `src/viz/harnessColors.test.ts`

**Interfaces:**
- Produces:
  ```ts
  export interface HarnessColor { id: 'claude_code'|'codex'|'unknown'; hue: string; glyph: string; label: string }
  export const HARNESS_COLORS: Record<'claude_code'|'codex'|'unknown', HarnessColor>
  export function harnessColor(harness: string | null | undefined): HarnessColor // unknown fallback
  ```
- Chosen unified hues (validated for contrast on `--bg #020403` in Task 12): `claude_code → #ff8c42` (orange, glyph `◆`, label `Claude`), `codex → #b98cff` (violet, glyph `▣`, label `Codex`), `unknown → #8fa0b8` (slate, glyph `◇`, label `Unknown`).

- [ ] **Step 1: Write the failing test** — `harnessColor('claude_code').hue === '#ff8c42'`; `harnessColor('codex').glyph === '▣'`; `harnessColor('weird').id === 'unknown'`; `harnessColor(null).label === 'Unknown'`.
- [ ] **Step 2:** Run `npx vitest run src/viz/harnessColors.test.ts` → FAIL (module missing).
- [ ] **Step 3:** Implement `harnessColors.ts` with the table above; `harnessColor` lowercases/normalizes and falls back to `unknown`.
- [ ] **Step 4:** Update `harnessTheme.ts` and `radarTheme.ts` to source claude/codex/unknown `color`, `glyph`, `label` from `harnessColor(...)`. Leave `severityColor` (issues) and `heatColor` (fill) exactly as-is.
- [ ] **Step 5:** Run `npx vitest run src/viz/harnessColors.test.ts` → PASS.
- [ ] **Step 6:** Commit `feat(viz): unify harness colors into one source of truth`.

---

## Task 2: Emphasis filter (color-only)

**Files:**
- Create: `src/viz/emphasis.ts`
- Test: `src/viz/emphasis.test.ts`

**Interfaces:**
- Produces:
  ```ts
  export type EmphasisFilter =
    | { kind: 'severity'; bucket: 'low'|'med'|'high'|'crit' }
    | { kind: 'harness'; harness: string }
    | null
  export interface EmphasisNode { harness?: string | null; severity?: number | null }
  export function severityBucket(severity: number): 'low'|'med'|'high'|'crit'
  export function matchesFilter(node: EmphasisNode, filter: EmphasisFilter): boolean // true when filter null OR node matches
  export function targetDim(node: EmphasisNode, filter: EmphasisFilter): number       // 0 = full, 1 = dimmed
  ```
- `severityBucket`: `<=2 → low`, `3 → med`, `4 → high`, `>=5 → crit` (matches the issue color ramp thresholds).
- `targetDim` = `filter == null ? 0 : matchesFilter(node, filter) ? 0 : 1`.

- [ ] **Step 1: Failing test** — bucket thresholds (1,2→low; 3→med; 4→high; 5,6→crit); `matchesFilter` truth table for severity + harness across claude/codex/unknown nodes; null filter → all match → `targetDim` 0; harness filter dims the non-matching harness to 1.
- [ ] **Step 2:** `npx vitest run src/viz/emphasis.test.ts` → FAIL.
- [ ] **Step 3:** Implement the three pure functions.
- [ ] **Step 4:** Run test → PASS.
- [ ] **Step 5:** Commit `feat(viz): emphasis filter (color-only dim logic)`.

---

## Task 3: Camera framing math

**Files:**
- Create: `src/viz/cameraFraming.ts`
- Test: `src/viz/cameraFraming.test.ts`

**Interfaces:**
- Consumes: `RadarAgent` (from `radarTypes.ts`) for hierarchy (`id`, `parentId`).
- Produces:
  ```ts
  export interface Bounds { center: [number, number, number]; radius: number }
  // positions: id -> { pos, radius } from the radar layout
  export function subtreeBounds(
    positions: Map<string, { pos: [number,number,number]; radius: number }>,
    agents: RadarAgent[], rootId: string): Bounds
  export function frameDistance(boundingRadius: number, fovDeg: number, fill?: number): number // fill default 0.6
  ```
- `frameDistance(r, fov, fill=0.6)` = `r / (Math.tan((fov*Math.PI/180)/2) * fill)`. Monotonic increasing in `r`.
- `subtreeBounds`: collect `rootId` + all transitive descendants (walk `parentId` graph), compute the enclosing sphere (center = mean of member centers, radius = max distance-to-center + member radius). If a node id is absent from `positions`, skip it.

- [ ] **Step 1: Failing test** — `frameDistance` larger for larger radius, exact value at fov=46,r=2,fill=0.6; `subtreeBounds` on a 1-root-2-child fixture encloses all three (every member center within `radius` of `center`); a leaf root returns its own pos + radius.
- [ ] **Step 2:** `npx vitest run src/viz/cameraFraming.test.ts` → FAIL.
- [ ] **Step 3:** Implement both functions (BFS over children built from `parentId`).
- [ ] **Step 4:** Run test → PASS.
- [ ] **Step 5:** Commit `feat(viz): camera framing math (subtree bounds + frame distance)`.

---

## Task 4: Habits layout — harness zones + severity (rewrite `orbLayout.ts`)

**Files:**
- Modify (rewrite internals): `src/viz/orbLayout.ts`
- Test: `src/viz/orbLayout.test.ts` (extend; keep existing severity→latitude assertion)

**Interfaces:**
- Consumes: `OrbSceneModel`, `OrbAgent`, `OrbIssue`, `LayoutNode`, `OrbLink`, `OrbLayout` (read `orbTypes.ts` — **do not change these types or the exported function name/signature**). Output `OrbLayout { nodes, links }` shape is FROZEN; only positions change.
- Produces: same `OrbLayout`. Layout grammar = zone(harness) → agent cluster → issue severity.

**Algorithm (deterministic, no RNG):**
1. `groupByHarness(agents)` → zones for `claude_code`, `codex`, `unknown` (only non-empty zones).
2. `placeZones(zones)`: assign each zone a center along a **shallow camera-facing arc** — spread zones across X, with a mild Z curvature toward the camera (e.g. `z = -k*(1 - cos(zoneAngle))`), so zones never line up front-to-back. Width per zone scales with its packed extent.
3. `packZone(zone)`: place agent hubs by golden-angle phyllotaxis sized so each hub footprint = `hubRadius + issueShellRadius + margin`; then run a **bounded pairwise separation pass** (≤ 24 iterations, clamped displacement) until no two footprints overlap. Deterministic.
4. `fanIssues(hub, issues)`: keep severity→latitude (crit toward front/top); shell radius enforces a **minimum angular gap** between issues so high-count agents fan out, not bunch.
5. Links: one hub→issue link per issue (unchanged kind), preserving what `Constellation.tsx` expects.

**Implementer note:** spawn a Haiku sub-agent to read `orbLayout.ts`, `orbTypes.ts`, `orbLayout.test.ts`, and how `Constellation.tsx` consumes the output, before editing. Keep your own context for the math.

- [ ] **Step 1: Extend tests** — keep severity→latitude; add: **no-overlap** (∀ node pairs, `dist(centers) ≥ r₁+r₂` with small margin); **zone grouping** (all hubs of a harness inside that zone's X/Z bounds; zones' bounds disjoint); **determinism** (two runs identical); **scale** (30 agents × up to 12 issues runs and stays non-overlapping).
- [ ] **Step 2:** `npx vitest run src/viz/orbLayout.test.ts` → FAIL on new assertions.
- [ ] **Step 3:** Rewrite `orbLayout.ts` internals per the algorithm; keep exports identical.
- [ ] **Step 4:** Run test → PASS (all, including kept ones).
- [ ] **Step 5:** Commit `feat(viz): habits harness-zone + severity layout (collision-free)`.

---

## Task 5: Radar layout — sectors + multi-shell + subtree spacing (extend `radarLayout.ts`)

**Files:**
- Modify: `src/viz/radarLayout.ts`
- Test: `src/viz/radarLayout` spec (extend existing tests; keep `radarHonesty.test.ts` green)

**Interfaces:**
- Consumes: `RadarAgent`, `RadarSceneModel` (read `radarTypes.ts`). Output shape FROZEN (nodes with `id`/`pos`/`radius`/`depth`, parent→child links); only placement changes.
- Produces: same layout shape; additionally guarantee a `positions` map keyed by id is derivable (used by Task 9 → `cameraFraming.subtreeBounds`).

**Algorithm (preserve honest-viz):**
1. **Harness sectors:** partition roots by harness; give each harness an angular arc on the root ring; place its roots within that arc.
2. **Subtree-scaled root spacing:** root ring radius / arc width per root scales with the root's descendant count (busy orchestrators get more room).
3. **Multi-shell siblings:** when a parent's `childCount` exceeds what one ring holds at a minimum angular gap, distribute children across 2+ concentric orbital shells with a small per-child radial stagger.
4. Flat (`origin === 'codex_vscode'`) / unknown agents stay solo roots; links only when parent resolves and is not flat (UNCHANGED honesty rules).

**Implementer note:** spawn a Haiku sub-agent to read `radarLayout.ts`, `radarTypes.ts`, `radarHonesty.test.ts`, and `RadarConstellation.tsx`'s consumption before editing.

- [ ] **Step 1: Extend tests** — **sector grouping** (roots of a harness fall within that harness's arc); **sibling separation** (no two same-parent children within min gap on the same shell; >ring-capacity children land on a second shell); **honesty preserved** (flat/unknown solo; no fabricated links). Keep `radarHonesty.test.ts`.
- [ ] **Step 2:** `npx vitest run src/viz/radarLayout*.test.ts src/viz/radarHonesty.test.ts` → FAIL on new assertions, honesty still PASS.
- [ ] **Step 3:** Extend `radarLayout.ts` per the algorithm.
- [ ] **Step 4:** Run tests → PASS (new + honesty).
- [ ] **Step 5:** Commit `feat(viz): radar harness sectors + multi-shell sibling spacing`.

---

## Task 6: Persistent window (Rust)

**Files:**
- Modify: `src-tauri/tauri.conf.json`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`

**Interfaces:**
- Produces (IPC, parameterless): `#[tauri::command] minimize_window`, `#[tauri::command] hide_window` — registered in the invoke handler. Frontend (Task 10) calls `invoke('minimize_window')` / `invoke('hide_window')`.

**Changes:**
1. `tauri.conf.json` overlay window: `resizable: true`, add `minWidth/minHeight` (≈ 760×560), `alwaysOnTop: false`, `skipTaskbar: false`, `focus: true`. Keep `transparent: true`, `decorations: false`, `center: true`.
2. `lib.rs`: set `ActivationPolicy::Regular` (dock icon); **remove the blur → `dismiss_overlay()` path** and the **click-through-idle** toggling (`set_ignore_cursor_events`); on summon, show + focus (no longer pre-warm click-through). Keep ⌘⇧Space toggle and tray menu. (Esc is now handled in the web layer for camera back-out — ensure Rust no longer hides on Esc.)
3. `commands.rs`: add `minimize_window` (`window.minimize()`) and `hide_window` (`window.hide()` — daemon keeps running, re-summonable). Register both.

**Implementer note (Opus): you may spawn Haiku sub-agents to locate the blur handler, click-through calls, ActivationPolicy line, the invoke_handler registration list, and Esc handling in `lib.rs`/`commands.rs` before editing. This touches the daemon lifecycle — change carefully.**

- [ ] **Step 1:** Read the current overlay lifecycle (delegate the search). Identify: blur handler, click-through calls, ActivationPolicy, Esc/dismiss, invoke_handler list.
- [ ] **Step 2:** Apply the config + lib.rs + commands.rs changes above.
- [ ] **Step 3:** Run `cd src-tauri && cargo check` → builds; then `cargo test` → existing tests PASS.
- [ ] **Step 4:** Commit `feat(window): persistent app window (no blur-dismiss, minimize/hide, dock)`.

---

## Task 7: Animate dim (color-only) in `Orb.tsx` + `RadarConstellation.tsx`

**Files:**
- Modify: `src/viz/Orb.tsx`, `src/viz/RadarConstellation.tsx`

**Interfaces:**
- Consumes: a per-orb `dimTarget: number` (0..1) prop (computed in Task 9 via `emphasis.targetDim`). Each orb keeps a ref of its *current* dim and eases it toward `dimTarget` in `useFrame` (~300ms time-constant), applying it to **color only** (reuse the existing `dimmed` color lerp; replace the boolean dim with the eased float). Must NOT affect scale/position or the select/hover boost.

**Implementer note (Opus): you may spawn Haiku sub-agents to read how `dimmed` currently feeds the shell/inner/gem color in `Orb.tsx:81-93` and `RadarConstellation.tsx:107-121` before editing.**

- [ ] **Step 1:** Replace the boolean `dimmed` color path with an eased float `dim` (ref-stepped in `useFrame`), driven by `dimTarget` prop; color lerps by the float, everything else unchanged.
- [ ] **Step 2:** Manual smoke in the dev harness later (Task 12); for now ensure `pnpm build` types pass for these two files (run after Task 9 wires the prop — see phase note). Within this task, verify no type errors in isolation by temporarily defaulting `dimTarget = 0`.
- [ ] **Step 3:** Commit `feat(viz): smooth color-only dim crossfade on orbs`.

---

## Task 8: Tame zoom + fly-to framing (`CameraRig.tsx`)

**Files:**
- Modify: `src/viz/CameraRig.tsx`

**Interfaces:**
- Consumes: `Bounds` (from `cameraFraming.ts`); new props `focusBounds: Bounds | null` (fly to frame it; null → ease back to overview) and existing selection. Uses `frameDistance` for target distance.
- Behavior: raise `minDistance` (≈5); add **FOV taper** (ease perspective FOV 46°→~38° as distance approaches min) to kill fisheye; enable **constrained pan** (clamp `target` to scene bounds); fly-to eases camera + target over ~700ms expo, preserving angle; back-out restores overview pose.

**Implementer note (Opus): you may spawn Haiku sub-agents to read the current `OrbitControls` config + focus-in/out pose logic in `CameraRig.tsx` before editing.**

- [ ] **Step 1:** Raise `minDistance`, add FOV taper (lerp `camera.fov` by distance, `updateProjectionMatrix()` each frame it changes), enable+clamp pan.
- [ ] **Step 2:** Add `focusBounds` handling: on change, compute `frameDistance` and ease to frame; null → ease to overview pose.
- [ ] **Step 3:** Ensure `pnpm build` types pass for this file (default `focusBounds = null` until Task 9 wires it).
- [ ] **Step 4:** Commit `feat(viz): tamed zoom + cinematic fly-to framing`.

---

## Task 9: Wire state in `WarRoom.tsx` (orchestrator-owned)

**Files:**
- Modify: `src/viz/WarRoom.tsx`

**Interfaces:**
- Consumes: `emphasis.targetDim`, `cameraFraming.subtreeBounds`, the radar layout `positions` map.
- Produces (props to children): per-node `dimTarget` (to forests → Orb/RadarConstellation), `focusBounds` (to CameraRig), `emphasisFilter` + `onFilter` (to chrome legend), `focusStack` + `onCrumb`/`onClear` (to chrome breadcrumb).
- State added: `emphasisFilter: EmphasisFilter` (null default); `focusStack: string[]` (agent ids root→deep). Selecting an agent pushes; `focusBounds = subtreeBounds(positions, agents, top(focusStack))`; clearing → null. Esc clears the deepest crumb (camera back-out).

- [ ] **Step 1:** Add `emphasisFilter` + `focusStack` state; compute `dimTarget` per node from the active filter; compute `focusBounds` from the focus stack.
- [ ] **Step 2:** Thread the new props into the forests, CameraRig, and chrome; severity filter applies on Habits, harness filter on both tabs.
- [ ] **Step 3:** `pnpm build` → tsc + vite PASS.
- [ ] **Step 4:** Commit `feat(viz): wire emphasis filter + focus stack in WarRoom`.

---

## Task 10: Interactive legend + breadcrumb + window controls (`chrome.tsx`)

**Files:**
- Modify: `src/viz/chrome.tsx`

**Interfaces:**
- Consumes: `emphasisFilter`, `onFilter(filter)`, `focusStack`, `onCrumb(index)`, `onClear` (from WarRoom); `harnessColor` (for chip color/glyph/label).
- Produces: legend chips (severity buckets + harness) calling `onFilter` (toggle single active filter); breadcrumb `Overview › … ` calling `onCrumb`/`onClear`; window controls calling `invoke('minimize_window')` / `invoke('hide_window')`; nav bar gets `data-tauri-drag-region`.

- [ ] **Step 1:** Make legend entries clickable chips with active ring + `aria-pressed`; clicking toggles the matching `EmphasisFilter` (severity buckets only on Habits tab; harness on both).
- [ ] **Step 2:** Add the breadcrumb (renders from `focusStack`) and the minimize/close controls (invoke the Task 6 commands); add `data-tauri-drag-region` to the nav bar.
- [ ] **Step 3:** `pnpm build` → PASS.
- [ ] **Step 4:** Commit `feat(viz): interactive legend filter + breadcrumb + window controls`.

---

## Task 11: Chrome layout system (`style.css`)

**Files:**
- Modify: `src/style.css`

**Changes:**
- Layout tokens: `--top-safe` (nav height + margin), `--side-margin`, `--panel-w: clamp(300px, 32vw, 380px)`; z-index scale `--z-canvas:0; --z-chrome:10; --z-panel:20; --z-deck:30; --z-nav:40; --z-controls:50`.
- `.wd-inspector` / `.wd-radar-dock`: `top: var(--top-safe)` (clears nav bar) and z tokens.
- Legend chip styles (active ring, hover, focus ring) and window-control button styles + drag-region cursor.
- Extend responsive rules so chrome stays proportional across the new resizable range (min 760×560 → large); no overlap at min size.

- [ ] **Step 1:** Add tokens + z-scale; repoint panels to `--top-safe`; add chip + control styles; extend media queries.
- [ ] **Step 2:** `pnpm build` → PASS.
- [ ] **Step 3:** Commit `feat(viz): proportional chrome layout system + z-index scale`.

---

## Task 12: Full verification + PROGRESS/memory

- [ ] **Step 1:** `pnpm build` (tsc + vite) → PASS; `cd src-tauri && cargo test` → PASS; `cargo check` clean.
- [ ] **Step 2:** Run all viz unit tests: `npx vitest run src/viz` → PASS.
- [ ] **Step 3:** Launch the dev harness/preview; with a high-count habits fixture and the 12+ radar fixture verify: (a) no globe overlap at scale; (b) legend chip dims color-only with smooth crossfade, nothing moves; (c) zoom shows no fisheye, click frames subtree, breadcrumb/Esc returns; (d) window persists on blur, minimize + close work, chrome proportional on resize. Capture screenshots as evidence.
- [ ] **Step 4:** Validate harness-color contrast on `--bg`; if `#ff8c42`/`#b98cff` read poorly, adjust in `harnessColors.ts` and update `CLAUDE.md`'s convention note to the chosen values.
- [ ] **Step 5:** Update `PROGRESS.md` + memory (`warden-face-visual-pass`); commit `chore(M3): verify + progress`.

---

## Self-Review (against spec)
- **Coverage:** A→Tasks 4,1; B→Tasks 2,7,9,10; C→Tasks 3,5,8,9,10; D→Tasks 6,10,11. ✓
- **Type consistency:** `EmphasisFilter`/`targetDim` (T2) consumed by T7/T9; `Bounds`/`frameDistance`/`subtreeBounds` (T3) consumed by T8/T9; `harnessColor` (T1) consumed by T10; `minimize_window`/`hide_window` (T6) consumed by T10. ✓
- **Honesty:** preserved in T5; `radarHonesty.test.ts` kept green. ✓
- **No placeholders:** greenfield modules carry full contracts + test specs; layout/component tasks carry test-as-contract + algorithm + read-current-file notes (implementer reads live internals rather than fabricated line-exact code). ✓
