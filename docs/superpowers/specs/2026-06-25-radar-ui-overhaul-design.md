# Radar UI Overhaul — Toggle Roster Sidebar, Bottom Filter Bar, Status-Deck Removal

- **Status:** Approved design (pre-plan)
- **Date:** 2026-06-25
- **Branch:** `radar-visual-overhaul`
- **Author:** Design pairing with Karim
- **Milestone:** M3 — RADAR (FACE polish). Preview-only; no writes to user projects. **Frontend-only — zero Rust/backend changes.**

---

## 1. Problem & Goals

The war-room renders two R3F constellations chosen by the top nav: **Habits** (diagnosed
anti-pattern orbs) and **Radar** (the live agent forest). Two problems at scale:

1. **No list view.** With 20–25 live agents the only way to reach a globe is to find it in 3D
   space. There is no scannable roster, no "is this one working?" at a glance, no jump-to.
2. **The bottom bar spends prime space on read-only telemetry.** The full-width `StatusDeck`
   (habits · agents · sessions · events · findings · watching pulse) is glanceable but not a
   control; meanwhile the *interactive* severity/harness filter (`Legend`) sits cramped above it.

### Confirmed direction (user-approved)
1. **Keep the top nav** (`Habits | Radar`) exactly as-is.
2. **Remove the bottom `StatusDeck`**; **extract the severity + harness filter into its own
   `FilterBar`**, centered bottom-middle (its old home).
3. **Add a left roster sidebar**, **closed by default**, opened by a `≡` button top-left:
   - **Radar:** agents grouped by harness (Claude ◆ / Codex ▣), root agents listed with their
     subagents **nested/indented** beneath them (`parentId`/`depth`).
   - **Habits:** habit orbs grouped by harness, each section **titled** by the harness.
   - Clicking a row reuses the **existing** select → camera-dive → detail-dock flow.
4. **Retire** the lost telemetry (sessions/events/findings). The sidebar header carries a live
   **`N agents · M working`** count instead; the nav keeps its per-tab badges.

### Non-goals
- No Rust/IPC/`radar_state` changes; no new backend signals.
- No change to the 3D forest layout, camera rig, globe rendering, or the tab-fold transition.
- No chat/ask-bar work — `Hud`/`Console`/`EmptyState` stay defined-but-dormant as today.
- No roster search, no drag-resize, no persisting the open/closed state to disk (session-local only).

### Success criteria
- `StatusDeck` and all `.wd-deck*` CSS are gone; no dangling references.
- `FilterBar` renders centered bottom-middle: severity chips on **Habits** only, harness chips on
  **both** tabs; toggling a chip emphasizes matching globes via the **existing** `emphasisFilter`
  dim/emphasis channel (behavior unchanged — new home only).
- `≡` toggles the left sidebar (default closed). A 25-agent roster scrolls; **working** rows pulse.
- Clicking a roster row selects that agent/habit → camera dives (existing `focusStack` → `CameraRig`)
  → the right detail dock opens. The selected row is marked.
- `pnpm build` (tsc + vite) clean; `pnpm test` green including the new helper + component tests.
- **Honest-viz preserved:** every row maps to a real agent/issue; `closed`/`terminated` render faded,
  never fabricated; an empty harness section simply does not appear.

## 2. Architecture overview

The DOM overlay layer over the single persistent `<Canvas>` becomes **four sibling docks**, each a
pure consumer of WarRoom's single sources of truth (`selectedId`, `displayTab`, `emphasisFilter`):

| Dock | Position | Component | Status |
|---|---|---|---|
| Nav | top-center | `NavBar.tsx` | unchanged |
| Filter | bottom-center | `FilterBar.tsx` | **new** (extracted from `chrome.tsx`) |
| Roster | left (toggle) | `Sidebar.tsx` | **new** |
| Detail | right | `RadarDetailPanel` (radar) / `Chrome` inspector (habits) | existing |

WarRoom gains one piece of state (`sidebarOpen`) and one toggle; everything else **reuses wiring that
already exists** (`onRadarJump` = `setSelectedId`, which already drives camera + panel).

### New / changed units
- **NEW `src/viz/rosterTree.ts`** (pure, no React/Three): `buildRadarRoster(agents)` and
  `buildHabitsRoster(layout)` → `HarnessGroup[]`. Unit-tested in node.
- **NEW `src/viz/Sidebar.tsx`**: presentational left dock; renders harness groups + rows; calls
  `onPick(id)` / `onToggle`.
- **NEW `src/viz/FilterBar.tsx`**: the `Legend` (+ `SEVERITY_CHIPS`, `isSeverityActive`,
  `isHarnessActive`) moved out of `chrome.tsx` **with its logic unchanged**.
- **CHANGED `src/viz/chrome.tsx`**: delete `StatusDeck` + `DeckStat`; remove `Legend` (moved). `Chrome`
  now renders `Breadcrumb` (radar) + the inspector only. Its `emphasisFilter`/`onFilter` props drop.
  Remove any helper (`compact`, etc.) left dead by the deletion.
