# M3 RADAR / Habits — Constellation Readability, Filtering & Navigation Redesign

- **Status:** Approved design (pre-plan)
- **Date:** 2026-06-24
- **Branch:** `m3-radar`
- **Author:** Artist agent (design lead)
- **Milestone:** M3 — RADAR (FACE polish). Preview-only; no writes to user projects.

---

## 1. Problem & Goals

The FACE renders two R3F constellations — **Habits** (agents and their diagnosed issues) and
**Radar** (live agents and their subagent hierarchy). Both are honest visualizations (nodes/flares
map to real backend signals). At scale they fail on readability and navigation, and the host window
fights its own UI. This redesign makes the data **decodable**, the legend **interactive**, the radar
**navigable**, and the window a **persistent application**.

### Confirmed direction (user-approved)
1. **Habits layout grammar:** *Harness zones + severity.* Position encodes meaning.
2. **Radar navigation:** *Cinematic focus-orbit.* Tamed zoom, fly-to framing, breadcrumb back-out.
3. **Window behavior:** *Persistent application.* Does not vanish on blur; stays until minimized or
   closed; resizable; hotkey still summons. Keep it simple — no resize presets in v1.

### Non-goals
- No new backend signals. Everything below is driven by **already-emitted** data
  (`RadarAgent`, `OrbIssue`/`OrbAgent`). Honest-viz rules are preserved, not bypassed.
- No M4+ features (apply/revert/voice/screen/fleet remain stubbed).
- No change to the Fugu pipeline, ingestion, or store.
- No resize presets, no minimap, no free-fly camera (explicitly deferred / rejected in brainstorming).

### Success criteria
- With 15+ radar agents or many habits/issues, **no two globes visually overlap** in the default
  framing, and a viewer can state *which harness / which agent / what severity* a globe is from.
- Clicking a legend chip emphasizes matching globes and dims the rest **in color only** (no motion,
  no resize), with a smooth crossfade.
- Manual zoom never produces wide-angle distortion; clicking a node smoothly frames it and its
  subtree; a breadcrumb returns to overview.
- The window stays open when focus is lost; minimize and close work; the window is resizable and the
  chrome (nav bar, legend, inspector, status deck) stays proportional at any size with no overlap.
- `pnpm build` and `cd src-tauri && cargo test` pass; pure layout/filter/framing helpers are unit-tested.

---

## 2. Architecture overview

The viz uses **props-drilling from `WarRoom.tsx`** (no global store) and **pure, unit-tested layout
functions**. We keep that shape. New logic lands as **small pure modules** with one responsibility,
imported by the React components that already exist.

### New / changed units

| Unit | Kind | Responsibility | Consumed by |
|---|---|---|---|
| `src/viz/harnessColors.ts` | **new** pure | Single source of truth: harness → `{hue, glyph, label}` | `harnessTheme.ts`, `radarTheme.ts`, legend |
| `src/viz/orbLayout.ts` | **rewrite** pure | Habits: harness-zone packing + severity fan, collision-free | `WarRoom.tsx`, `Constellation.tsx` |
| `src/viz/emphasis.ts` | **new** pure | `EmphasisFilter` type + `matchesFilter(node, filter)` + `targetDim(...)` | `WarRoom.tsx`, `Orb.tsx`, `RadarConstellation.tsx` |
| `src/viz/cameraFraming.ts` | **new** pure | `subtreeBounds(agents, rootId)`, `frameDistance(radius, fov)` | `CameraRig.tsx` |
| `src/viz/radarLayout.ts` | **extend** pure | Harness sectors, multi-shell siblings, subtree-scaled root ring | `WarRoom.tsx`, `RadarConstellation.tsx` |
| `src/viz/CameraRig.tsx` | extend | Tame zoom (min-dist + FOV taper), fly-to framing, focus stack, constrained pan | — |
| `src/viz/WarRoom.tsx` | extend | `emphasisFilter` + `focusStack` state; per-node dim; render legend/breadcrumb/window-controls | — |
| `src/viz/chrome.tsx` | extend | Interactive legend chips; breadcrumb; window controls | — |
| `src/viz/Orb.tsx` / `RadarConstellation.tsx` | extend | Animate dim smoothly (ref-stepped lerp), color-only | — |
| `src/style.css` | extend | Layout tokens, z-index scale, `--top-safe`, responsive clamps, chip/control styles | — |
| `src-tauri/tauri.conf.json` | edit | `resizable`, `minSize`, `alwaysOnTop`, `skipTaskbar`, `focus` | — |
| `src-tauri/src/lib.rs` | edit | `ActivationPolicy::Regular`; remove blur-dismiss + click-through-idle; focus on summon | — |
| `src-tauri/src/commands.rs` | edit | `minimize_window`, `hide_window` commands | `chrome.tsx` |

