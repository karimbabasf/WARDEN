# RADAR Presence Polish — Backend Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the radar backend name roots by folder + subagents as "subagent N", link live-spawned subagents to their parent reliably, and detect a finished subagent as `terminated` (imploded once, never resurrected).

**Architecture:** All changes live in the Rust radar/ingest layer. Naming and termination are computed in `radar::assemble` (the one place with the full kept-forest context); the subagent→parent link is re-derived every recompute from permanent store facts (parent `Task` tool-call `call_id` ↔ the subagent's persisted `tool_use_id`); termination is derived from the parent's `ToolResult` for that `call_id` (a permanent fact ⇒ idempotent), with a quiet-timeout backstop. The `radar_state` event gains a `"terminated"` status value; everything else in the contract is unchanged.

**Tech Stack:** Rust, rusqlite, chrono, serde; tests via `cargo test`.

## Global Constraints

- Package manager for the repo is **pnpm**; Rust tests run from `src-tauri/` via `cargo test`.
- **M2/M3 are observe-only.** No writes to user projects, ever. We only read transcripts and write WARDEN's own sqlite store.
- **Env-helper convention** (CLAUDE.md): `std::env::var("X").ok().and_then(parse).unwrap_or(default)` in `util.rs`.
- **Harness theme / wire contract:** `RadarAgent.status` is a snake_case string. New value: `"terminated"`. Do not rename existing values (`working`/`idle`/`closed`).
- **Honest viz:** never fabricate a link or a status. An unmatched subagent renders as a root; a still-running subagent is never marked terminated.
- Adding an `AgentStatus` variant requires updating `AgentStatus::as_str` (the only exhaustive match) in `src-tauri/src/radar/liveness.rs`.

---

### Task B0: Diagnose the live-link + termination facts against real data

**Files:**
- Create: `docs/superpowers/notes/2026-06-24-radar-diagnosis.md` (findings log)

**Interfaces:**
- Produces: confirmed answers that calibrate B2–B4 (does child meta carry `toolUseId`? do live subagents get `parent_session_id`? are `ToolResult` rows present for finished `Task` calls?).

- [ ] **Step 1: Locate the live WARDEN db**

Run: `ls -la ~/Library/Application\ Support/*warden* ~/.warden 2>/dev/null; echo "WARDEN_DB_PATH=$WARDEN_DB_PATH"`
Expected: a path to `warden.sqlite` (or the env override). Record it as `$DB`.

- [ ] **Step 2: Check whether any subagent session carries a persisted `toolUseId`**

Run: `sqlite3 "$DB" "SELECT id, json_extract(meta_json,'$.toolUseId') AS tid, json_extract(meta_json,'$.agentType') AS at, parent_session_id FROM sessions WHERE source_path LIKE '%/subagents/%' LIMIT 20;"`
Expected: confirms whether `tid` is NULL today (hypothesis: it is) and whether `parent_session_id` is set for recent subagents.

- [ ] **Step 3: Confirm parent transcripts contain a `Task`/`Agent` tool call AND a matching tool result**

Run: `sqlite3 "$DB" "SELECT json_extract(payload_json,'$') FROM events WHERE 1=0;" 2>/dev/null; sqlite3 "$DB" ".schema events" | head -40`
Then inspect a real Claude transcript directly:
Run: `f=$(ls -t ~/.claude/projects/**/*.jsonl 2>/dev/null | head -1); echo "$f"; grep -o '"type":"tool_result"' "$f" | head -1; grep -o '"name":"Task"' "$f" | head -1`
Expected: confirms tool calls and tool results are both present in the raw transcript (the termination signal exists on disk).

- [ ] **Step 4: Write the findings**

Record in the notes file: (a) is `toolUseId` persisted today? (b) do live subagents link? (c) is the `ToolResult` signal present? These confirm the B2/B3/B4 approach below. If any assumption is wrong, note it in the relevant task before implementing.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/notes/2026-06-24-radar-diagnosis.md
git commit -m "docs(radar): diagnosis of subagent link + termination signals"
```

---

### Task B1: Folder/subagent naming (`display_label`)

**Files:**
- Modify: `src-tauri/src/radar/mod.rs` (add `display_label` + `circled`; wire into `assemble`)
- Test: `src-tauri/src/radar/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn display_label(depth: u32, cwd_basename: Option<&str>, subagent_ordinal: Option<u32>, root_dup_ordinal: Option<u32>, fallback: &str) -> String`

- [ ] **Step 1: Write the failing test**

Add to the tests module in `src-tauri/src/radar/mod.rs`:

```rust
#[test]
fn display_label_names_root_by_folder_and_subagent_by_ordinal() {
    // root with a folder → the folder name
    assert_eq!(display_label(0, Some("WARDEN"), None, None, "fallback"), "WARDEN");
    // a second live root in the same folder → circled disambiguator (oldest keeps bare name)
    assert_eq!(display_label(0, Some("WARDEN"), None, Some(2), "fallback"), "WARDEN ②");
    assert_eq!(display_label(0, Some("WARDEN"), None, Some(1), "fallback"), "WARDEN");
    // root with no folder → falls back to the identity label
    assert_eq!(display_label(0, None, None, None, "diagnose the bug"), "diagnose the bug");
    // subagent → strictly "subagent N", regardless of any role/description
    assert_eq!(display_label(1, Some("WARDEN"), Some(1), None, "Explore"), "subagent 1");
    assert_eq!(display_label(2, None, Some(3), None, "x"), "subagent 3");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd src-tauri && cargo test --lib display_label_names_root`
Expected: FAIL — `cannot find function display_label`.

- [ ] **Step 3: Implement `display_label` + `circled`**

Add near the other private helpers in `src-tauri/src/radar/mod.rs` (e.g. just below `clean_task_label`):

```rust
/// The radar display label. Roots are named by their project folder; subagents by a
/// per-parent ordinal ("subagent N"). `root_dup_ordinal` is `Some(n)` only when
/// several live roots share `cwd_basename` — `n == 1` (the oldest) keeps the bare
/// name, `n >= 2` gets a circled disambiguator. `fallback` is the identity-derived
/// label, used only for a root with no project folder.
fn display_label(
    depth: u32,
    cwd_basename: Option<&str>,
    subagent_ordinal: Option<u32>,
    root_dup_ordinal: Option<u32>,
    fallback: &str,
) -> String {
    if depth >= 1 {
        return format!("subagent {}", subagent_ordinal.unwrap_or(1));
    }
    match cwd_basename {
        Some(name) if !name.is_empty() => match root_dup_ordinal {
            Some(n) if n >= 2 => format!("{name} {}", circled(n)),
            _ => name.to_string(),
        },
        _ => fallback.to_string(),
    }
}

/// Circled-number glyph for 2..=20 (② = U+2461 = U+2460 + (n-1)), else " (n)".
fn circled(n: u32) -> String {
    if (2..=20).contains(&n) {
        char::from_u32(0x2460 + (n - 1))
            .map(|c| c.to_string())
            .unwrap_or_else(|| format!("({n})"))
    } else {
        format!("({n})")
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd src-tauri && cargo test --lib display_label_names_root`
Expected: PASS.

- [ ] **Step 5: Wire `display_label` into `assemble`**

In `src-tauri/src/radar/mod.rs`, inside `assemble`, AFTER `kept_parent` is built and BEFORE the build loop, add the ordinal maps:

```rust
    // Per-parent subagent ordinals (1-based by spawn order) and per-folder root
    // disambiguators (1-based by spawn order) — both over the KEPT set so the
    // numbering is stable and never counts a dropped/closed sibling.
    let started = |id: &str| by_id.get(id).map(|s| (s.started_at, s.id.clone()));
    let mut subagent_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            if let Some(Some(p)) = kept_parent.get(id) {
                by_parent.entry(p.clone()).or_default().push(id.clone());
            }
        }
        for sibs in by_parent.values_mut() {
            sibs.sort_by_key(|id| started(id));
            for (i, id) in sibs.iter().enumerate() {
                subagent_ordinal.insert(id.clone(), (i as u32) + 1);
            }
        }
    }
    let mut root_dup_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            let is_root = kept_parent.get(id).map(|p| p.is_none()).unwrap_or(true);
            if !is_root {
                continue;
            }
            let folder = by_id
                .get(id.as_str())
                .and_then(|s| s.project.as_ref())
                .and_then(|p| p.cwd.file_name())
                .map(|n| n.to_string_lossy().to_string());
            if let Some(folder) = folder {
                by_folder.entry(folder).or_default().push(id.clone());
            }
        }
        for roots in by_folder.values_mut() {
            if roots.len() < 2 {
                continue; // a lone root keeps its bare folder name
            }
            roots.sort_by_key(|id| started(id));
            for (i, id) in roots.iter().enumerate() {
                root_dup_ordinal.insert(id.clone(), (i as u32) + 1);
            }
        }
    }
