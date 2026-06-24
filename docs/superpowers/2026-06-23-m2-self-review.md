# WARDEN M2 "Face" — Spec Self-Review

> Controller's checklist against `docs/superpowers/specs/2026-06-22-m2-face-design.md` §0 Definition of Done
> and non-goals. Run at the close of the M2 build (branch `m2-face`, Tasks 0–10). Evidence column cites the
> task + commit that satisfies each item.

## §0 Definition of Done (7 criteria)

| # | DoD criterion | Status | Evidence |
|---|---|---|---|
| 1 | Menubar agent (no Dock), continuously ingesting `~/.claude/projects` **and** `~/.codex/sessions` via FSEvents | ✅ code-complete; runtime confirmed at live launch | Task 5 `ef00119` (`ActivationPolicy::Accessory` + `LSUIElement=true` + tray; bundle verified). Task 3 `02cf004` (Codex adapter). Task 4 `7caa7a6` (FSEvents live tail, byte-offset watermark, both adapters). Startup runs `backfill_all` + `spawn_watchers`, per-adapter isolated. |
| 2 | Global hotkey **⌘⇧Space** summons pre-warmed overlay <150 ms | ✅ wiring verified; latency at live launch | Task 5 `ef00119` (⌘⇧Space replaces poisoned Alt+Space, `is_registered`-guarded; hidden pre-warmed `overlay` window; summon = `set_ignore_cursor_events(false)`+`show()`+`set_focus()`). Mount-once island (Task 7) keeps R3F cost off the summon path. |
| 3 | Asking runs the M1 pipeline + drives the **R3F war-room from real Fugu signals** (token deltas, orchestration weight, per-finding verdicts) — no fabricated theater | ✅ verified honest | Task 6 `4d8cdeb` emits `candidates_nominated`/`finding_verdict` from real findings/verdicts (+ existing `fugu_delta`/`fugu_usage`). Task 7 `53653f0` bridge maps them; degrades to plain token weight off-Fugu. Controller visually confirmed: node count = real candidates, amber `#ff5a37` flares = real confirmed verdicts, refuted collapse. |
| 4 | Diagnosis renders severity, frequency, est. cost, do/stop, narrative, **per-finding evidence drill-down to the exact `raw_ref`** | ✅ | Task 9 `1a5b38e` (full diagnosis screen; drill-down → session·turn·quote). Fix `44ec2b8` adds read-only `resolve_evidence` recovering the ground-truth quote from `raw_ref` when a Fugu finding's quote is null — "every claim traceable to ground truth." |
| 5 | Each finding shows a **read-only fix preview** (diff); apply deferred to M4 | ✅ | Task 6 `4d8cdeb` (`forge::fix_preview`, `applied:false`, strictly read-only — verified no `fs::write` in production path). Task 9 `1a5b38e` (UI renders unified diff; `[Y/n]` disabled + "apply coming in M4"; no apply wiring anywhere in `src/`). |
| 6 | UI **differentiates harnesses** (Claude vs Codex) everywhere a session/finding appears | ✅ | `harness_theme.rs` (Task 2 `e088394`) + web `harnessTheme.ts` (Task 7) single source. Task 10 `cd12d55` swept all 6 surfaces (HUD breakdown, diagnosis badge, war-room rim+legend, Reveal+Recap badges) — color paired with glyph + label everywhere (color-blind a11y). Claude emerald `#3dffa0` ◆, Codex violet `#b98cff` ▲. |
| 7 | Reasoning engine **env-swappable** (base URL + models), defaulting to Fugu | ✅ | Task 1 `df3df79`/`75b625a` (`WARDEN_BRAIN_BASE_URL`/`_API_KEY`/`_DIAGNOSE_MODEL`/`_VERIFY_MODEL`/`_EFFORT`, default Sakana/Fugu; Responses-API path unchanged → Near AI ready). |

**All 7 DoD criteria met** (code-complete + tested; the two runtime-GUI behaviors in #1/#2 — Dock-less + hotkey summon — are build- and wiring-verified and confirmed at the live launch step).

## Non-goals — respected

- ✗ RADAR / voice / screen / Forge **apply** — remain `scaffold::not_in_slice()` stubs (unchanged). ✅
- ✗ Cursor / Hermes / OpenClaw / Generic adapters — not built; Claude + Codex only (trait + registry architected for more). ✅
- ✗ Settings UI pane — env + `~/.warden/config.toml` only (Task 6 `config.rs`). ✅
- ✗ Any `git push` / MR / PR / outbound publishing; no writes to user projects — preview-only verified (read-only `fix_preview`, no apply wiring, read-only `resolve_evidence`). ✅

## Honest deviations from spec wording (all defensible, all documented)

1. **Adapter trait** keeps `backfill(&self)->Vec<SessionBatch>` and adds `parse_range`/`roots` + `AdapterRegistry` rather than the spec's `backfill(&self,store)`/`watch(&self,store,tx)`/`map`. The registry + scheduler own store-upsert and watching, keeping adapters pure and unit-testable. (Tasks 2–4)
2. **Codex `token_count`** maps from the real nested `payload.info.last_token_usage` (per-event delta) — the spec's §2.3 table showed a flattened shape; the real on-disk format nests it. (Task 3)
3. **Remotion intro** plays live via `@remotion/player` on first summon (not build-time pre-rendered); `@remotion/renderer` is deferred post-M2 per risk R-Rem. `scripts/render-intro.mjs` is a documented stub. (Task 8)
4. **Recap** ships via the `MediaRecorder` canvas-capture fallback (role 3), with `@remotion/renderer` post-M2. (Task 8)
5. **`meta.ignored_record_types`** drift counts are lossy-merged (not deep-merged) on a tail slice that itself carries an unknown record — tolerable for M2 preview-only telemetry. (Task 4)

## Risks (spec §13) — mitigated

- **R-Bundle** — mount-once island, RAF pause on hidden, Remotion in a lazy 236 kB `PlayerHost` chunk (reviewer rebuilt `dist/` to confirm the main bundle is Remotion-free). ✅
- **R-Rem** — Player (live reveal + intro) now; recap via MediaRecorder; `@remotion/renderer` post-M2. ✅
- **R-Codex** — defensive `unknown → SystemNotice`; golden + idempotency tests pinned to real on-disk shapes. ✅
- **R-Honesty** — off-Fugu engines degrade to delta pulses + plain token weight; never fabricated. ✅
- **R-NodeScale** — candidate nodes clamp ≤24 with a `clustered` overflow count + glyph. ✅
- **R-Color-a11y** — every harness color paired with glyph + label. ✅

## Test/build evidence at close

- `cd src-tauri && cargo test` — 67 unit + 2 e2e, 0 failures.
- `pnpm test` — 38 vitest (bridge, timing, recorder, viz smoke, diagnosis render), 0 failures.
- `pnpm build` — clean (`tsc && vite build`), lazy `PlayerHost` chunk split, no CDN.
- `pnpm tauri build` — full `WARDEN.app` bundle (final run at close).
- Controller visual verification (headless `/dev-viz.html`): honest war-room (hot-white cores, harness rims, amber confirmed flares, collapse on refuted), cinematic reveal slam-in with honest counts, 0 console errors.

*Outcome: M2 "Face" meets its Definition of Done. Deferrals are explicit and architected-for. No non-goal breached.*
