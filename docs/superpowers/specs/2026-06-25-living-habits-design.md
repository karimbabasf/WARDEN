# Living Habits — Design Spec (2026-06-25)

**Status:** locked by Karim ("make the decisions and implement"). Foundation-first, verified per piece.

## Goal
Turn WARDEN's static "Habits" readout into a **living, time-windowed** system: habits reflect
recent reality, refresh at a cadence tied to the selected window, and **implode when proven fixed**
via a time-gated streak. Guardrail writes to `~/.claude/CLAUDE.md` **update in place** (no bloat).

Almost entirely local. The only external call remains GLM-5.2 via NEAR AI (already wired). No new
third-party tools. Time-windows are timestamp filters over data WARDEN already stores.

## The master dial — timeframe windows (rolling)
Buttons: **Today (24h) · 7d · 30d · 6mo · All-time**. Rolling cutoffs from `now`; All-time = no cutoff.
The selected window controls THREE things at once:
1. **What is analyzed** — which sessions/findings (timestamp filter).
2. **Refresh cadence** — see below.
3. **How hard a habit is to clear** — streak K and gap S scale with the window.

## Two-layer refresh cadence
Cheap layer = WARDEN's local detectors (free, on-device). Expensive layer = GLM-5.2 (paid API call).
"Live" never means "call the AI every tick."

| Window   | Cheap (live feel)   | Expensive (GLM)                         |
|----------|---------------------|-----------------------------------------|
| Today    | ~1–2 min            | on-open + on material change (no clock)  |
| 7d       | every few min       | ~daily                                  |
| 30d      | on-open             | weekly                                  |
| 6mo      | on-open             | monthly                                 |
| All-time | on-open             | ~every 2 months                         |

Principle: expensive refresh ≈ **window ÷ 5**. Windowing is what makes frequent refresh affordable —
a small recent slice is cheap to re-scan (this also fixes the old all-time-recompute CPU storm at root).

## Resolution — time-gated clean streak (spaced repetition)
Bias: **easy to flag, hard to clear.** One slip in the window raises a habit; clearing it must be *earned*.
- Streak counts **clean credits** toward `K`. Reaching `K` → habit **implodes** (resolved).
- **Time-gated:** a clean session only adds a credit if **≥ S** has elapsed since the last credit — you
  cannot cram `K` in one afternoon; durability requires spacing.
- **A slip resets the streak to 0.**
- `K` and `S` scale with the window:

| Window   | K (credits) | S (min gap) | ~min wall-clock proof |
|----------|-------------|-------------|-----------------------|
| Today    | 1           | —           | within the day        |
| 7d       | 3           | ~1 day      | ~3 days               |
| 30d      | 5           | ~3 days     | ~2 weeks              |
| 6mo      | 8           | ~1 week     | ~2 months             |
| All-time | 10          | ~2 weeks    | ~5 months             |

- **Clean session** = an in-window session where the pattern could apply and did NOT fire.
  **Slip** = the pattern fired in an in-window session.
- (Future, optional) GLM "blessing" gate on 6mo/all-time before the final implode.

## Idempotent guardrail writes (no bloat)
- Each block is identified by its header `## WARDEN guardrail — {pattern_id}`.
- On change → **replace that block in place** (never append a second copy).
- On implode/fixed → **remove** the block.
- Extend M4's existing backup + sha256 integrity + revert path. Never blind-append.

## Liveness
- Small status indicator: "live · last scanned Xs ago", built on the existing FSEvents/scheduler
  ingest + RADAR presence. Honest signal — reflects the real last scan.

## Build order (bounded pieces, each a frozen contract)
1. **Time Machine (foundation):** `Window` resolution + windowed store queries + `nominate_windowed`
   + a windowed Tauri command. ← **start here**
2. **Heartbeat:** scheduler cadence per window (cheap loop + event-driven expensive trigger). Needs 1.
3. **Implode-when-fixed:** per-habit streak state (credits, last-credit time, reset-on-slip), time-gating,
   K/S-by-window. Needs 1 + 2.
4. **Clean Writes:** replace/remove-by-header in `forge.rs`, extending M4 backup/revert. Mostly independent.

**UI:** dial (segmented control) on the Habits instrument; each habit shows a streak progress arc;
imploded habits animate out. Built with `frontend-design` + `r3f-mastery` after the backend is solid.
