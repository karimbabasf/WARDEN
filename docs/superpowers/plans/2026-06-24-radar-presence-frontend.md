# RADAR Presence Polish — Frontend Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Open maximized, keep animating off-focus and pause only on minimize, be draggable like a native macOS window, implode terminated subagents once, and make working/lit globes blaze while idle/filtered ones fall to dim embers.

**Architecture:** Pure modules get TDD (`radarLifecycle`, `activeFor`); R3F material/window behavior is changed in place and verified in the running app via the preview tools. The frontend consumes the backend's new `status:"terminated"` (backend plan, Task B4) by treating it like `closed` in the lifecycle reconciler. Animation is gated on *minimize* (tracked from Tauri window resize → `isMinimized`), never on blur.

**Tech Stack:** React, React-Three-Fiber, @react-three/postprocessing (Bloom), Tauri v2 window API, vanilla-TS bridge; tests via `pnpm test` (vitest), full build via `pnpm build`.

## Global Constraints

- Package manager is **pnpm**. Typecheck+bundle: `pnpm build` (= `tsc && vite build`). Unit tests: `pnpm test`.
- **Palette is fixed** — emerald `#3dffa0` (Claude), violet `#b98cff` (Codex), verdict amber `#ff5a37`, phosphor green `#76ff9d`, bg `#020403`. No new hues; contrast is illumination vs dullness only.
- **Not always-on-top.** The overlay stays a normal window (`alwaysOnTop:false` unchanged) — it must never float over other apps.
- **Animation gate = minimize only.** Moving focus to another app/screen must NOT pause; the habits forest must NOT collapse on blur.
- `RadarStatus` wire values: `'working' | 'idle' | 'closed' | 'terminated'` (mirror of the Rust contract).

---

### Task F1: Frontend `terminated` lifecycle (implode once, never resurrect)

**Files:**
- Modify: `src/viz/radarTypes.ts` (`RadarStatus` union)
- Modify: `src/viz/radarLifecycle.ts` (`LiveId.status` union; treat `terminated` like `closed`)
- Test: `src/viz/radarLifecycle.test.ts`

**Interfaces:**
- Consumes: backend `radar_state` agents with `status:"terminated"`.
- Produces: a `terminated` agent imploding exactly once and staying `gone` (reuses the proven closed-path graveyard logic).

- [ ] **Step 1: Write the failing test**

Add to `src/viz/radarLifecycle.test.ts`:

```typescript
describe('reconcileLifecycle — terminated subagent', () => {
  it('a terminated id implodes like closed and does not resurrect after prune', () => {
    const alive = run({}, [live('a', 'working')], 240);
    const closing = reconcileLifecycle(alive, [live('a', 'terminated')], DT);
    expect(closing.a.phase).toBe('imploding');

    const gone = run(closing, [live('a', 'terminated')], 240);
    expect(gone.a.phase).toBe('gone');
    const pruned = pruneGone(gone);
    expect(pruned.a).toBeUndefined();
    // a stray late payload still tagging it terminated must NOT bloom it back
    const after = reconcileLifecycle(pruned, [live('a', 'terminated')], DT);
    expect(after.a.phase).toBe('gone');
    expect(after.a.scale).toBe(0);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm test -- radarLifecycle`
Expected: FAIL — `'terminated'` is not assignable to `LiveId.status`, and/or it is not treated as terminal.

- [ ] **Step 3: Widen the status unions**

In `src/viz/radarTypes.ts`:

```typescript
export type RadarStatus = 'working' | 'idle' | 'closed' | 'terminated';
```

In `src/viz/radarLifecycle.ts`:

```typescript
export type LiveId = { id: string; status: 'working' | 'idle' | 'closed' | 'terminated' };
```

- [ ] **Step 4: Treat `terminated` as terminal in the reconciler**

In `reconcileLifecycle` (radarLifecycle.ts), change the closed check so both terminal statuses implode + graveyard. Replace:

```typescript
    const closed = status === 'closed';
```

with:

