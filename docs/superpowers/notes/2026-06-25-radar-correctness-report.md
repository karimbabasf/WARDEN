# Radar Correctness Fix — Implementation Report

**Date:** 2026-06-25
**Plan:** `docs/superpowers/plans/2026-06-25-radar-correctness-fix.md`
**Status:** DONE — both faults fixed with TDD, full suite green, verified on the live store.
**No git:** nothing was added/committed/pushed; no frontend (`src/`) edits.

---

## Summary

Both verified backend radar faults are fixed:

- **Fault A — subagent linking (was 0/740):** the store-wide relinker
  (`link_claude_subagents_in_store`) now reads each Claude subagent's `toolUseId` from
  the **on-disk sidecar `agent-<id>.meta.json`** (immune to the live-tail meta clobber)
  instead of the row's `meta_json`, links child→parent by `Agent`/`Task` `call_id` ==
  sidecar `toolUseId`, and repairs the row's identity fields
  (`toolUseId`/`agentType`/`description`) via `merge_session_meta` when missing.
- **Fault B — working/idle from mtime (racy, flickered):** status now derives from the
  session's **last ingested Event** (conversation state), not transcript file mtime.
  Deterministic across reads → the working↔idle flicker is gone. Registry `status`
  stays authoritative when present; mtime survives only as a last-resort fallback when a
  session has no usable events.

**Live probe (Task 3):** subagent→parent links now **> 0** (was 0), at least one root has
**childCount > 0** (was 0), and **status is byte-identical across two consecutive probe
runs** (0 flips).

---

## Live diagnosis (pre-fix, on a copy of `~/.warden/warden.db`)

Confirmed both root causes against the real store + on-disk sidecars before coding:

- 740 Claude subagent rows under `/subagents/`; **0** carried `toolUseId` in `meta_json`
  (all clobbered to `{"ignored_record_types":{"attachment":N}}`); **0** had a non-null
  `parent_session_id`.
- The on-disk sidecar **does** carry the durable fields, e.g.
  `{"agentType":"general-purpose","description":"...","toolUseId":"toolu_015ND3Hz..."}`.
- Sidecar path == transcript `source_path` with `.jsonl` → `.meta.json` (same
  `subagents/` dir).
- Parent side healthy: 455 `Agent`/`Task` `tool_call` events with `call_id` present.

---

## Changes by file

### `src-tauri/src/ingest/claude_code.rs` (Fault A)
- **`link_claude_subagents_in_store`** (the relinker): replaced the
  `s.meta.get("toolUseId")` gate with a read of the on-disk sidecar via the new
  `sidecar_path(source_path)` + the existing `read_subagent_meta`. The sidecar
  `toolUseId` is used for matching (falling back to the row meta for synthetic stores
  that never wrote a sidecar — keeps the existing `relink_resolves_subagent_across_ingest_passes`
  test green). Kept the existing ClaudeCode-harness, `/subagents/`, and self-link
  (`parent != &s.id`) guards.
- **Identity repair:** when the row is missing `toolUseId`/`agentType`/`description`,
  `merge_session_meta` writes the sidecar values back onto the row (gated to
  only-when-missing to bound the write cost), so the FACE detail-panel Role/name survive
  the live-tail clobber.
- Added a private helper **`sidecar_path(transcript: &Path) -> PathBuf`**
  (`agent-<id>.jsonl` → `agent-<id>.meta.json`, spelled out so it is robust to ids that
  contain dots).