```

Then, in the build loop, change the push so the label is overridden with the forest-aware name. Replace:

```rust
        agents.push(build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            status,
        ));
```

with:

```rust
        let mut agent = build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            status,
        );
        agent.label = display_label(
            depth,
            agent.cwd.as_deref(),
            subagent_ordinal.get(&s.id).copied(),
            root_dup_ordinal.get(&s.id).copied(),
            &agent.label,
        );
        agents.push(agent);
```

- [ ] **Step 6: Run the full radar test suite**

Run: `cd src-tauri && cargo test --lib radar::`
Expected: PASS (existing assemble tests still green; new naming test green).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/radar/mod.rs
git commit -m "feat(radar): name roots by folder + subagents as 'subagent N'"
```

---

### Task B2: Persist every subagent's `toolUseId` (and role) at ingest

**Files:**
- Modify: `src-tauri/src/ingest/claude_code.rs` (write `toolUseId` for every detected subagent meta, not just matched pairs)
- Test: `src-tauri/src/ingest/claude_code.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `read_subagent_meta -> SubagentMeta { tool_use_id, agent_type, description, agent_id }`, `Store::merge_session_meta(&self, session_id, &serde_json::Value)`.
- Produces: every subagent session row has `meta_json.toolUseId` (+ `agentType`, `description`) set, independent of whether the parent link resolved this pass. This is what B3 (linking) and B4 (termination) read.

- [ ] **Step 1: Find the meta-writing site**

Read `src-tauri/src/ingest/claude_code.rs` around the subagent ingest (the `child_sid_to_meta` map and the `pairs` loop near line 260). Confirm `merge_session_meta` exists with signature `(&self, &str, &serde_json::Value) -> Result<()>` (grep `fn merge_session_meta` in `store.rs`).

- [ ] **Step 2: Write the failing test**

Add to the tests module in `src-tauri/src/ingest/claude_code.rs` (mirror the existing ingest-test fixture style; if helpers like `tmp_store()` exist, reuse them):

```rust
#[test]
fn subagent_meta_persists_tool_use_id_even_without_parent() {
    let store = Store::open_in_memory().expect("store");
    // A subagent transcript with NO parent ingested in this pass.
    let dir = tempfile::tempdir().unwrap();
    let sub_dir = dir.path().join("proj/session-1/subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    std::fs::write(
        sub_dir.join("agent-abc.meta.json"),
        r#"{"agentType":"Explore","description":"map it","toolUseId":"toolu_99"}"#,
    )
    .unwrap();
    std::fs::write(sub_dir.join("agent-abc.jsonl"), claude_min_transcript("child-sid")).unwrap();

    ingest_dir_for_test(&store, dir.path()).expect("ingest");

    let tid: Option<String> = store
        .session_meta_value("child-sid", "toolUseId")
        .unwrap();
    assert_eq!(tid.as_deref(), Some("toolu_99"));
}
```

(If the file has no `claude_min_transcript`/`ingest_dir_for_test`/`session_meta_value` helpers, add minimal versions next to the existing test helpers — a 1-line transcript, a thin wrapper over the real ingest entry point, and `SELECT json_extract(meta_json,'$.'||?)`. Keep them in the test module.)

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd src-tauri && cargo test --lib subagent_meta_persists_tool_use_id`
Expected: FAIL — `toolUseId` is `None`.