```typescript
    // `closed` (root/process gone) and `terminated` (a finished subagent) are both
    // terminal: implode once, then stay gone (no resurrection bloom).
    const closed = status === 'closed' || status === 'terminated';
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `pnpm test -- radarLifecycle`
Expected: PASS (the new test + all existing lifecycle tests).

- [ ] **Step 6: Commit**

```bash
git add src/viz/radarTypes.ts src/viz/radarLifecycle.ts src/viz/radarLifecycle.test.ts
git commit -m "feat(radar-ui): implode terminated subagents like closed (no resurrection)"
```

---

### Task F2: Maximize on launch, keep animating off-focus, pause only on minimize

**Files:**
- Modify: `src-tauri/src/lib.rs` (`summon_overlay` maximizes; show maximized at launch)
- Modify: `src/main.ts` (replace blur-dismiss with minimize tracking)
- Modify: `src/viz/bridge.ts` (`minimized` state + reducer cases)
- Modify: `src/viz/WarRoom.tsx` (`activeFor` signature; read `scene.minimized`; drop blur listeners)
- Test: `src/viz/mount.test.ts`

**Interfaces:**
- Produces: `activeFor(summoned: boolean | undefined, visHidden: boolean, minimized?: boolean): boolean` — active unless minimized; blur is irrelevant.
- Consumes (bridge): events `warden_minimized` / `warden_restored`.

- [ ] **Step 1: Rewrite the `activeFor` tests (failing)**

In `src/viz/mount.test.ts`, replace the two `activeFor`/`frameloopFor` describe blocks that reference `blurred` with minimize semantics:

```typescript
describe('activeFor — animate unless minimized (blur is irrelevant)', () => {
  it('a summoned overlay stays active when blurred / on another screen', () => {
    expect(activeFor(true, false)).toBe(true);
    // even if the page-visibility flag is stale-true right after a native show
    expect(activeFor(true, true)).toBe(true);
  });

  it('pauses only when minimized', () => {
    expect(activeFor(true, false, true)).toBe(false);
    expect(frameloopFor(!activeFor(true, false, true))).toBe('never');
  });

  it('a visible dev/browser page (no summon) is active, and pauses when tab-hidden', () => {
    expect(activeFor(false, false)).toBe(true);
    expect(activeFor(undefined, false)).toBe(true);
    expect(activeFor(false, true)).toBe(false);
  });

  it('minimize overrides everything', () => {
    expect(activeFor(false, false, true)).toBe(false);
    expect(activeFor(undefined, false, true)).toBe(false);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm test -- mount`
Expected: FAIL — current `activeFor` pauses on blur and has no minimize semantics.

- [ ] **Step 3: Reimplement `activeFor`**

In `src/viz/WarRoom.tsx`, replace `activeFor`:

```typescript
// The render loop runs whenever the window is on screen — even unfocused or sitting
// on another display. The ONLY thing that pauses it is MINIMIZE (CPU saver). A
// summoned overlay is active regardless of the page-visibility flag (a native
// .show() may leave document.hidden stale-true). Dev/browser (no summon) keys off
// page visibility so a hidden tab still pauses.
export function activeFor(
  summoned: boolean | undefined,
  visHidden: boolean,
  minimized = false,
): boolean {
  if (minimized) return false;
  return Boolean(summoned) || !visHidden;
}
```

- [ ] **Step 4: Track minimize in `main.ts` (replace blur-dismiss)**

In `src/main.ts`, delete the `appWindow.onFocusChanged(...) → warden_dismiss` block (lines ~131–138) and replace with resize-driven minimize tracking:

```typescript
// The overlay STAYS ON SCREEN and KEEPS ANIMATING when it loses focus or you move to
// another display — animation is no longer tied to focus. The ONLY pause is minimize.
// Tauri has no dedicated minimize event, so we sample isMinimized() on every resize.
appWindow.onResized(async () => {
  try {
    bridge.ingest((await appWindow.isMinimized()) ? 'warden_minimized' : 'warden_restored', {});
  } catch {
    /* non-Tauri / dev surface: no-op */
  }
}).catch(() => {});
```

(`appWindow` is the existing `getCurrentWindow()` handle already imported in `main.ts`.)

- [ ] **Step 5: Add `minimized` to the bridge state**

In `src/viz/bridge.ts`, add `minimized: false` to the initial scene state (next to `summoned`), and add reducer cases (near the `warden_hotkey`/`warden_dismiss` cases):

```typescript
    case 'warden_minimized':
      return state.minimized ? state : { ...state, minimized: true };

    case 'warden_restored':
      return state.minimized ? { ...state, minimized: false } : state;
```

(Also add `minimized: boolean` to the SceneState type and its initial value.)

- [ ] **Step 6: Gate the Canvas on minimize; drop blur state**

In `src/viz/WarRoom.tsx`:
- Remove the `blurred` state + the `focus`/`blur` listeners in the `useEffect` (keep `visibilitychange` → `visHidden`).
- Change the active derivation:

```typescript
  const active = activeFor(scene.summoned, visHidden, scene.minimized);
```

- [ ] **Step 7: Maximize on launch (Rust)**

In `src-tauri/src/lib.rs`, make `summon_overlay` maximize before showing:

```rust
fn summon_overlay(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.maximize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit(
            "warden_hotkey",
            serde_json::json!({"hotkey":"cmd+option+control+m"}),
        );
    }
}
```

Then show it maximized at startup: at the END of `setup(...)` (after state + watchers are managed), add:

```rust
            // Start maximized + on screen (Karim: "start out completely maximized").
            // Remove this single call to revert to hotkey-only summon.
            summon_overlay(&app.handle());
```

- [ ] **Step 8: Unit tests green**

Run: `pnpm test -- mount`
Expected: PASS.

- [ ] **Step 9: Verify in the running app**

Start the app (`pnpm tauri dev`), then with the preview/inspection tools:
- Confirm it opens filling the screen.
- Click to another app / another display → confirm orbs keep moving and new agents still appear (check console has no pause; the canvas keeps rendering). Confirm the habits globes do NOT shrink (switch to Habits, click away, come back — they're full size, no Radar toggle needed).
- Minimize → confirm animation stops; restore → it resumes.

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/lib.rs src/main.ts src/viz/bridge.ts src/viz/WarRoom.tsx src/viz/mount.test.ts
git commit -m "feat(overlay): open maximized, animate off-focus, pause only on minimize"
```

---

### Task F3: Draggable like a native macOS window

**Files:**
- Modify: `src/viz/WarRoom.tsx` (add a top drag strip)
- Modify: `src/style.css` (`.wd-dragbar`)

**Interfaces:**
- Consumes: Tauri window API via dynamic import (test- and browser-safe).

- [ ] **Step 1: Add the drag strip to the war-room root**

In `src/viz/WarRoom.tsx`, as the FIRST child inside the top-level `<div className="viz-root …">` (before `<Canvas>`), add:

```tsx
      {/* Frameless window has no titlebar — this top strip is the drag handle. Drag
          to move (across displays); double-click toggles maximize; dragging while
          maximized un-maximizes first (macOS "zoom" behavior). Dynamic import keeps
          it safe under vitest/jsdom and the dev browser (no Tauri global). */}
      <div
        className="wd-dragbar"
        onMouseDown={async (e) => {
          if (e.button !== 0) return;
          try {
            const { getCurrentWindow } = await import('@tauri-apps/api/window');
            const win = getCurrentWindow();
            if (await win.isMaximized()) await win.unmaximize();
            await win.startDragging();
          } catch {
            /* non-Tauri surface: no-op */
          }
        }}
        onDoubleClick={async () => {
          try {
            const { getCurrentWindow } = await import('@tauri-apps/api/window');
            const win = getCurrentWindow();
            (await win.isMaximized()) ? await win.unmaximize() : await win.maximize();
          } catch {
            /* no-op */
          }
        }}
      />
```

- [ ] **Step 2: Style the drag strip**

In `src/style.css`, add (near the `.wd-chrome` block). Keep it BELOW the HUD/NavBar so their buttons stay clickable; it grabs the bare top band:

```css
/* Native-style titlebar drag handle for the frameless overlay. Thin top band; the
   HUD + nav sit above it (higher z) and keep their own pointer-events. */
.wd-dragbar {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 34px;
  z-index: 4;
  pointer-events: auto;
  cursor: grab;
}
.wd-dragbar:active {
  cursor: grabbing;
}
```

(If `.wd-hud`/NavBar use a z-index ≤ 4, bump the dragbar lower or the chrome higher so controls win — verify in Step 4.)

- [ ] **Step 3: Typecheck + bundle**

Run: `pnpm build`
Expected: PASS (no TS errors from the dynamic import).

- [ ] **Step 4: Verify in the running app**

With `pnpm tauri dev`:
- Drag the top strip → the window moves; drag it onto a second display → it follows.
- While maximized, start a drag → it un-maximizes into a movable window.
- Double-click the strip → toggles maximize.
- Confirm the HUD brand/metrics and the NavBar tabs are still clickable (not swallowed by the drag region).

- [ ] **Step 5: Commit**

```bash
git add src/viz/WarRoom.tsx src/style.css
git commit -m "feat(overlay): draggable titlebar strip (move across displays, un-maximize on drag)"
```

---

### Task F4: Habits filter — blaze the matches, crush the rest to embers

**Files:**
- Modify: `src/viz/Orb.tsx` (dim floors + opacity crush for the filter dim)
- Modify: `src/viz/WarRoom.tsx` (habits Bloom threshold/intensity)

**Interfaces:**
- Consumes: the eased `s.colorDim` (0 = lit, 1 = filtered-out) already computed in `Orb`'s `useFrame`.

- [ ] **Step 1: Deepen the colour crush**

In `src/viz/Orb.tsx`, lower the dim floors so a filtered-out node falls to ~18% colour instead of 42%. Replace:

```typescript
      const shellScale = dimScale(s.colorDim, 0.42);
      const innerScale = dimScale(s.colorDim, 0.45);
```

with:

```typescript
      const shellScale = dimScale(s.colorDim, 0.18);
      const innerScale = dimScale(s.colorDim, 0.22);
```

- [ ] **Step 2: Crush opacity too (so filtered nodes drop under the bloom)**

Still in `Orb.tsx`, fold the filter dim into opacity/emissive (today it is colour-only). Replace:

```typescript
      const dimK = 1 - s.dim * 0.6;
      gemMat.current.emissiveIntensity = (0.55 + s.glow * 0.6) * dimK;
      haloMat.current.opacity = (0.2 + s.glow * 0.28) * dimK;
      nodeMat.current.opacity = (0.45 + s.glow * 0.32) * dimK;
```

with:

```typescript
      const dimK = 1 - s.dim * 0.6;
      // The legend filter also crushes opacity/emissive (not just colour) so a
      // filtered-out node falls below the bloom threshold → near-dark ember, while a
      // match keeps its full halo and blooms. litK=1 when lit, ~0.18 when filtered.
      const litK = 1 - s.colorDim * 0.82;
      gemMat.current.emissiveIntensity = (0.55 + s.glow * 0.6) * dimK * litK;
      haloMat.current.opacity = (0.2 + s.glow * 0.28) * dimK * litK;
      nodeMat.current.opacity = (0.45 + s.glow * 0.32) * dimK * litK;
```

- [ ] **Step 3: Make the lit nodes pop in bloom**

In `src/viz/WarRoom.tsx`, lower the habits bloom threshold and lift intensity a touch so matches bloom harder and crushed nodes stay dark. Replace:

```tsx
          <Bloom intensity={0.93} luminanceThreshold={0.27} luminanceSmoothing={0.95} mipmapBlur radius={0.74} />
```

with:

```tsx
          <Bloom intensity={1.05} luminanceThreshold={0.22} luminanceSmoothing={0.95} mipmapBlur radius={0.78} />
```

- [ ] **Step 4: Typecheck + bundle**

Run: `pnpm build`
Expected: PASS.

- [ ] **Step 5: Verify the contrast in-app**

With `pnpm tauri dev` on the Habits tab: click a severity/harness legend chip and screenshot. Confirm matching globes are unmistakably brighter and the non-matching ones fall to dim embers (strong, not childish). Clear the filter → all return. Capture a before/after screenshot to share.

- [ ] **Step 6: Commit**

```bash
git add src/viz/Orb.tsx src/viz/WarRoom.tsx
git commit -m "feat(habits): dramatic lit-vs-dull contrast for the legend filter"
```

---

### Task F5: Radar — working blazes, idle dims, terminated implodes amber

**Files:**
- Modify: `src/viz/RadarConstellation.tsx` (`RadarGlobe` idle colour-crush + working lift + terminated amber; radar Bloom)

**Interfaces:**
- Consumes: `agent.status` (`working|idle|closed|terminated`), `radarNodeColor`, the eased `s.colorDim`.

- [ ] **Step 1: Strengthen idle dimming (glow + colour) and lift working**

In `src/viz/RadarConstellation.tsx`, in `RadarGlobe`'s `useFrame`, replace the glow block:

```typescript
    const fillGlow = 0.25 + agent.fillPct * 0.65; // fuller = hotter core
    const idleDim = working ? 0 : 0.28; // idle agents read dimmer (at-a-glance who's thinking)
    const targetGlow = (isRoot ? 0.8 : 0.55) + fillGlow - idleDim + (selected ? 0.9 : hovered ? 0.35 : 0);
    const targetDim = dimmed ? 1 : 0;
    // Legend colour-dim: dims for the legend filter OR the boolean other-selected
    // state, whichever is stronger — one eased float, colour only.
    const targetColorDim = Math.max(targetDim, Math.min(1, Math.max(0, dimTarget)));
```

with:

```typescript
    const fillGlow = 0.25 + agent.fillPct * 0.65; // fuller = hotter core
    // Working blazes; idle is crushed HARD on both glow and colour so the contrast
    // reads instantly. (was idleDim 0.28, colour untouched.)
    const idleDim = working ? 0 : 0.5;
    const workingLift = working ? 0.18 : 0;
    const targetGlow =
      (isRoot ? 0.8 : 0.55) + fillGlow - idleDim + workingLift + (selected ? 0.9 : hovered ? 0.35 : 0);
    const targetDim = dimmed ? 1 : 0;
    const idleColorDim = working ? 0 : 0.5; // idle also DESATURATES, not only loses glow
    const targetColorDim = Math.max(targetDim, Math.min(1, Math.max(0, dimTarget)), idleColorDim);
```

- [ ] **Step 2: Deepen the radar colour floor (match habits)**

In the same `useFrame`, replace:

```typescript
    const shellScaleC = dimScale(s.colorDim, 0.42);
    const innerScaleC = dimScale(s.colorDim, 0.45);
```

with:

```typescript
    const shellScaleC = dimScale(s.colorDim, 0.2);
    const innerScaleC = dimScale(s.colorDim, 0.24);
```

- [ ] **Step 3: Terminated → amber as it implodes**

In `RadarGlobe`, where the base colours are derived from `radarNodeColor(agent)` (the `color`/`nodeColor`/`shellBase`/`innerBase` setup near the top of the component), make a terminated agent render in verdict amber. Add right after `const working = agent.status === 'working';`:

```typescript
  const terminated = agent.status === 'terminated';
  // A finishing subagent flares verdict-amber as the lifecycle implodes its scale.
  const baseHex = terminated ? '#ff5a37' : radarNodeColor(agent);
```

Then use `baseHex` in place of the existing `radarNodeColor(agent)` call(s) that build the THREE.Color bases for this globe (shell/inner/node/halo/gem). (Leave `RadarLinks`' own `radarNodeColor` use unchanged — a link fades out with its imploding endpoint already.)

- [ ] **Step 4: Radar bloom to match**

Replace the radar `<Bloom>` (RadarConstellation.tsx):

```tsx
        <Bloom intensity={0.95} luminanceThreshold={0.26} luminanceSmoothing={0.95} mipmapBlur radius={0.74} />
```

with:

```tsx
        <Bloom intensity={1.05} luminanceThreshold={0.22} luminanceSmoothing={0.95} mipmapBlur radius={0.78} />
```

- [ ] **Step 5: Typecheck + bundle**

Run: `pnpm build`
Expected: PASS.

- [ ] **Step 6: Verify in-app**

With `pnpm tauri dev` on the Radar tab and at least one working + one idle agent: screenshot and confirm the working globe blazes vs a clearly dim idle one. If a subagent finishes during the session, confirm it flashes amber and implodes away once (does not linger). Capture a screenshot to share.

- [ ] **Step 7: Commit**

```bash
git add src/viz/RadarConstellation.tsx
git commit -m "feat(radar): working blazes, idle crushed, terminated implodes amber"
```

---

### Task F6: Keep the subagent's real role visible under "subagent N"

**Files:**
- Modify: `src/viz/RadarDetailPanel.tsx` (show `role` in the identity section / head)

**Interfaces:**
- Consumes: `RadarAgent.role` (already in contract; e.g. Claude `agentType` "Explore").

- [ ] **Step 1: Surface role in the detail head**

In `src/viz/RadarDetailPanel.tsx`, add a `Role` row to `IdentitySection`'s `<dl>` (so a subagent labelled "subagent 2" still shows what it is). After the `Harness` block:

```tsx
        {agent.role ? (
          <div>
            <dt>Role</dt>
            <dd>{agent.role}</dd>
          </div>
        ) : null}
```

(The children roster already names subagents by `role || nickname || label` via `childName`, so the parent's panel lists them meaningfully too.)

- [ ] **Step 2: Typecheck + bundle**

Run: `pnpm build`
Expected: PASS.

- [ ] **Step 3: Verify**

With `pnpm tauri dev`: select a subagent globe → its panel title reads `subagent N` and the identity section shows its `Role` (e.g. Explore). Open its parent → the children roster lists the subagents.

- [ ] **Step 4: Commit**

```bash
git add src/viz/RadarDetailPanel.tsx
git commit -m "feat(radar-ui): show subagent role in the detail panel"
```

Note (confirm with Karim): the free-text task *description* is not in the `RadarAgent` contract today (only `role`). If he wants the full description shown too, that's a one-field add (`RadarAgent.task`) in the backend `build_agent` + `radarTypes.ts` — a quick follow-up, out of scope here.

---

### Task F7: Full build + end-to-end verification

- [ ] **Step 1: Unit tests + typecheck + bundle**

Run: `pnpm test && pnpm build`
Expected: all green.

- [ ] **Step 2: End-to-end smoke (with the backend plan merged)**

With `pnpm tauri dev` and real Claude/Codex sessions running:
- Roots show folder names; spawn a subagent → it appears tethered as `subagent 1` within ~2s; let it finish → it implodes amber once and stays gone.
- Working vs idle contrast is obvious on the radar; the habits filter blazes matches and crushes the rest.
- Window opens maximized, keeps animating when you switch apps/displays, pauses on minimize, and drags between screens.

- [ ] **Step 3: Capture proof**

Screenshot the radar (working vs idle), the habits filter (lit vs dull), and confirm in logs/console there is no pause on blur. Share with Karim.

---

## Self-Review

- **Spec coverage:** terminated implode (F1), maximize + animate-off-focus + minimize-pause + habits-no-collapse (F2), draggable (F3), habits glow (F4), radar glow + terminated amber (F5), subagent role visible (F6), verification (F7). Naming + linking + termination *detection* are the backend plan.
- **Type consistency:** `RadarStatus` and `LiveId.status` both gain `'terminated'`; `activeFor` third param renamed `blurred`→`minimized` and all call sites + tests updated.
- **No placeholders:** every code change shows the before/after; visual tasks specify exact verification with the preview tools.
- **Dependency note:** F1 and F5/F7's terminated behavior depend on the backend plan (B4) emitting `status:"terminated"`. Land backend first (it defines the contract), then this plan.