- **CHANGED `src/viz/WarRoom.tsx`**: add `sidebarOpen` state + `onToggleSidebar`; render `<FilterBar>`,
  `<Sidebar>`, and the `≡` button; build rosters with `useMemo`; wire `onPick`.
- **CHANGED `src/style.css`**: delete `.wd-deck*`; add `.wd-sidebar*`, `.wd-roster*`, `.wd-side-toggle`,
  a status-dot scale + `@keyframes wd-status-pulse`; rename/recenter `.wd-legend` → `.wd-filterbar`.

## 3. Workstream A — Remove the bottom bar (`StatusDeck`)

- `chrome.tsx`: delete the `StatusDeck` and `DeckStat` components and the `<StatusDeck … />` call.
- `style.css`: remove `.wd-deck`, `.wd-deck-group`, `.wd-deck-stats`, `.wd-deck-live`, `.wd-deck-stat*`,
  `.wd-deck-div`, `.wd-deck-pulse`, `.wd-deck-phase` rules.
- `scene.profile`-derived figures (sessions/events/findings) are no longer surfaced — accepted per the
  user decision. `deriveFindings` in `WarRoom.tsx` is still consumed by the `reveal` PlayerHost, so it
  stays; only the deck readout is removed.
- **Tests:** none new (deletion). Gate: `pnpm build` clean, grep shows no `wd-deck` / `StatusDeck` refs.

## 4. Workstream B — `FilterBar` (extract + recenter)

### 4.1 Extraction
Move `Legend`, `SEVERITY_CHIPS`, `isSeverityActive`, `isHarnessActive` verbatim from `chrome.tsx` into
`FilterBar.tsx`. Export `function FilterBar({ tab, model, filter, onFilter })`. Logic is unchanged: the
severity group renders only when `tab === 'habits'`; harness chips render on both tabs; chips read the
real snake_case harness id so `matchesFilter` lines up with scene nodes.

### 4.2 Wiring
`WarRoom` renders `<FilterBar tab={tab} model={chromeModel} filter={emphasisFilter} onFilter={onFilter} />`
as a bottom-center sibling (using the same `chromeModel` already computed per tab). `onFilter` now flows
**WarRoom → FilterBar** directly; `Chrome` no longer receives `emphasisFilter`/`onFilter`.

### 4.3 Style
`.wd-filterbar`: absolutely positioned bottom-center (`left:50%; transform:translateX(-50%)`), glass pill
(`--panel-strong`, `--hair`), `pointer-events:auto` only on its buttons, small `FILTER` kicker, wraps on
narrow widths, sits at the `--z-deck` band so it clears the canvas but stays under the nav.

### 4.4 Tests
`FilterBar.test.tsx` (jsdom): severity chips present on `habits` and absent on `radar`; harness chips on
both; clicking a chip calls `onFilter` with the expected `EmphasisFilter` and clicking the lit chip
clears it. Existing `emphasis.test.ts` (`targetDim`/`matchesFilter`) already covers the dim math.

## 5. Workstream C — Roster sidebar (new)

### 5.1 Data (`rosterTree.ts`, pure)
```
type RosterRow = {
  id: string;          // radar: agent.id · habits: layout node.id (selection-safe)
  title: string;       // radar: nickname ?? label · habits: issue.title
  subtitle: string | null;
  harness: string;
  depth: number;       // indent level (radar nesting); habits = 0
  status: RadarStatus; // radar: real · habits: 'idle' (no liveness)
  tone: string;        // dot color — radar: status color · habits: severityColor(sev)
};
type HarnessGroup = { harness: string; label: string; glyph: string; color: string; rows: RosterRow[] };
```
- `buildRadarRoster(agents: RadarAgent[]): HarnessGroup[]` — group by harness in fixed order
  (`claude_code`, `codex`, then any other present, `unknown` last). Within a group: order **roots**
  (`parentId === null`) by status (working → idle → closed → terminated) then `label`; under each root,
  append its descendants via **DFS in `depth` order** so subagents render indented. `subtitle =
  radarSubtitle(agent)`. `tone` from the status convention (working `--acid`, idle `--ink-faint`,
  closed/terminated `--amber`).
- `buildHabitsRoster(layout: OrbLayout): HarnessGroup[]` — read **layout nodes** (`kind === 'issue'`),
  **not** raw `model.issues`, so each `row.id === node.id` and a click selects the exact node the forest
  rendered. Group by `node.harness`; rows sorted by `issue.severity` desc then `issue.count`; `title =
  issue.title`; `subtitle = ×{count} · sev {severity}/5`; `tone = severityColor(severity)`.
- Counts for the header are derived by the caller: radar `agents.length` / working count; habits total
  rows.

### 5.2 Component (`Sidebar.tsx`)
- Props: `{ open: boolean; displayTab: ConstellationTab; groups: HarnessGroup[]; headerCount: string;
  selectedId: string | null; onPick: (id: string) => void; onToggle: () => void }`.