**Principle:** every layout/filter/framing decision is a pure function returning data; React
components render that data and own only animation refs. This keeps units independently testable and
keeps the render path free of branching logic.

---

## 3. Workstream A — Habits: harness zones + severity

### 3.1 Current behavior (`src/viz/orbLayout.ts`)
Agents are sorted worst-first and laid out **left→right in a straight line** (`x += extent + GAP`,
`GAP = 2.4`); each agent's issues sit on a **Fibonacci sphere shell** of radius `~√N`. There is **no
global de-collision** and **no harness grouping**. In perspective, the receding line of clusters
collapses front-to-back into an unreadable tangle.

### 3.2 New grammar
Position is decodable as **zone → agent → severity**:

1. **Harness zones.** Partition agents by `harness` into zones: `claude_code`, `codex`, and a small
   `unknown` neutral zone (only if present). Each zone is a labeled spatial region.
2. **Zones on a shallow camera-facing arc.** Zone centers are placed along a gentle arc (lateral X
   spread, mild Z curvature toward the camera) — **not** a straight line receding in Z. This is the
   fix for perspective collapse: clusters spread sideways and never sit directly behind one another.
3. **Collision-free agent packing within a zone.** Hubs are placed by golden-angle phyllotaxis sized
   so each hub's *footprint* = `hubRadius + issueShellRadius + margin`, followed by a bounded
   pairwise **separation relaxation** (≤ N iterations, clamped displacement) guaranteeing no two
   footprints overlap. Deterministic (no RNG) so the layout is stable and testable.
4. **Issues fan by severity.** Keep the severity→latitude mapping (crimson/critical toward the
   front-top, calm severities trailing), but compute shell radius to enforce a **minimum angular gap**
   between issues so a high-count agent fans out instead of bunching on the surface.

### 3.3 Output contract (unchanged)
`layoutOrbs(model: OrbSceneModel): OrbLayout` still returns `{ nodes, links }` where nodes are hubs +
issues and links are hub→issue tethers. `Constellation.tsx` (bezier tethers + intra-cluster MST)
consumes the same shape — only positions change, so tethers automatically read as clean radial spokes.

### 3.4 Internal helpers (pure)
- `groupByHarness(agents, issues)` → zones
- `placeZones(zones)` → zone centers on the arc
- `packZone(zone)` → non-overlapping hub positions (phyllotaxis + separation pass)
- `fanIssues(hub, issues)` → issue positions on the severity shell with min-gap

### 3.5 Tests (`src/viz/orbLayout.test.ts`, extend)
- **(kept)** severity → latitude ordering holds.
- **(new)** *no-overlap invariant:* for every pair of nodes, center distance ≥ r₁ + r₂ (with margin).
- **(new)** *zone grouping:* all hubs of one harness fall inside that zone's bounds; zones do not overlap.
- **(new)** *determinism:* two runs on the same model produce identical positions.
- **(new)** *scale:* runs clean at 30 agents × up to ~12 issues each.

---

## 4. Workstream B — Interactive legend filter (both tabs)

### 4.1 Behavior
The bottom-left legend (`.wd-legend`, currently static DOM) becomes a row of **clickable chips**:
- Severity buckets: `low → med → high → critical` (Habits only — issues carry `severity`).
- Harness: `Claude`, `Codex` (+ `unknown` if present) — applies to **both** tabs.

Clicking a chip sets a **single active filter** (click again toggles off). Matching globes hold full
color/brightness; non-matching globes **desaturate + dim — color only.** Nothing moves, scales, or
re-layouts. Transition is a **~300 ms eased crossfade**. The active chip shows a clear ring; every
chip pairs color with a **glyph + label** (color-blind a11y, per repo convention). Chips are
keyboard-focusable with `aria-pressed`.

### 4.2 Mechanism (reuse existing dim channel)
Globes already have a `dimmed` color path (used by hover: `Orb.tsx` lerps shell color toward dim by
×0.42; `RadarConstellation.tsx` mirrors it). We:
1. Add `emphasisFilter` state to `WarRoom.tsx`.
2. Compute a **target dim factor per node** via pure `emphasis.ts`:
   `targetDim(node, filter)` → `0` (full) when matching or no filter, `1` (dim) otherwise.
3. Animate each node's *actual* dim toward its target inside `useFrame` (ref-stepped lerp), so the
   change crossfades smoothly instead of snapping. Dim affects **only color saturation/brightness** —
   never scale, position, or the selected/hover boost (those remain independent).

### 4.3 Tests
- `emphasis.test.ts` (new): `matchesFilter` truth table for severity buckets and harness across
  Claude/Codex/unknown nodes; `targetDim` returns full for the active set and dim otherwise; a null
  filter yields full for all.

