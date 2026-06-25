# Discovery Mode Double-Click Home Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Discovery Mode double-click gesture that returns the shared 3D camera to the centered, fully zoomed-out overview while browsing.

**Architecture:** `WarRoom.tsx` owns whether the app is in Discovery Mode and converts a valid double-click into a monotonically increasing camera reset token. `CameraRig.tsx` receives the token and flies the OrbitControls camera to a canonical overview pose without changing selected globe behavior.

**Tech Stack:** React 19, TypeScript, Vite, Vitest, React Three Fiber, drei OrbitControls.

## Global Constraints

- Do not push or open a PR/MR without Karim's explicit instruction in the same message.
- Work in the existing `/Users/karimbaba/WARDEN` branch and avoid unrelated dirty files.
- Discovery Mode means `selectedId === null` and `focusStack.length === 0`.
- Buttons, inputs, selects, textareas, links, and contenteditable elements must not trigger the camera home gesture.
- The reset must not clear filters, switch tabs, dismiss the overlay, or mutate backend state.

---

### Task 1: Discovery Mode Double-Click Camera Home

**Files:**
- Modify: `/Users/karimbaba/WARDEN/src/viz/WarRoom.tsx`
- Modify: `/Users/karimbaba/WARDEN/src/viz/CameraRig.tsx`
- Modify: `/Users/karimbaba/WARDEN/src/viz/useOrbCamera.ts`
- Modify: `/Users/karimbaba/WARDEN/src/viz/useOrbCamera.test.ts`
- Create: `/Users/karimbaba/WARDEN/src/viz/WarRoom.test.ts`

**Interfaces:**
- Produces: `isDiscoveryHomeDoubleClickAllowed(args: { selectedId: string | null; focusDepth: number; eventTarget: EventTarget | null }): boolean`
- Produces: `cameraTargetForOrbitOverview(): CameraTarget`
- Consumes: `CameraRig({ selected, focusBounds, homeSignal })`, where `homeSignal` is a number incremented by `WarRoom`.

- [ ] **Step 1: Write failing Discovery Mode guard tests**

```ts
import { describe, expect, it } from 'vitest';
import { isDiscoveryHomeDoubleClickAllowed } from './WarRoom';

describe('isDiscoveryHomeDoubleClickAllowed', () => {
  it('allows double-click home while browsing with no selected globe or radar focus', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: document.body })).toBe(true);
  });

  it('blocks double-click home while a globe is selected', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: 'agent-1', focusDepth: 0, eventTarget: document.body })).toBe(false);
  });

  it('blocks double-click home while radar focus is active', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 1, eventTarget: document.body })).toBe(false);
  });

  it('blocks double-click home from interactive controls', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: document.createElement('button') })).toBe(false);
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: document.createElement('input') })).toBe(false);
  });
});
```

- [ ] **Step 2: Run guard tests to verify they fail**

Run: `pnpm vitest run src/viz/WarRoom.test.ts`

Expected: FAIL because `WarRoom` does not export `isDiscoveryHomeDoubleClickAllowed`.

- [ ] **Step 3: Write failing camera-home helper test**

```ts
import { cameraTargetForOrbitOverview } from './useOrbCamera';

it('returns the canonical OrbitControls overview pose used by double-click home', () => {
  expect(cameraTargetForOrbitOverview()).toEqual({
    position: { x: 0, y: 1, z: 12.6 },
    lookAt: { x: 0, y: 0, z: 0 },
  });
});
```

- [ ] **Step 4: Run camera helper test to verify it fails**

Run: `pnpm vitest run src/viz/useOrbCamera.test.ts`

Expected: FAIL because `cameraTargetForOrbitOverview` is not exported yet.

- [ ] **Step 5: Implement guard, root double-click handler, and camera reset token**

```ts
export function isDiscoveryHomeDoubleClickAllowed(args: {
  selectedId: string | null;
  focusDepth: number;
  eventTarget: EventTarget | null;
}): boolean {
  if (args.selectedId !== null || args.focusDepth > 0) return false;
  if (!(args.eventTarget instanceof Element)) return true;
  return args.eventTarget.closest('button, input, select, textarea, a, [contenteditable="true"], [role="button"]') === null;
}
```

In `WarRoom`, add `homeSignal`, increment it on allowed `onDoubleClick`, clear hover, and pass it into `SceneShell` and `CameraRig`.

- [ ] **Step 6: Implement canonical OrbitControls overview target and CameraRig reset handling**

```ts
export function cameraTargetForOrbitOverview(): CameraTarget {
  return {
    position: { x: 0, y: 1, z: 12.6 },
    lookAt: { x: 0, y: 0, z: 0 },
  };
}
```

In `CameraRig`, watch `homeSignal`; when it changes after mount, set `targetGoal` and `posGoal` from `cameraTargetForOrbitOverview`, clear stored focus-home state, and call `beginFly()`.

- [ ] **Step 7: Run focused tests and build**

Run:

```bash
pnpm vitest run src/viz/WarRoom.test.ts src/viz/useOrbCamera.test.ts
pnpm build
```

Expected: both commands pass.