- Renders `<aside className="wd-sidebar" data-open={open} aria-hidden={!open}>`: a header
  (`ROSTER`/`HABITS` + `headerCount`, plus a `✕` that calls `onToggle`), then each group as a titled
  section (glyph + label + row count), then rows as `<button>`s: status/severity dot (`.wd-roster-dot`,
  `+ is-working` pulse), `title`, `subtitle`, indented by `depth`. A row gets `.is-selected` +
  `aria-current` when `id === selectedId`. The body is the scroll container.
- a11y: each section `role="group"` with `aria-label`; each row `aria-label` includes the status/severity
  word so color is never the only signal (matches the repo's color-blind rule).

### 5.3 Wiring (`WarRoom.tsx`)
- `const [sidebarOpen, setSidebarOpen] = useState(false)`; `onToggleSidebar = () => setSidebarOpen(o => !o)`.
- `groups`/`headerCount` via `useMemo` keyed on `displayTab` + `radarModel` / `layout`:
  - radar → `buildRadarRoster(radarModel.agents)`, header `"{n} agents · {w} working"`.
  - habits → `buildHabitsRoster(layout)`, header `"{n} habits"`.
- `onPick(id)`: radar → `onRadarJump(id)` (existing `setSelectedId`); habits → `setSelectedId(id)`. Both
  already drive the camera (`focusStack` → `focusBounds` → `CameraRig`) and the right detail dock.
- Render order in the root: `<Canvas>` … `<NavBar>` `<button className="wd-side-toggle" aria-expanded>`
  `<Sidebar>` `<FilterBar>` `<Chrome>` (Breadcrumb+inspector) and the existing radar dock.

### 5.4 Style (`style.css`)
- `.wd-sidebar`: left dock (`top: var(--top-safe)`, `bottom` clearing the `.wd-filterbar`),
  width `clamp(220px, 24vw, 280px)`, `--panel-strong` glass, `--hair` border, `--z-panel`. Closed state
  (`[data-open="false"]`) slides out left + fades + `pointer-events:none` (mirrors `.wd-radar-dock`'s
  open/closed pattern so left/right docks feel symmetric).
- `.wd-side-toggle`: top-left `≡` button at `--z-controls`, placed clear of any window chrome.
- `.wd-roster-row` hover/`.is-selected`; `.wd-roster-dot` + `is-working` → `@keyframes wd-status-pulse`.

### 5.5 Tests
- `rosterTree.test.ts`: harness group order; subagent DFS nesting + depth; working-first root order;
  empty harness omitted; unknown → slate; habits rows sorted by severity; habits row ids equal node ids.
- `Sidebar.test.tsx` (jsdom): renders titled groups + nested rows; clicking a row calls `onPick(id)`;
  selected row marked; `aria-hidden`/`aria-expanded` reflect `open`.

## 6. Data flow (unchanged contracts)
`radar_state` → `bridge` → `scene.radarScene` → `radarModel` (unchanged). `Sidebar` and `FilterBar` are
pure consumers. `selectedId` remains the single selection source of truth; the camera and both detail
docks already key off it. **No new IPC, no new events, no new fields.**

## 7. Testing & verification strategy
- `pnpm build` (tsc + vite) — clean.
- `pnpm test` (vitest) — existing suites green; new `rosterTree.test.ts`, `Sidebar.test.tsx`,
  `FilterBar.test.tsx` green. Component tests use the `// @vitest-environment jsdom` pragma (same pattern
  as `RadarDetailPanel.test.tsx`).
- Live (`pnpm tauri dev`, manual — honest-viz can't be asserted in node): open Claude + Codex sessions →
  the roster lists them grouped by harness with subagents nested; working rows pulse; a row click dives
  the camera and opens the detail dock; filter chips emphasize matching globes; the bottom bar is gone.

## 8. Risks & mitigations
- **`≡` collides with window chrome (top-left).** Audit `chrome.tsx` / window-chrome top-left occupancy
  and place the toggle clear of it (`--z-controls`).
- **Sidebar overlaps the centered filter at short heights.** Sidebar `bottom` stops above the filter
  band; the filter is centered and narrower than the side gaps.
- **Habits node-id vs issue-id mismatch.** Avoided by building the habits roster from **layout nodes**
  (id-safe), never from raw `model.issues`.
- **25-agent perf.** Plain DOM list; no virtualization needed at this scale; memoize roster build.
- **Stale `CLAUDE.md` harness colors** (line ~85 says emerald/violet). `harnessColors.ts` is canonical
  (Claude `#ff8636` ◆ / Codex `#4fc9ff` ▣). Fixing the doc note is **out of scope** here — flag only.

## 9. Out of scope / deferred
Backend/IPC changes · new telemetry · chat/ask bar · Habits camera/layout redesign · roster search or
in-list filtering · drag-resize of the sidebar · persisting sidebar open-state to disk · fixing the stale
`CLAUDE.md` color note.