- **New test (TDD):** `relink_reads_tool_use_id_from_sidecar_not_clobbered_meta` +
  helper `upsert_subagent_with_sidecar_but_clobbered_meta` (writes a real sidecar on
  disk while leaving the row's `meta_json` in the production clobbered shape).

### `src-tauri/src/radar/liveness.rs` (Fault B core + `partition_claude` refactor)
- Added imports `crate::ir::{Event, EventRecord, Turn}` and `chrono::{DateTime, Utc}`.
- **New pure function `status_from_last_event(events, now, stale_secs) -> Option<AgentStatus>`:**
  last conversational event decides working/idle —
  `UserPrompt` ⇒ Working; `ToolCall` with no answering `ToolResult` ⇒ Working; a
  completed `AssistantText`/`TokenUsage` or answered `ToolResult` ⇒ Idle. Backstop:
  a "working" verdict on an event older than `stale_secs` downgrades to Idle. `None`
  when there is no usable event (caller falls back to mtime). Age is computed from the
  event's `ts` vs `now` (deterministic across reads — no file mtime).
- **`partition_claude` signature change:** the racy mtime `None`-arm
  (`transcript_mtime_secs_ago` + `working_threshold_secs`) is replaced by an injected
  `fallback_status: &dyn Fn(&str) -> AgentStatus`. Registry `status` precedence
  (`busy`→Working / else→Idle) is unchanged; only the *absent-status* path is now the
  caller's conversation-state decision. This keeps `partition_claude` a pure router and
  moves the store-backed rule to `assemble` (which has store access).
- **New tests (TDD):** `status_from_last_event_classifies_by_conversation_state`,
  `status_from_last_event_stale_backstop_downgrades_to_idle`,
  `status_from_last_event_is_deterministic_across_reads`. Updated the 3 existing
  `partition_claude_*` tests to the new `fallback_status` closure (their subject —
  registry-status precedence + dead-PID drop — is unchanged and still asserted).

### `src-tauri/src/radar/mod.rs` (Fault B wiring)
- **`assemble`:** builds a `fallback_status` closure (new helper
  `claude_conversation_status`) and passes it to `partition_claude`; reads the new
  `radar_working_stale_secs()` tunable.
- **New helper `claude_conversation_status(store, sessions, external_id, now, stale_secs, mtime_fn)`:**
  bridges the registry external `sessionId` → store row → `status_from_last_event`, with
  the mtime heuristic only as a last resort (no row / no usable events).
- **`agent_status`** (the non-registry / subagent path) now takes `store` + `now` and
  uses `status_from_last_event` first, mtime last; updated its one call site.
- **New integration test (TDD):** `assemble_status_from_conversation_state_is_deterministic`
  (a `TokenUsage`-tailed session → idle, a `UserPrompt`-tailed session → working, two
  assembles on the unchanged store at one instant → identical statuses).
- **Updated one existing test's expectation:** `claude_forest_includes_only_registry_open_sessions`
  asserted the old mtime "idle"; the `seed` helper's last event is a fresh unanswered
  `UserPrompt`, whose honest verdict under Fault B is **working**. Membership assertions
  unchanged. (This is the one behavioral expectation the fix deliberately corrects.)

### `src-tauri/src/util.rs` (new tunable)
- Added **`radar_working_stale_secs()`** (`WARDEN_RADAR_WORKING_STALE_SECS`, default
  **180**) following the existing env-helper pattern — the backstop window for the
  conversation-state "working" verdict.

---

## Test evidence (failing → green)

**Fault A — RED (before the fix):**
```
---- ingest::claude_code::tests::relink_reads_tool_use_id_from_sidecar_not_clobbered_meta stdout ----
panicked at src/ingest/claude_code.rs:1291:9:
at least one sidecar-resolved link recorded
test result: FAILED. 0 passed; 1 failed; ... 141 filtered out
```
(The old linker read `s.meta["toolUseId"]`, which is absent on the clobbered row → 0 links.)

**After the fix — module green:** `ingest::claude_code` → 10 passed.
**Fault B unit tests — green:** `radar::liveness` → 10 passed (incl. the 3 new).
**Fault B integration — green:** `radar::` → 53 passed (incl. the new determinism test).

**Full suite (Definition of Done #1), exact summary line:**
```
test result: ok. 146 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s
```
(plus the 2 integration-test binaries → **148 total**, 0 failed.)

New tests added: **5** (1 Task 1 linking, 3 Task 2 unit, 1 Task 2 integration) — exceeds
the "3+ new tests" bar.

---

## Live probe (Task 3) — before vs after

Command (run on a copy, twice, back-to-back):
```
cp ~/.warden/warden.db /tmp/warden_probe.db
WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --manifest-path src-tauri/Cargo.toml --example radar_probe
```
Store had 1103 sessions; the live `~/.claude/sessions` registry decides the open forest.

| metric | before (live diagnosis) | after (probe) |
|---|---|---|
| total agents (open forest) | — | 5 |
| status breakdown | — | `{idle: 5}` |
| subagents w/ non-null parent | **0 / 740** | **1** (depth=1, role=`general-purpose`) |
| roots with childCount > 0 | **0** | **1** ("WARDEN ②", childCount=1) |
| childCount distribution | — | `{0: 4, 1: 1}` |
| status flips across 2 runs | (flickered) | **0** (flicker gone) |

Determinism detail (analyzed with a script, not by eyeballing JSON): across the two
consecutive runs the **structural fields** (status / parentId / childCount / depth /
labels / composition) are **byte-identical**; the only run-to-run variation is the
wall-clock `generatedAt` / per-activity `ts` — exactly as expected. The **status vector
is identical** (0 flips). JSON byte size identical (9321 bytes) both runs.

The one linked subagent renders `idle` (not `terminated`) on live data — its parent has
not logged a tool-result for its call_id and it has not passed the silence backstop, so
that is the honest live state. The termination path itself is proven end-to-end by the
pre-existing `terminated_subagent_is_emitted_once_then_dropped_and_never_resurrects` test
(now meaningful because subagents finally have parents); **no termination-path gap was
found**, so no extra code was written for Task 3.

The git-tracked `src/viz/preview/realRadar.json` (overwritten by the probe) was **restored
to its exact pre-probe working-tree state** — no stray change left behind.

---

## Deviations from the plan (adapted to the real code)

1. **`partition_claude` signature change.** The plan described replacing the `None`-arm
   mtime fallback in place. The real `partition_claude` is a pure unit-tested core, so
   the cleanest honest fix was to make the fallback **injectable**
   (`fallback_status: &dyn Fn(&str) -> AgentStatus`), removing `transcript_mtime_secs_ago`
   + `working_threshold_secs` from its signature, and to put the store-backed
   conversation-state rule in `assemble` (which has the store). The 3 existing
   `partition_claude_*` tests were updated to the new closure shape; their behavioral
   subject (registry precedence, dead-PID drop) is preserved.

2. **`agent_status` also updated.** The plan named `liveness.rs` + `mod.rs`'s status fn.
   There are actually **two** status decision points in `mod.rs`: the `partition_claude`
   fallback (registry-open sessions) *and* `agent_status` (non-registry sessions, e.g.
   subagents, which have no PID). Both were switched to conversation-state so a subagent's
   status is honest too; this required threading `store` + `now` into `agent_status` and
   updating its single call site.

3. **Age from event `ts`, not file mtime, inside the new rule.** `status_from_last_event`
   computes the staleness backstop from the last event's `ts` (already in the IR) vs
   `now`, which is strictly deterministic on an unchanged store — a stronger guarantee
   than the plan's "no new event for > stale" phrasing, and the property the determinism
   test pins.

4. **One existing test expectation corrected** (`claude_forest_includes_only_registry_open_sessions`:
   old mtime "idle" → conversation-state "working" for a fresh unanswered prompt). This
   is the intended semantic correction, documented in the test.

5. **Unrelated pre-existing working-tree edits left untouched.** `src-tauri/src/commands.rs`
   (hotkey text `⌘⇧Space`→`⌘⌥⌃M`) and `src-tauri/src/harness_theme.rs` (Codex glyph
   `▲`→`▣`) were already modified before this task (the session-start `git status` listed
   `commands.rs` as `M`). They are **not** part of this fix and were not modified by me.

---

## Concerns / follow-ups (non-blocking)

- **Per-recompute cost:** `agent_status` now calls `store.session_events(&s.id)` for each
  non-registry session, and `claude_conversation_status` does one more per registry-open
  Claude session. `build_agent` already loads `session_events` per kept agent, so for the
  open forest the extra reads are bounded (forest size, not the 1103-row store). If the
  open forest ever grows large, hoist a single `session_events` fetch per kept session and
  reuse it for both status and `build_agent`. Not needed at current scale (5 open agents).
- The mtime fallback (`radar_working_ms`, `transcript_mtime_secs_ago`) is now only reached
  when a session has **no** usable events — effectively dead for real transcripts. Kept as
  a safety net; could be removed in a later cleanup once confirmed never hit in production.