- [ ] **Step 4: Implement — persist all subagent metas unconditionally**

In `ingest_all` (claude_code.rs), replace the existing pairs-only meta write:

```rust
    for (child, parent) in &pairs {
        store.link_child_session(child, parent)?;
        if let Some(meta) = child_sid_to_meta.get(child.as_str()) {
            store.merge_session_meta(
                child,
                &json!({ "description": meta.description, "agentType": meta.agent_type }),
            )?;
        }
    }
```

with: persist meta for EVERY detected subagent (so `toolUseId` is always available to the recompute-time linker + terminator), then link the pairs:

```rust
    // Persist each subagent's sidecar fields onto its OWN session row, independent of
    // whether the parent link resolved this ingest pass — `toolUseId` is the key the
    // recompute-time linker (B3) and terminator (B4) match against, so it must be
    // present even before the parent's Task call has been ingested.
    for (child_sid, meta) in &child_sid_to_meta {
        store.merge_session_meta(
            child_sid,
            &json!({
                "description": meta.description,
                "agentType": meta.agent_type,
                "toolUseId": meta.tool_use_id,
            }),
        )?;
    }
    for (child, parent) in &pairs {
        store.link_child_session(child, parent)?;
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd src-tauri && cargo test --lib subagent_meta_persists_tool_use_id`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/ingest/claude_code.rs
git commit -m "feat(ingest): persist subagent toolUseId on its own session row"
```

---

### Task B3: Re-link subagents from the store every recompute

**Files:**
- Modify: `src-tauri/src/ingest/claude_code.rs` (rewrite `link_claude_subagents_in_store` to the store-wide `tool_use_id` path)
- Test: `src-tauri/src/ingest/claude_code.rs`

**Interfaces:**
- Consumes: `Store::sessions`, `Store::session_events`, `Store::link_child_session`, persisted `meta_json.toolUseId` (B2), `Event::ToolCall { tool, call_id, .. }`.
- Produces: `parent_session_id` set for any subagent whose parent's `Task`/`Agent` call exists anywhere in the store — regardless of which ingest pass each arrived in (fixes the live-spawn link).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn relink_resolves_subagent_across_ingest_passes() {
    let store = Store::open_in_memory().expect("store");
    // Pass 1: only the subagent transcript+meta exist (parent's Task call not yet ingested).
    upsert_subagent_session(&store, "child-sid", "toolu_77"); // sets meta.toolUseId, source under /subagents/
    assert_eq!(store.parent_of("child-sid").unwrap(), None, "no parent yet");

    // Pass 2: the parent session arrives carrying the Task tool-call call_id.
    upsert_parent_with_task_call(&store, "parent-sid", "toolu_77");

    let n = link_claude_subagents_in_store(&store).expect("relink");
    assert!(n >= 1);
    assert_eq!(store.parent_of("child-sid").unwrap().as_deref(), Some("parent-sid"));
}
```

