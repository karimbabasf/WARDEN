# RADAR Presence Polish — B0 Diagnosis (against real data)

Date: 2026-06-24. Method: inspected the live WARDEN sqlite store + the real
`~/.claude/projects` transcripts in a sandbox (no raw bytes pulled into context).
Cross-checked the Rust IR/store/radar/ingest source for the exact shapes the
plan's B1–B4 depend on.

## Environment facts

- **Live db path:** `~/.warden/warden.db` (143 MB). `WARDEN_DB_PATH` unset; the
  default is `~/.warden/warden.db` (`util::default_db_path`) — NOT
  `~/Library/Application Support/...` and NOT a `.sqlite` extension. The plan's
  Step-1 path glob found nothing for that reason; the real default is the
  `.warden/warden.db` fallback.
- Store has 1081 sessions; **722** of them are subagent rows
  (`source_path LIKE '%/subagents/%'`).
- Tauri identifier: `ai.sakana.warden` (not used for the db path).

## The three plan hypotheses — all CONFIRMED

### (a) Is `toolUseId` persisted on the subagent row today? → NO (confirmed)

```
subagent rows total .................. 722
   with meta.toolUseId IS NOT NULL ...   0
   with parent_session_id ............   0
```

Every subagent row has empty `meta.toolUseId` and NULL `parent_session_id`.
So **B2 is genuinely required** — `toolUseId` must be written onto the child row
at ingest, unconditionally (today only `description`+`agentType` are merged, and
only for pairs that linked in the same pass).

### (b) Do live-spawned subagents link to their parent today? → NO (confirmed)

0 of 722 subagent rows are linked. The current linker
(`link_claude_subagents_in_store`) derives links from the parent's
`SubagentSpawn.child_session`, which is only filled when parent + child are
ingested in the *same* pass (`hierarchy::link_claude_subagents` over the
in-memory batches). Across passes (the live-spawn case: child transcript appears,
parent's Agent tool-call arrives later, or vice-versa) the pointer stays `None`
and no link is recorded. **B3 (store-wide re-link keyed on persisted toolUseId)
is the correct fix.**

### (c) Is the `ToolResult` termination signal present on disk/in the store? → YES (confirmed)

The correlation that B3/B4 rest on holds on real data. Sampling 40 subagent
`*.meta.json` `toolUseId`s and matching them against db events:

```
subagent toolUseId -> parent tool_call with call_id == toolUseId : 40/40 hits, 0 misses
subagent toolUseId -> parent tool_result with same call_id       : 40/40 present
```

So the parent transcript carries BOTH the dispatch (`tool_call`) and the
completion (`tool_result`) keyed by exactly the subagent's `toolUseId`. The
termination signal B4 needs is a permanent transcript fact and is present.

## Shape facts that calibrate the implementation (deviations from plan snippets)

1. **Tool name is `Agent`, not `Task`, on this machine.** 446 `Agent` tool calls,
   **0** `Task`. Claude Code's subagent dispatch tool is `Agent` here. The plan's
   matcher checks `tool == "Agent" || tool == "Task"` — keep BOTH (Task may appear
   on other setups / older versions), but the live signal is `Agent`. (Confirmed
   the existing `hierarchy.rs` linker also matches on `ToolCall` `call_id`, same key.)

2. **`Event::ToolCall` DOES have `call_id`** (ir.rs:130) — the B3 destructure
   `Event::ToolCall { tool, call_id, .. }` compiles. `Event::ToolResult` has
   `{ call_id, status, bytes, summary }` (ir.rs:133). `EventRecord` carries both
   `event: Event` and `ts: DateTime<Utc>`, so `e.event` / `e.ts` in the B4 snippet
   are valid.

3. **`Store::session_meta_value` does NOT exist** (store.rs has `merge_session_meta`,
   `sessions`, `session_events`, `parent_of`, `link_child_session` — no
   `session_meta_value`). The plan's B2/B3 test snippets call
   `store.session_meta_value(...)` / a non-existent constructor. **Deviation:** read
   meta in tests via `store.sessions()` → find by id → `session.meta.get("toolUseId")`.
   Do NOT add a store.rs method (out of scope).

4. **In-memory store constructor is `Store::memory()`**, not `Store::open_in_memory()`
   (radar + ingest tests both use `Store::memory()`). The plan snippets say
   `Store::open_in_memory()`. **Deviation:** use `Store::memory()`.

5. **`merge_session_meta` exists and is a REPLACE-merge of top-level keys**
   (store.rs:129, patches each top-level key into `meta_json`). Good — no store
   change needed for B2.

6. **`link_claude_subagents_in_store` is `pub` with a single caller**:
   `radar/mod.rs:783` inside `recompute_radar_state`. B3 must preserve the name +
   signature `pub fn link_claude_subagents_in_store(&Store) -> Result<usize>` and
   only rewrite the body (which the plan does).

7. **`Harness` variants:** `ClaudeCode`, `Codex`, `Cursor`, `Hermes`,
   `Generic(String)`. B3/B4 gate on `Harness::ClaudeCode`. ✓

8. **events payload JSON is internally-tagged with flat fields**
   (`{"event_type":"tool_call","tool":"Agent","call_id":"toolu_..","input":{..}}`),
   not nested under a `ToolCall` key. Irrelevant to the Rust code (it deserializes
   to the `Event` enum), but noted so the diagnosis queries used `$.tool`/`$.call_id`.

## Conclusion

The plan's architecture is sound and matches reality: persist `toolUseId` (B2),
re-link store-wide on that key (B3), terminate from the parent's `tool_result`
with a quiet-timeout backstop (B4). Implement with the constructor/meta-read
deviations above (Store::memory, read meta via sessions(), no session_meta_value,
no store.rs edits). The live-link bug is real and currently affects 100% of
subagents (0/722 linked).