---

## 5. Workstream C — Radar: focus-orbit navigation + organization

### 5.1 Navigation (`src/viz/CameraRig.tsx`)
Current: drei `OrbitControls`, dolly zoom to `minDistance = 3` against fixed `fov = 46°` →
wide-angle (fisheye) distortion on close zoom; `enablePan = false`.

Changes:
1. **Tame the fisheye.** Raise `minDistance` (≈ 5) and apply a **subtle FOV taper**: as camera
   distance approaches the minimum, ease FOV from 46° toward ~38°, counteracting wide-angle
   exaggeration. Orbit stays damped (`dampingFactor` unchanged).
2. **Fly-to framing.** On node select, compute the **bounding sphere of the node + its subtree** via
   `cameraFraming.subtreeBounds`, then `frameDistance(radius, fov)` for a distance that fits the
   subtree; ease camera + target over ~700 ms (expo in-out), preserving the viewing angle (extends the
   existing focus-in pose capture).
3. **Breadcrumb + back-out.** A `focusStack` (`Overview › Agent › Subagent`) lives in `WarRoom.tsx`.
   Selecting pushes; clicking a crumb or **Esc** pops and eases back; "Overview" pops to root. (Esc is
   now a camera control, **not** a window-dismiss — see §6.)
4. **Constrained pan.** Enable `OrbitControls` pan but clamp `target` within scene bounds so roaming
   can't lose the constellation.

### 5.2 Organization (`src/viz/radarLayout.ts`)
Current: all roots on **one ring**; each parent's children share **one orbit** (`parentR + 1.5`),
shrinking with depth → siblings overlap; deep subagents bunch.

Changes (honest-viz preserved — flat agents stay solo, unknown stays neutral):
1. **Harness sectors for roots.** Partition roots by harness; assign each harness an **angular arc**
   on the root ring and place its roots within that arc (mirrors the legend filter spatially).
2. **Sibling de-overlap via multiple shells.** When a parent's `childCount` exceeds what one ring
   holds with a minimum angular gap, distribute children across **2+ concentric orbital shells** with
   a small per-child radial stagger, instead of cramming a single ring.
3. **Subtree-scaled root spacing.** Root ring radius / arc allocation scales with each root's
   **subtree extent** (descendant count), so a busy orchestrator gets proportionally more room.
4. Size = `contextTokens` occupancy, color = harness heat (`fillPct`), status = lifecycle scale — all
   retained.

`subtreeBounds` (in `cameraFraming.ts`) is shared between layout-extent reasoning and camera framing.

### 5.3 Tests
- `radarLayout` (extend / new spec): *sector grouping* (roots of a harness fall in that arc);
  *sibling separation* (no two siblings within min gap on the same shell); *honesty preserved* (flat
  / `codex_vscode` agents remain solo roots; unknown harness neutral) — keep `radarHonesty.test.ts`
  green.
- `cameraFraming.test.ts` (new): `frameDistance` monotonic in radius and correct at known fov;
  `subtreeBounds` encloses all descendants of a fixture forest.

---

## 6. Workstream D — Persistent window + proportion system

### 6.1 Window model (`tauri.conf.json`, `lib.rs`, `commands.rs`)
Today the overlay is a hotkey-summoned HUD: `Accessory` activation policy, `alwaysOnTop`,
`skipTaskbar`, `resizable:false`, click-through when idle, and **dismiss on blur / Esc**. We make it a
**persistent application window**:

- **No blur-dismiss.** Remove the focus-loss → `dismiss_overlay()` path. Clicking elsewhere keeps it open.
- **Stays until minimized or closed.** Add window controls (custom, matching the phosphor aesthetic,
  since `decorations:false` is kept for the frameless look):
  - **Minimize** → `window.minimize()`.
  - **Close (X)** → `window.hide()` — the daemon keeps watching; tray + ⌘⇧Space re-summon. (Closing
    hides rather than quits, preserving the always-watching daemon nature.)
  - The nav bar gets a `data-tauri-drag-region` so the frameless window can be moved.
- **Dock presence so minimize has a home:** `ActivationPolicy::Regular`, `skipTaskbar:false`.
- **`alwaysOnTop:false`** by default (true "general application" stacking).
- **`resizable:true`** with a sane `minSize` (≈ 760×560); window keeps `transparent:true`.
- **Summon focuses** (`focus:true` on summon); remove the click-through-idle toggling — a real window
  is interactive whenever visible.
- **Esc** is repurposed to the camera back-out (§5.1), no longer hides the window.
- ⌘⇧Space still toggles visibility (summon / hide), and the tray menu is retained.

New commands invoked from `chrome.tsx`: `minimize_window`, `hide_window`.