(Add `upsert_subagent_session` / `upsert_parent_with_task_call` test helpers next to existing ones: each builds a `SessionBatch`/`Session` + events and calls the real upsert + B2 meta write. Keep them in the test module.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd src-tauri && cargo test --lib relink_resolves_subagent_across_ingest_passes`
Expected: FAIL — the current `SubagentSpawn`-based linker leaves `parent_of` as `None` when the link spans passes.

- [ ] **Step 3: Implement the store-wide re-link**

Replace the body of `link_claude_subagents_in_store` in `claude_code.rs`:

```rust
/// Re-derive Claude subagent→parent links from the WHOLE store (not a single ingest
/// pass): match each subagent's persisted `toolUseId` to the parent session whose
/// transcript contains the `Task`/`Agent` tool-call with that `call_id`. Idempotent
/// — both facts are permanent, so it converges no matter how the writes interleaved.
pub fn link_claude_subagents_in_store(store: &Store) -> Result<usize> {
    let sessions = store.sessions()?;

    // call_id → parent session id, scanned across every Claude session's events.
    let mut call_to_parent: HashMap<String, String> = HashMap::new();
    for s in &sessions {
        if !matches!(s.harness, Harness::ClaudeCode) {
            continue;
        }
        for (_, e) in store.session_events(&s.id).unwrap_or_default() {
            if let Event::ToolCall { tool, call_id, .. } = &e.event {
                if tool == "Agent" || tool == "Task" {
                    call_to_parent.insert(call_id.clone(), s.id.clone());
                }
            }
        }
    }

    let mut recorded = 0;
    for s in &sessions {
        if !matches!(s.harness, Harness::ClaudeCode) {
            continue;
        }
        let is_subagent = s
            .source_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "subagents")
            .unwrap_or(false);
        if !is_subagent {
            continue;
        }
        let tid = s.meta.get("toolUseId").and_then(|v| v.as_str());
        if let Some(parent) = tid.and_then(|t| call_to_parent.get(t)) {
            if parent != &s.id {
                store.link_child_session(&s.id, parent)?;
                recorded += 1;
            }
        }
    }
    Ok(recorded)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd src-tauri && cargo test --lib relink_resolves_subagent_across_ingest_passes`
Expected: PASS.

- [ ] **Step 5: Run the full ingest + radar suites**

Run: `cd src-tauri && cargo test --lib ingest:: && cargo test --lib radar::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/ingest/claude_code.rs
git commit -m "fix(radar): re-link live-spawned subagents from store-wide toolUseId"
```

---

### Task B4: Detect terminated subagents (implode once, no resurrection)

**Files:**
- Modify: `src-tauri/src/radar/liveness.rs` (`AgentStatus::Terminated` + `as_str`)
- Modify: `src-tauri/src/util.rs` (`radar_subagent_terminate_ms`, `radar_terminate_grace_ms`)
- Modify: `src-tauri/src/radar/mod.rs` (`subagent_terminated_at` pure fn + `assemble` integration)
- Test: `src-tauri/src/radar/mod.rs`, `src-tauri/src/radar/liveness.rs`

**Interfaces:**
- Consumes: `Event::ToolResult { call_id, .. }`, persisted `meta.toolUseId`, `kept_parent`, `Store::session_events`, `transcript_mtime_secs_ago`.
- Produces: a kept subagent with a terminated signal is emitted with `status:"terminated"` for a grace window, then excluded from the forest on later recomputes (permanent fact ⇒ never re-appears).

- [ ] **Step 1: Add the `Terminated` status (failing test first)**

In `src-tauri/src/radar/liveness.rs` tests module:

```rust
#[test]
fn agent_status_terminated_wire_value() {
    assert_eq!(AgentStatus::Terminated.as_str(), "terminated");
}
```

Run: `cd src-tauri && cargo test --lib agent_status_terminated_wire_value`
Expected: FAIL — no `Terminated` variant.

Then add the variant + arm:

```rust
pub enum AgentStatus {
    Working,
    Idle,
    Closed,
    Terminated,
}
```
```rust
            AgentStatus::Closed => "closed",
            AgentStatus::Terminated => "terminated",
```

Run again → PASS.

- [ ] **Step 2: Add the env helpers**

In `src-tauri/src/util.rs`, copy the `radar_working_ms` pattern:

```rust
/// RADAR: how long a subagent may be silent (no transcript writes) while its parent
/// is still alive before it is treated as terminated — a BACKSTOP only; the primary
/// signal is the parent's tool-result for the subagent's call. `WARDEN_RADAR_SUBAGENT_TERMINATE_MS`
/// overrides. Generous default (90s) so a long-running tool call is never mistaken
/// for a finished subagent.
pub fn radar_subagent_terminate_ms() -> u64 {
    std::env::var("WARDEN_RADAR_SUBAGENT_TERMINATE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(90_000)
}

/// RADAR: how long a terminated subagent stays in the emitted forest (status
/// "terminated") so the FACE can play its implode, before it is dropped. Derived
/// from the permanent termination timestamp, so dropping is idempotent.
pub fn radar_terminate_grace_ms() -> u64 {
    std::env::var("WARDEN_RADAR_TERMINATE_GRACE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(5_000)
}
```

- [ ] **Step 3: Write the failing test for the pure `subagent_terminated_at`**

In `src-tauri/src/radar/mod.rs` tests:

```rust
#[test]
fn subagent_terminated_at_uses_result_then_timeout() {
    use crate::ir::{Event, ToolStatus};
    let now = Utc::now();
    let result_ts = now - chrono::Duration::seconds(2);
    let parent_events = vec![mk_tool_result_event("toolu_5", result_ts)];

    // Primary: a matching tool-result → terminated at the result's ts.
    assert_eq!(
        subagent_terminated_at(Some("toolu_5"), &parent_events, Some(now), now, 90_000),
        Some(result_ts)
    );
    // No result, recently active → still live.
    assert_eq!(
        subagent_terminated_at(Some("toolu_x"), &[], Some(now - chrono::Duration::seconds(3)), now, 90_000),
        None
    );
    // No result, silent past the backstop → terminated at (last + timeout).
    let last = now - chrono::Duration::seconds(200);
    assert_eq!(
        subagent_terminated_at(Some("toolu_x"), &[], Some(last), now, 90_000),
        Some(last + chrono::Duration::milliseconds(90_000))
    );
    // No tool_use_id and no last activity → never terminated.
    assert_eq!(subagent_terminated_at(None, &[], None, now, 90_000), None);
}
```

(Add a tiny `mk_tool_result_event(call_id, ts)` test helper building a `(Turn, EventRecord)` with `Event::ToolResult { call_id, status: ToolStatus::Ok, bytes: 0, summary: None }`, mirroring the fixture style in `hierarchy.rs` tests.)

- [ ] **Step 4: Run the test to verify it fails**

Run: `cd src-tauri && cargo test --lib subagent_terminated_at_uses_result_then_timeout`
Expected: FAIL — function not defined.

- [ ] **Step 5: Implement `subagent_terminated_at`**

Add to `src-tauri/src/radar/mod.rs`:

```rust
/// When a subagent became terminated, or `None` if still live.
/// Primary: the parent logged a tool RESULT for the subagent's `tool_use_id` →
/// terminated at the result's timestamp (a permanent transcript fact ⇒ idempotent
/// across recomputes). Backstop: no result, but the subagent has been silent longer
/// than `terminate_ms` while its parent is alive → terminated at `last + terminate_ms`.
fn subagent_terminated_at(
    tool_use_id: Option<&str>,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    child_last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    terminate_ms: u64,
) -> Option<DateTime<Utc>> {
    if let Some(tid) = tool_use_id {
        if let Some(ts) = parent_events.iter().rev().find_map(|(_, e)| match &e.event {
            Event::ToolResult { call_id, .. } if call_id == tid => Some(e.ts),
            _ => None,
        }) {
            return Some(ts);
        }
    }
    let last = child_last_activity?;
    let quiet_ms = now.signed_duration_since(last).num_milliseconds().max(0) as u64;
    (quiet_ms > terminate_ms).then(|| last + chrono::Duration::milliseconds(terminate_ms as i64))
}
```

Run: `cd src-tauri && cargo test --lib subagent_terminated_at_uses_result_then_timeout`
Expected: PASS.

- [ ] **Step 6: Integrate termination into `assemble`**

In `assemble`, AFTER `kept_parent` is built and BEFORE the ordinal maps from Task B1 (so excluded subagents never get numbered or counted), insert:

```rust
    // ── subagent termination ─────────────────────────────────────────────────────
    // A subagent has no PID, so liveness can't see it finish. Derive it: the parent
    // logged a tool-result for the subagent's call (permanent ⇒ idempotent), or the
    // subagent fell silent past the backstop. Within a grace window we EMIT it as
    // `terminated` (the FACE implodes it); past the window we DROP it from the forest
    // so it never lingers as idle and never resurrects.
    let terminate_ms = crate::util::radar_subagent_terminate_ms();
    let grace_ms = crate::util::radar_terminate_grace_ms();
    let mut terminated_now: HashSet<String> = HashSet::new();
    let mut terminated_drop: HashSet<String> = HashSet::new();
    for id in &keep {
        let Some(Some(parent)) = kept_parent.get(id) else {
            continue; // roots are never "terminated" (they Close instead)
        };
        let Some(child) = by_id.get(id.as_str()) else { continue };
        let tid = child.meta.get("toolUseId").and_then(|v| v.as_str());
        let parent_events = store.session_events(parent).unwrap_or_default();
        let last = mtime_secs_ago(&child.external_id)
            .map(|secs| now - chrono::Duration::seconds(secs as i64));
        if let Some(ts) = subagent_terminated_at(tid, &parent_events, last, now, terminate_ms) {
            let age_ms = now.signed_duration_since(ts).num_milliseconds().max(0) as u64;
            if age_ms <= grace_ms {
                terminated_now.insert(id.clone());
            } else {
                terminated_drop.insert(id.clone());
            }
        }
    }
    // Drop past-grace terminated subagents from the forest entirely (BEFORE counts).
    if !terminated_drop.is_empty() {
        keep.retain(|id| !terminated_drop.contains(id));
        kept_parent.retain(|id, _| !terminated_drop.contains(id));
    }
```

Make `keep` and `kept_parent` mutable (`let mut keep` / `let mut kept_parent`). Recompute `child_count` AFTER this block (it already runs after `kept_parent`; ensure its loop now iterates the pruned `keep`). In the build loop, change the status to honor the override:

```rust
        let status = if terminated_now.contains(&s.id) {
            AgentStatus::Terminated
        } else {
            agent_status(s, &claude_status, &mtime_secs_ago)
        };
```

- [ ] **Step 7: Write an integration test for idempotent drop**

```rust
#[test]
fn terminated_subagent_is_emitted_once_then_dropped_and_never_resurrects() {
    // Build a store: a live Claude root + a subagent whose parent has a tool-result
    // for its call. First assemble (result fresh) → subagent present as "terminated".
    // Assemble again with `now` advanced past the grace → subagent absent, and a third
    // assemble keeps it absent (no resurrection).
    let store = Store::open_in_memory().unwrap();
    seed_root_with_terminated_subagent(&store, "root", "sub", "toolu_1");
    let dir = tempfile::tempdir().unwrap();
    let alive = |_pid: u32| true;
    let codex_open = |_s: &Session| false;

    let t0 = result_ts_of(&store, "root", "toolu_1"); // helper: the result event ts
    let s1 = assemble(&store, dir.path(), &alive, &codex_open, t0 + chrono::Duration::seconds(1));
    let sub = s1.agents.iter().find(|a| a.id == "sub").expect("present within grace");
    assert_eq!(sub.status, "terminated");

    let s2 = assemble(&store, dir.path(), &alive, &codex_open, t0 + chrono::Duration::seconds(30));
    assert!(s2.agents.iter().all(|a| a.id != "sub"), "dropped past grace");
    let s3 = assemble(&store, dir.path(), &alive, &codex_open, t0 + chrono::Duration::seconds(60));
    assert!(s3.agents.iter().all(|a| a.id != "sub"), "stays dropped (no resurrection)");
}
```

(Add `seed_root_with_terminated_subagent` + `result_ts_of` helpers mirroring existing assemble-test seeding. If assemble tests already seed a store, reuse that scaffolding.)

Run: `cd src-tauri && cargo test --lib terminated_subagent_is_emitted_once`
Expected: FAIL first if helpers missing, then PASS once implemented.

- [ ] **Step 8: Full suite + typecheck**

Run: `cd src-tauri && cargo test`
Expected: PASS (all radar/ingest/liveness tests green).

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/radar/liveness.rs src-tauri/src/util.rs src-tauri/src/radar/mod.rs
git commit -m "feat(radar): terminate finished subagents (result-signal + timeout backstop)"
```

---

### Task B5: Live sanity + liveness threshold check

**Files:**
- Modify (only if data shows a wrong threshold): `src-tauri/src/util.rs` (`radar_working_ms` default)

**Interfaces:**
- Consumes: the running daemon's `radar_state`.

- [ ] **Step 1: Build**

Run: `cd src-tauri && cargo build`
Expected: clean build.

- [ ] **Step 2: Inspect a live recompute**

With at least one Claude session running and one idle, and (ideally) a subagent that just finished, run the existing radar probe if present (grep `radar_probe` in `src-tauri`), else add a 1-off `#[ignore]` test that calls `recompute_radar_state` against the real db and prints `id, label, status, depth, parent_id` per agent.

Run: `cd src-tauri && cargo test --lib radar_probe -- --ignored --nocapture` (or the existing probe command)
Expected: roots show folder names; a finished subagent shows `terminated` then disappears on the next probe; a genuinely working agent shows `working`, an idle one `idle`.

- [ ] **Step 3: Tune only if needed**

If a known-working agent reads `idle` (or vice-versa), adjust `radar_working_ms` default and note why in the commit. Otherwise leave it.

- [ ] **Step 4: Commit (if changed)**

```bash
git add -A && git commit -m "chore(radar): verified live statuses; tune working window"
```

---

## Self-Review

- **Spec coverage:** Naming (B1), live linking (B2+B3), termination/implode-once (B4), liveness accuracy (B5). The frontend half of termination (treating `terminated` as implode) lives in the frontend plan, Task F1.
- **Type consistency:** `display_label`, `subagent_terminated_at`, `AgentStatus::Terminated`, `radar_subagent_terminate_ms`, `radar_terminate_grace_ms` are used with identical signatures everywhere referenced.
- **No placeholders:** test-helper additions are explicitly called out where the existing fixtures don't already provide them; reuse existing scaffolding when present.
