# Radar Correctness Fix — Implementation Plan

> **For agentic workers:** Execute task-by-task with TDD. Each fix lands with a regression test that fails before and passes after. Do NOT commit or push — the controller commits after review.

**Goal:** Make the radar correctly (1) link Claude subagents to their parent so each agent + its subagents render as one constellation with edges, and (2) report working vs idle from *real agent activity*, not transcript file mtime — low latency, no flicker.

**Architecture:** Rust `src-tauri/` radar/ingest/store. The frontend already draws parent→child lines (`RadarLinks`) and a status glow; both are starved/misfed by the backend. This plan fixes the backend signals only. Frontend visual polish (lush tethers, modern palette, brighter glow) is a SEPARATE later pass owned by the controller.

**Tech Stack:** Rust, rusqlite/FTS5, serde_json, the existing `radar_probe` example for live verification.

## Global Constraints
- macOS, Tauri v2. M2/M3 are **observe-only** — NEVER write to user projects. Reading `~/.claude/**` sidecars/registry is read-only and allowed.
- **No `git commit` / `git push`.** Implement + test + write a report; the controller commits.
- **TDD mandatory** (this is a correctness regression that already shipped once "fixed" without verification). Every task: failing test first, then the fix, then green.
- **Honest viz contract:** status must map to a real signal. No faking.
- Follow the env-helper convention in `util.rs` for any new tunable.

## Root causes (live diagnosis, 2026-06-25 — both verified against the real store + registry)

**Fault A — subagent linking (0/732 linked).** `link_claude_subagents_in_store` (`ingest/claude_code.rs:285-328`) gates the link on `s.meta.get("toolUseId")`, but **no** Claude subagent row carries `toolUseId` (`0/732`). The persist loop that writes it (`ingest/claude_code.rs:264-274`, `merge_session_meta`) runs **only inside `ingest_all`**. The live tail (`scheduler.rs`) re-`upsert_session_batch`es those rows, and `upsert_session_batch` (`store.rs:200`) writes `meta_json` REPLACE-unless-empty-sentinel — the subagent meta is `{"ignored_record_types":{"attachment":N}}` (non-empty, not the sentinel) so it **replaces**, clobbering any merged `toolUseId`; the scheduler path never re-runs the merge. Result: `tid` is always `None` → link never attempted → `parent_session_id` NULL for every Claude subagent. **The on-disk sidecar `.meta.json` (618 present) DOES carry `toolUseId`/`agentType`/`description`.** Parent side is healthy: 16072 `tool_call` events; `tool:"Agent"` calls with `call_id` are in the store and queryable.

**Fault B — working/idle from mtime.** `liveness.rs` `partition_claude` decides status `match registry.status { Some("busy")=>Working, Some(_)=>Idle, None => mtime<threshold ? Working : Idle }`. The `None` arm (hit by every Claude < v2.1.187 — 4 of 5 live sessions) uses `transcript_mtime_secs_ago` (`radar/mod.rs:384-396`) vs `radar_working_ms` (default 15000, `util.rs:82`). mtime = "file touched recently," not "agent generating" → wrong verdicts AND racy: the same session flipped working↔idle between two probe reads seconds apart (FSEvents-coalesced writes move mtime independent of real activity).

## Tasks

### Task 1: Subagent linking via a durable signal (TDD)
**Files:** `src-tauri/src/ingest/claude_code.rs` (relink fn ~285-328, persist loop ~264-274), maybe `src-tauri/src/store.rs` (merge helper), test in the same module or `src-tauri/tests/`.
- Make the relink immune to the store-meta clobber: obtain each Claude subagent's `toolUseId` from the **on-disk sidecar `.meta.json`** (derive its path from the session's `source_path`) rather than depending on `s.meta`. Match parent `Agent` (and `Task`, if any) tool_call `call_id` ↔ child `toolUseId`; set `parent_session_id` via the existing `link_child_session`. Keep the existing self-link guard + ClaudeCode-harness + `/subagents/` path filters.
- Repair the durable identity fields too: when a subagent row is missing them, `merge_session_meta` the sidecar's `toolUseId`/`agentType`/`description` back onto the row so the detail-panel Role/name survive the clobber (the same clobber currently nulls `role`). Gate to only-when-missing to bound cost.
- **Failing test first:** build a store with a parent session that has an `Agent` tool_call (`call_id = X`) and a child subagent session whose sidecar/meta `toolUseId = X`; assert that BEFORE the fix `parent_of(child)` is `None`, and AFTER relink it is `Some(parent)`. Then assert via the probe path that the assembled child has a non-null parent / the root reports `childCount > 0`.

### Task 2: working/idle from conversation state, not mtime (TDD)
**Files:** `src-tauri/src/radar/liveness.rs`, `src-tauri/src/radar/mod.rs` (status fn ~373-396), `src-tauri/src/util.rs` (new env helper).
- **Precedence:** (1) registry `status` busy/idle when present → authoritative (unchanged). (2) When absent: derive from the session's **last ingested Event** in the store — if the last event is a completed assistant turn / an answered tool_result ⇒ **idle**; if it is a `UserPrompt`, or a `ToolCall` with no following `ToolResult` ⇒ **working**. Process must be alive (existing pid check). (3) Backstop: if "working" by rule (2) but the session has produced no new event for > `radar_working_stale_secs` (new env helper, generous default — start 180s), downgrade to idle to kill the stuck-forever case. (4) Only if a session has no usable events at all, fall back to the old mtime path.
- This must be **deterministic** across reads (the fix for the flicker): two assembles seconds apart with no new events return the SAME status.
- **Failing test first:** synthesize sessions whose last event is (a) assistant end-of-turn → expect idle, (b) `UserPrompt` → expect working, (c) `ToolCall` without `ToolResult` → expect working; plus a determinism test (assemble twice on an unchanged store → identical statuses). Assert old behavior fails these, new passes.

### Task 3: Verify termination fires + live probe sanity (no new code unless a gap is found)
- With Task 1 landed, subagents now have parents, so `subagent_terminated_at` (`radar/mod.rs`) finally has linked children to evaluate. Confirm the existing terminated path fires for a finished subagent (parent logged the tool-result). If it does not, root-cause and fix minimally.
- Run the live probe on a COPY and report counts:
  `cp ~/.warden/warden.db /tmp/warden_probe.db && WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --manifest-path src-tauri/Cargo.toml --example radar_probe`
  Report: total agents, status breakdown, # subagents with non-null parent (must be > 0 now), childCount distribution, and whether status is stable across two consecutive probe runs.

## Verification (Definition of Done)
- `cd src-tauri && cargo test` green (incl. the 3 new tests).
- Probe on the live-db copy shows **subagent→parent links > 0** and `childCount > 0` on at least one root (was 0/0).
- Status is **stable** across two probe runs seconds apart (flicker gone) and matches registry `status` where present.
- Report written; NO commit, NO push.