### 6.2 Chrome layout system (`src/style.css`, `chrome.tsx`)
Root cause of the nav/panel conflict: nav bar at `top:16px` (~48px tall, z-7) and the inspector at
`top:20px` → a 4px tuck and backdrop-blur bleed. Fix with **one layout system**:

- **CSS layout tokens:** `--top-safe` (= nav height + margin), `--side-margin`, `--panel-w`
  (`clamp(300px, 32vw, 380px)`), and a **z-index scale**:
  `--z-canvas:0; --z-chrome:10; --z-panel:20; --z-deck:30; --z-nav:40; --z-controls:50`.
- **Panels open below the bar:** `.wd-inspector` / `.wd-radar-dock` use `top: var(--top-safe)` and the
  z-scale tokens — never under the nav bar.
- **Proportional & responsive at any window size** (the window is now resizable): widths via
  `clamp()`, legend repositions, nav bar/deck scale; extend the existing `max-width:760/820px` media
  queries to cover the new resizable range so nothing overlaps when small.
- **Window controls + breadcrumb** rendered in the chrome with their own z token; controls and drag
  region don't intercept canvas interaction outside their bounds.

### 6.3 Harness color — single source of truth
Harness colors are currently defined **three ways**: `harnessTheme.ts` (Habits) uses coral/teal,
`radarTheme.ts` (Radar) uses orange/violet, and `CLAUDE.md` documents emerald/violet. Consolidate into
`src/viz/harnessColors.ts` (one `{hue, glyph, label}` per harness) imported by both themes and the
legend, so zones, legend chips, and filter all agree. Choose the unified hues to (a) be mutually
distinct, (b) contrast against the green-phosphor background, (c) match the legend; validate contrast
on the real background. Update `CLAUDE.md`'s convention note to match the chosen values.

---

## 7. Data flow (unchanged contracts)

```
Rust radar watcher ──app.emit("radar_state")──▶ main.ts ──bridge──▶ WarRoom.tsx
WarRoom.tsx ──invoke("get_radar_state" / "query_profile" / "get_findings")──▶ Rust
WarRoom.tsx state: { scene, tab, selectedId, hoveredId, emphasisFilter*, focusStack* }   (* new)
   ├─ radarLayout(scene)        → RadarConstellation  (positions, links)
   ├─ orbLayout(model)          → Constellation/Orb   (positions, links)
   ├─ emphasis.targetDim(node)  → per-node dim         (animated in useFrame)
   └─ focusStack/selected       → CameraRig            (fly-to framing)
chrome.tsx: legend chips → set emphasisFilter ; breadcrumb → pop focusStack ;
            window controls → invoke minimize_window / hide_window
```

No new IPC payloads; `minimize_window` / `hide_window` are parameterless commands.

---

## 8. Testing & verification strategy

- **Pure units (Vitest):** `orbLayout.test.ts` (extended), `emphasis.test.ts`, `cameraFraming.test.ts`,
  radar layout sector/sibling specs; `radarHonesty.test.ts` stays green.
- **Build gates:** `pnpm build` (tsc + vite) after frontend work; `cd src-tauri && cargo check` then
  `cargo test` after window changes.
- **Honest-viz regression:** confirm flat/unknown agents still render solo + neutral; estimated-null
  still shows "—"; no fabricated signals.
- **Manual/preview proof (per pass):** use the existing dev harness (`src/viz/preview/radarReal.tsx`
  + `realRadar.json`, and a high-count habits fixture) in the browser preview to verify: (a) no globe
  overlap at scale, (b) legend filter dims color-only with smooth crossfade, (c) zoom shows no
  fisheye, fly-to frames subtree, breadcrumb returns, (d) window persists on blur, minimize/close
  work, chrome stays proportional on resize. Capture screenshots as evidence before claiming done.

---

## 9. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Window-model change touches daemon lifecycle (lib.rs) and could break summon/hotkey | Change config + policy incrementally; `cargo check`/`cargo test`; manually verify summon → persist → minimize → close → re-summon |
| Harness recolor is user-visible and emerald may be low-contrast on green phosphor | Centralize first, then pick/validate hues against the real bg; update `CLAUDE.md` to the chosen truth |
| Layout rewrite could regress tethers/MST | Keep `OrbLayout` output contract identical; only positions change; layout unit tests + preview check |
| Separation relaxation could be non-deterministic or slow | Deterministic, bounded iterations, no RNG; covered by determinism + scale tests |
| Resizable window breaks chrome proportions | `clamp()`-based tokens + extended media queries; verify at min and large sizes in preview |

---

## 10. Out of scope / deferred
- Resize presets, minimap, free-fly camera, multi-dimension simultaneous filters (single active
  filter in v1).
- Any backend signal additions, M4+ features, writes to user projects.
