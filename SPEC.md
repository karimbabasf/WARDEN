# WARDEN — Master Design Spec

> *The agent that watches your agents.*
> A always-on macOS daemon that observes every coding agent on your machine, learns where your agentic workflow leaks, and coaches you in real time — powered by Sakana **Fugu** multi-agent orchestration.

| | |
|---|---|
| **Status** | Draft v1 — approved design, full spec |
| **Date** | 2026-06-22 |
| **Codename** | WARDEN *(placeholder; alternates: GHOST, OVERSEER)* |
| **Owner** | — |
| **Spec type** | Umbrella master spec. Decomposes into 7 milestone sub-specs (§19), each of which gets its own implementation plan. |
| **Build target** | macOS (Apple Silicon first), Tauri v2 (Rust core + web overlay) |
| **Engine** | Sakana Fugu (`api.sakana.ai/v1`, OpenAI-compatible) |

---

## 0. Executive summary

Everyone now runs a *fleet* of coding agents — Claude Code, Codex, Cursor, Hermes, and a growing zoo of CLI agents — and nobody can see how well they're using them. The agentic workflow is a black box. You don't know where you burn tokens, where your prompts are vague, where context detonates, where you re-explain the same architecture every session, where an agent silently fails and you don't notice.

**WARDEN** is a passive, always-on daemon that:

1. **Watches** every agent harness on your machine via pluggable adapters, normalizing all of them into one canonical event model.
2. **Learns** your recurring failure patterns ("holes") by featurizing your real transcripts and confirming findings with a Fugu war room.
3. **Coaches** you — a ranked diagnosis of what to do and what to stop doing, delivered through a green-on-black terminal overlay summoned by a global hotkey, answerable by voice.
4. **Interjects** live — hooks catch you repeating a known hole mid-session and whisper a warning in the moment.
5. **Forges** fixes — turns each diagnosed hole into an approvable diff: a better `CLAUDE.md`/`AGENTS.md` block, a custom skill, a hook, a prompt template. Your own transcripts become the spec for the tools that make you better.

**Why this is not a UI-on-an-API.** The value is in (a) the multi-harness ingestion + normalization layer, (b) a deterministic featurizer over agent transcripts, (c) an anti-pattern taxonomy with detectors, (d) live hook IPC, and (e) artifact generation that writes real files. The LLM is one organ (the Brain), not the whole body. And the Brain genuinely needs Fugu: "diagnose this messy multi-session agentic workflow and tell me what to change" is a hard, quality-critical reasoning task where learned multi-agent orchestration beats any single model. The poetry is exact — **a war room of agents, dissecting your agents** — and we visualize it honestly using Fugu's `orchestration_tokens` usage fields.

**The jaw-drop** (full script in §20): hotkey → black screen, green cursor → you say *"what's wrong with how I use my agents?"* → a war-room visualizer lights up as Fugu orchestrates → a ranked diagnosis slams in with a one-keystroke fix that writes itself into your project.

---

## 1. Goals / non-goals

### 1.1 Goals (v1)
- **G1.** Ingest Claude Code, Codex, Cursor, Hermes, and generic agent CLIs into one normalized model.
- **G2.** A deterministic featurizer producing a per-session feature vector + a cross-session **competence profile**.
- **G3.** A Fugu pipeline (Diagnostician → Coach → Verifier) that turns features + transcript evidence into a **ranked, verified diagnosis**.
- **G4.** A green/black terminal overlay (Tauri), summoned by a global hotkey, with a live **war-room visualizer** of Fugu orchestration.
- **G5.** **Voice** in/out (local STT + TTS) and **screen Q&A** ("what's on my screen?").
- **G6.** **Live interjection** via hooks for Claude Code & Codex.
- **G7.** **The Forge** — generate & apply fix artifacts as approvable diffs.
- **G8.** 100% on-device storage; the only data that leaves the machine is what's explicitly sent to Fugu, under user control.

### 1.2 Non-goals (v1)
- ✗ Cross-machine sync / cloud account / team dashboards. (Single-device, local.)
- ✗ Windows / Linux. (macOS Apple-Silicon first; architecture stays portable.)
- ✗ Auto-applying fixes without approval. (Always diff → approve.)
- ✗ Any `git push`, MR/PR creation, or outbound publishing. (Hard rule.)
- ✗ Replacing the agents. WARDEN observes and coaches; it never drives the agents itself.
- ✗ Fine-tuning / training a model. The "learning" is a persistent, sharpening profile + cached findings, not model training.

### 1.3 Success criteria
- On a cold machine with existing transcripts, WARDEN produces a **verified, evidence-cited diagnosis of the top 3 holes** within one run, each with a concrete, applyable fix.
- Live interjection fires within **<500 ms** of a hooked event for deterministic checks.
- Overlay summon-to-first-frame **<150 ms**; war-room reflects real Fugu orchestration tokens.
- Adding a new harness = writing one adapter; **zero changes** to featurizer or Brain.

---

## 2. System overview

```
                          ┌──────────────────────────────────────────────┐
                          │                  WARDEN daemon (Rust)         │
   on-disk transcripts    │                                              │
  ┌───────────────┐       │   ┌─────────┐   normalized    ┌───────────┐  │
  │ Claude Code   │──tail─┼──▶│ EYES    │── AgentEvent ───▶│  MEMORY   │  │
  │ Codex         │──tail─┼──▶│ adapters│      IR          │ SQLite+   │  │
  │ Cursor (sqlite)│─poll─┼──▶│ registry│                  │ FTS5      │  │
  │ Hermes (API)  │──poll─┼──▶│         │                  └─────┬─────┘  │
  │ Generic CLIs  │──tail─┼──▶└─────────┘                        │        │
  └───────────────┘       │                                      ▼        │
                          │                              ┌──────────────┐ │
        live hooks        │   ┌──────────────┐  features │ FEATURIZER   │ │
  ┌───────────────┐  POST │   │ Scheduler    │◀──────────│ + detectors  │ │
  │ CC/Codex hook │──────▶┼──▶│ (idle/close/ │           └──────┬───────┘ │
  └───────────────┘ socket│   │  ondemand)   │   candidates     │         │
                          │   └──────┬───────┘                  ▼         │
                          │          │              ┌──────────────────┐  │
                          │          ▼              │ BRAIN (Fugu)     │  │
                          │   ┌──────────────┐ SSE  │ Diagnostician →  │──┼──▶ api.sakana.ai
                          │   │ Fugu client  │◀────▶│ Coach → Verifier │  │
                          │   └──────────────┘      └────────┬─────────┘  │
                          │                                  │ diagnosis  │
                          │   ┌──────────────┐               ▼            │
                          │   │ FORGE        │◀──── findings ─┘           │
                          │   │ artifact gen │                            │
                          │   └──────────────┘                            │
                          └───────────┬──────────────────┬───────────────┘
                            IPC (commands/events)         │ global hotkey, screen, audio
                                      │                   │
                          ┌───────────▼───────────────────▼───────────────┐
                          │            FACE (web overlay, Tauri webview)   │
                          │  terminal renderer · war-room viz · VOICE      │
                          │  green-on-black · anime.js · WebGL             │
                          └────────────────────────────────────────────────┘
```

**Data flow (steady state):** adapters tail/poll sources → normalize to IR → persist in SQLite → featurizer computes per-session features + updates profile → deterministic detectors nominate candidate findings → scheduler batches candidates → Brain (Fugu) confirms/ranks/verifies → diagnosis persisted → Forge generates fix artifacts → FACE renders, user asks/approves → artifacts applied to real files.

**The six organs:** EYES (ingest), MEMORY (store), BRAIN (Fugu analysis), VOICE (interaction), FORGE (artifacts), **RADAR** (fleet locate + navigate — §9A). Live interjection and the Scheduler are cross-cutting runtime services.

**RADAR's parallel data flow (operational, real-time):** a Fleet Tracker continuously reconciles *running process ↔ session ↔ physical on-screen location ↔ live status* for every agent on the machine (independent of the analytical EYES→BRAIN path above), so the user can **see every agent and Warp to any one of them**. It shares `SessionId` with EYES, so the fleet view can show live findings against a running agent.

---

## 3. Core data model — the canonical IR

The keystone. **Every adapter maps into this; the featurizer and Brain read *only* this.** Adding a harness never touches anything downstream. (Rust types shown; serialized to SQLite + serde-JSON.)

```rust
enum Harness { ClaudeCode, Codex, Cursor, Hermes, Generic(String) }

struct Session {
    id: SessionId,              // stable hash(harness + external_id + source_path)
    harness: Harness,
    external_id: String,        // harness-native session id
    project: Option<ProjectRef>,// cwd / repo root / git branch
    model_ids: Vec<String>,     // models seen in this session
    started_at: DateTime, ended_at: Option<DateTime>,
    source_path: PathBuf,       // file or db locator
    raw_hash: u64,              // content hash of source slice (idempotency)
    ingested_at: DateTime,
    meta: JsonMap,              // harness-specific extras (lossless escape hatch)
}

struct ProjectRef { cwd: PathBuf, repo_root: Option<PathBuf>, git_branch: Option<String> }

struct Turn {
    id: TurnId, session_id: SessionId,
    parent_id: Option<TurnId>,  // from parentUuid → builds the turn DAG
    role: Role,                 // User | Assistant | System | Tool
    index: u32,
    started_at: DateTime, duration_ms: Option<u64>,
    is_sidechain: bool,         // subagent turn (CC isSidechain)
}

enum Event {
    UserPrompt   { text: String, attachments: Vec<Attachment>, is_meta: bool },
    AssistantText{ text: String },
    Thinking     { tokens: u32 },                       // reasoning/plan signal
    ToolCall     { tool: String, input: Json, call_id: String,
                   kind: ToolKind },                    // Builtin | Mcp | Subagent(Task)
    ToolResult   { call_id: String, status: ToolStatus, // Ok | Error
                   bytes: u64, summary: Option<String> },
    TokenUsage   { input: u32, output: u32,
                   cache_creation: u32, cache_read: u32,
                   model: String,
                   orchestration: Option<Orchestration> }, // Fugu only
    FileSnapshot { files: Vec<FileEdit> },              // CC file-history-snapshot
    SubagentSpawn{ source_assistant_uuid: String, child_session: Option<SessionId> },
    ModeChange   { mode: String },                      // permission/plan mode
    Error        { source: String, message: String },
    SystemNotice { subtype: String, data: Json },       // turn_duration, away_summary…
}

struct EventRecord { id: EventId, turn_id: TurnId, session_id: SessionId,
                     ts: DateTime, event: Event, raw_ref: RawRef }
```

`raw_ref` is a `(source_path, byte_offset|rowid)` pointer back to the original record — every normalized event is traceable to ground truth for evidence citation and audits. WARDEN never discards the original; it indexes it.

---

## 4. EYES — ingest adapters

### 4.1 Adapter interface

```rust
trait Adapter {
    fn harness(&self) -> Harness;
    fn detect(&self) -> Vec<SourceLocator>;          // discover sources on disk/API
    fn backfill(&self, since: Watermark) -> Stream<Item = SessionBatch>;
    fn watch(&self, tx: Sender<AgentEvent>);          // live tail / poll
    fn map(&self, raw: RawRecord) -> Vec<EventRecord>;// raw → IR
}
```

- **Registry** loads all enabled adapters; each runs in its own Tokio task.
- **Idempotency:** every source slice carries `raw_hash`; re-ingesting an unchanged slice is a no-op. **Watermarks** (per source: last byte offset / last rowid / last API cursor) persist in MEMORY so restart resumes, never re-processes.
- **Backpressure:** bounded channel; adapters block rather than balloon memory on a 155 MB backfill.

### 4.2 Claude Code adapter *(first to ship — ground truth confirmed)*

**Source:** `~/.claude/projects/<slug>/<sessionId>.jsonl` (one JSONL per session). 8 projects / 168 files / 155 MB confirmed on the dev machine.

**Live:** `notify` watch on `~/.claude/projects/**`; on append, read from the persisted byte-offset watermark to EOF, parse new lines.

**Record types → IR map** (confirmed schema):

| JSONL `type` | Top-level fields (confirmed) | → IR |
|---|---|---|
| `user` | `parentUuid, isSidechain, userType, cwd, sessionId, version, gitBranch, message, uuid, timestamp, isMeta` | `Turn{role:User, parent_id:parentUuid, is_sidechain}` + `UserPrompt` (content `string` or `array[tool_result]` → `ToolResult` when array) |
| `assistant` | `parentUuid, message{role,content,usage,model}, sourceToolAssistantUuid, uuid, timestamp` | `Turn{role:Assistant}` + per content block: `text`→`AssistantText`, `thinking`→`Thinking`, `tool_use`→`ToolCall`, `server_tool_use`→`ToolCall{kind:Mcp}`; `message.usage`→`TokenUsage` |
| `system` | `subtype` (`turn_duration`, `away_summary`), data | `SystemNotice`; `turn_duration` populates `Turn.duration_ms` |
| `file-history-snapshot` | `messageId, snapshot, isSnapshotUpdate` | `FileSnapshot{files}` (drives file-churn / thrash features) |
| `mode` / `permission-mode` | `mode` / `permissionMode, sessionId` | `ModeChange` |
| `attachment` | attachment payload | `UserPrompt.attachments` |
| `last-prompt`, `ai-title`, `queue-operation` | session meta | `Session.meta` |

**Content block parsing** (`message.content` array): `{type:"text"}`, `{type:"thinking"}`, `{type:"tool_use", name, input, id}`, `{type:"server_tool_use"}`, and in user turns `{type:"tool_result", tool_use_id, content, is_error}`.

**Usage** (`message.usage`): `input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, service_tier` → `TokenUsage`.

**Subagent reconstruction:** `isSidechain:true` marks subagent (Task tool) turns; `sourceToolAssistantUuid` links a sidechain back to the spawning assistant turn → emit `SubagentSpawn`, build the delegation tree. **This is what makes the `NO_DELEGATION` and `CONTEXT_BLOAT` detectors possible.**

**Project context:** `cwd` + `gitBranch` → `ProjectRef`; repo root resolved by walking up for `.git`.

### 4.3 Codex adapter

**Source:** `~/.codex/sessions/**` + `~/.codex/archived_sessions/**`, JSONL. Confirmed record envelope: `{timestamp, type, payload}` with `type ∈ {session_meta, event_msg, response_item, turn_context}`.

| `type` | → IR |
|---|---|
| `session_meta` | `Session` header (id, model, cwd) |
| `turn_context` | `Turn` boundary + project context |
| `event_msg` | user/assistant messages → `UserPrompt`/`AssistantText` |
| `response_item` | tool calls / results / reasoning inside `payload` → `ToolCall`/`ToolResult`/`Thinking` |

Adapter inspects `payload` shape per `type` (payload schema is Codex-internal; mapped defensively with an unknown→`SystemNotice` fallback so schema drift never drops a session).

### 4.4 Cursor adapter

**Source:** SQLite. `globalStorage/state.vscdb` + per-workspace `workspaceStorage/<id>/state.vscdb`. Tables confirmed: `ItemTable(key,value)` and `cursorDiskKV(key,value)`.

**Thread model** (confirmed keys):
- `ItemTable['composer.composerHeaders']` → JSON list of all composer threads (index, ~430 KB).
- `cursorDiskKV['composerData:<composerId>']` → thread metadata.
- `cursorDiskKV['bubbleId:<composerId>:<bubbleId>']` → individual messages (the turns).
- `cursorDiskKV['agentKv:<…>']` → agent run state; `ofsContent:` / `codeBlockPartialInlineDiffFates:` → applied-edit signals.

**Read strategy:** SQLite is single-writer and Cursor holds the file; open **read-only + immutable** (`file:<db>?mode=ro&immutable=1`) to avoid locks and WAL surprises. **Poll** (default 15 s) diffing `composerHeaders` + new `bubbleId:*` rows against a rowid/hash watermark (no file-append signal like JSONL). Map bubbles → `Turn`+events; `ofsContent`/diff-fate rows → `FileSnapshot`.

### 4.5 Hermes adapter *(cloud / API class)*

`hermes-agent.nousresearch.com` is a hosted SPA agent — no local transcript files. Adapter is **API-class**:
- Config: optional Hermes API base + key (Keychain). If unset → adapter disabled (gracefully).
- `detect()` probes for a session/history endpoint; `backfill()`/`watch()` poll it with a cursor watermark.
- Because the exact Hermes history schema is not publicly documented, the adapter ships with a **schema-discovery mapper**: it introspects returned JSON, maps the obvious role/content/tool/usage fields, and dumps the rest into `meta` (lossless). Refined as the API is confirmed during M7. *(This is the one adapter whose remote schema is assumed, not verified — flagged in §21.)*

### 4.6 Generic adapter

Auto-detects other agent CLIs ("open claw" set — opencode, Crush, Aider, etc.) by scanning known config dirs (`~/.config/<tool>`, `~/.<tool>`) and a user-extensible registry (`config.toml [[generic_adapter]]`). For each, a declarative mapping (jsonpath-style) describes role/content/tool/usage extraction from its JSONL/log. Unknown harnesses get a best-effort line-based importer that at minimum captures prompts and timing.

---

## 5. MEMORY — the local store

**SQLite** (bundled, via `rusqlite`) at `~/.warden/warden.db`. WAL mode. FTS5 for transcript/profile search (mirrors the team's existing context-mode pattern). Everything on-device.

```sql
sessions(id, harness, external_id, project_json, model_ids_json,
         started_at, ended_at, source_path, raw_hash, ingested_at, meta_json)
turns(id, session_id, parent_id, role, idx, started_at, duration_ms, is_sidechain)
events(id, turn_id, session_id, ts, kind, payload_json, raw_ref)
events_fts USING fts5(text, content='events')          -- prompt/result search
watermarks(source_path PRIMARY KEY, offset, rowid, cursor, updated_at)

features(session_id PRIMARY KEY, vector_json, computed_at, featurizer_version)
profile(id=1, vector_json, updated_at)                  -- the evolving "you-as-operator"
profile_history(ts, vector_json)                        -- trend over time

findings(id, pattern_id, session_ids_json, severity, frequency,
         est_cost_tokens, est_cost_minutes, confidence,
         evidence_json, status, created_at)              -- status: candidate|confirmed|refuted|fixed
diagnoses(id, created_at, ranked_findings_json, do_json, stop_json, narrative)
artifacts(id, finding_id, kind, target_path, diff, status, applied_at, backup_path)
                                                         -- status: proposed|applied|reverted
fugu_runs(id, stage, model, effort, req_hash, input_tokens, output_tokens,
          orchestration_input_tokens, orchestration_output_tokens,
          latency_ms, cost_usd, created_at)              -- cost ledger + war-room source
interjections(id, ts, pattern_id, session_id, shown, dismissed, muted)
```

- **Retention:** raw events kept indefinitely by default; configurable cap (e.g. prune raw event bodies older than N days, keep features/findings). Source files are never modified or deleted by WARDEN.
- **Encryption at rest (option):** SQLCipher-backed DB behind a Keychain-stored key.
- **Migrations:** versioned, forward-only; `featurizer_version`/`schema_version` gate re-computation.

---

## 6. The Featurizer

Deterministic, local, cheap. Runs per session on ingest/close. Produces a feature vector; aggregates roll up into the **competence profile**. Detectors (§7) read these.

**Per-session features** (non-exhaustive; each versioned):

| Feature | Definition |
|---|---|
| `token_burn_total` | Σ input+output across turns |
| `tokens_per_useful_output` | burn ÷ (assistant turns that produced an edit/answer) |
| `context_saturation_peak` | max(cumulative input_tokens) ÷ model window |
| `saturation_at_first_edit` | saturation when the first `FileSnapshot` edit occurs |
| `cache_read_ratio` | cache_read ÷ total input (low = cold restarts / re-sends) |
| `search_in_main_context` | count of Grep/Glob/Read in non-sidechain turns |
| `subagent_delegation_rate` | `SubagentSpawn` count ÷ search-heavy turns |
| `tool_error_rate` | ToolResult(Error) ÷ ToolResult(all) |
| `ignored_error_count` | error followed by a non-corrective next action / identical retry |
| `reprompt_count` | consecutive user turns / corrective-phrase hits ("no, actually", "that's wrong") |
| `prompt_specificity` | heuristic 0–1: length, file-path presence, concrete nouns, acceptance criteria (Fugu-refined later) |
| `file_churn` | edits per file (from FileSnapshot deltas) |
| `thrash_index` | repeated edits to same file/lines + re-runs of same failing command |
| `time_to_first_output_ms`, `total_session_ms`, `idle_gaps` | timing (from `turn_duration`/`away_summary`) |
| `model_effort_fit` | model+reasoning_effort vs task difficulty proxy |
| `planning_ratio` | Thinking/plan tokens ÷ action tokens |
| `verification_present` | did a test/build/verify tool run before "done"/commit? |
| `permission_friction` | repeated permission prompts for the same tool |

**Cross-session / profile:**
- `repeated_explanation_clusters` — MinHash/shingle (and optional local embeddings) over user prompts across sessions; clusters of near-duplicate setup context = "belongs in CLAUDE.md."
- Per-pattern **frequency** and **trend** (improving / worsening over `profile_history`).
- Per-project rollups (which repos leak most).

The featurizer is the cheap pre-filter that keeps Fugu costs bounded: only sessions/patterns crossing detector thresholds reach the Brain.

---

## 7. The Anti-Pattern Taxonomy — "the holes"

The heart of "learns what your holes are." Each pattern = a **deterministic detector** (cheap nomination) + **Fugu confirmation** (judgment) + **cost model** + **Forge fix template**. Detectors only *nominate*; Fugu decides.

| id | Name | Detector signal | Cost axis | Forge fix |
|---|---|---|---|---|
| `CONTEXT_BLOAT` | Searching/reading in main context | `search_in_main_context` high & `saturation_at_first_edit` > 0.6 | tokens | `/search-first` subagent hook + CLAUDE.md note |
| `NO_DELEGATION` | Never spawns subagents | `subagent_delegation_rate ≈ 0` & tool_call_count high | tokens, time | hook nudging Task delegation |
| `VAGUE_PROMPT` | Under-specified asks | low `prompt_specificity` + reprompt ≤2 turns | time, tokens | prompt template per task type |
| `WHACK_A_MOLE` | Symptom-patching loops | `thrash_index` high | time, tokens, risk | "stop & rethink root" hook + plan-first nudge |
| `IGNORED_TOOL_ERROR` | Errors not handled | `ignored_error_count` > 0 | risk | guardrail hook |
| `REPEATED_EXPLANATION` | Re-explaining same context | cross-session prompt cluster | time | generated `CLAUDE.md`/`AGENTS.md` block |
| `WRONG_MODEL_FOR_TASK` | Effort/model mismatch | `model_effort_fit` low | tokens, quality | model-routing guidance |
| `NO_PERSISTENT_CONTEXT` | Missing project memory | recurring setup & no CLAUDE.md/AGENTS.md | time | scaffold context file |
| `CONTEXT_NEVER_COMPACTED` | Window blown, no offload | high saturation, no compact/subagent | tokens | compaction-reminder hook |
| `CACHE_COLD_RESTARTS` | Restarting too often | low `cache_read_ratio` across many short sessions | tokens | session-continuity guidance |
| `TOOL_MISFIRE` | Wrong tool for the job (e.g. `cat`/`grep` in Bash vs Read/Grep) | tool-usage heuristics | tokens | tool-routing hook |
| `NO_PLAN_FOR_COMPLEX` | Big task, no plan step | many files touched, `planning_ratio` low | time, risk | plan-mode nudge |
| `UNVERIFIED_COMPLETION` | "Done" without tests | `verification_present=false` before done/commit | risk | verification-before-done hook |
| `ABANDONED_TASKS` | Sessions end unresolved; reopened later | dangling sessions + later same-topic session | time | follow-up surfacing |
| `PROMPT_OVERLOAD` | Mega-prompt dumps | single prompt token spike | quality | structuring template |
| `SKILL_UNDERUSE` | Doing manually what a skill/hook automates | recurring manual sequence matching a known skill | time | suggest/auto-wire skill |
| `PERMISSION_FRICTION` | Repeated same permission prompts | `permission_friction` high | time | settings allowlist diff |

The taxonomy is data-driven (TOML/registry), so patterns are added without code changes to the pipeline. Each finding carries **evidence** (session/turn/event refs) so the UI can show *exactly* where it happened.

---

## 8. BRAIN — the Fugu analysis pipeline

Three Fugu stages over candidates. All calls go to `https://api.sakana.ai/v1` (OpenAI-compatible). Stages use **structured output** (`text.format: json_schema` on Responses) so results are validated objects, not parsed prose. Key stored in Keychain.

**Models / effort** (confirmed surface):
- Diagnostician & Coach → `fugu-ultra`, `reasoning.effort: "xhigh"` (max quality; routes 1–3 expert agents).
- Verifier & live checks → `fugu`, `effort: "high"` (cheaper, fast).
- `previous_response_id` is **not** accepted — full context is sent in `input` each call.

### 8.1 Diagnostician
- **Input:** featurized session summaries + trimmed/redacted transcript evidence for the *nominated* candidates only (cost control).
- **Output schema:** `Finding[] { pattern_id, evidence:[{session,turn,event}], severity:1-5, frequency, est_cost_tokens, est_cost_minutes, confidence:0-1, rationale }`.
- Batched per (harness, project). One war-room run per batch.

### 8.2 Coach
- **Input:** confirmed findings + profile + trends.
- **Output schema:** `Diagnosis { ranked_holes:[finding_id…], do:[string], stop:[string], narrative }` — the human-facing coaching.

### 8.3 Verifier (adversarial)
- For each high-severity finding, a `fugu` call **prompted to refute it** (perspective-diverse: correctness / evidence-sufficiency / alternative-explanation). Finding survives only if it isn't refuted. Kills plausible-but-wrong coaching before it reaches you.

### 8.4 War-room visualization (honest)
Fugu's API exposes **orchestration token aggregates**, not per-agent identities:
`input_tokens_details.orchestration_input_tokens`, `output_tokens_details.orchestration_output_tokens`, etc. We **stream** (`stream:true`, SSE `response.output_text.delta`, `stream_options.include_usage`) and drive the visualizer from real signals: deltas → activity pulses; final orchestration token weight + `fugu` vs `fugu-ultra` (1–3 agents) → node count/intensity. **We visualize orchestration *intensity*, truthfully — we do not fabricate fake per-agent dialogue.** (Constraint noted in §21.)

### 8.5 Cost control
- Deterministic detectors gate what reaches Fugu.
- `fugu_runs` ledger records tokens/latency/cost per call (incl. orchestration tokens — they're billed as normal tokens).
- Per-run + **daily budget caps** (config); on cap, Brain degrades to detector-only findings and tells the user.
- **Caching:** request keyed on `raw_hash` of inputs; unchanged sessions are never re-diagnosed.

---

## 9. FORGE — artifact generation

Turns each confirmed finding into a real, approvable fix. **Never auto-applies.**

**Artifact kinds → target:**
- `claude_md_block` → project `CLAUDE.md` / `~/.claude/CLAUDE.md`
- `agents_md_block` → `AGENTS.md` (Codex)
- `skill` → `.claude/skills/<name>/SKILL.md`
- `hook` → `settings.json` `hooks` entry (e.g. UserPromptSubmit / PreToolUse / Stop)
- `prompt_template` → snippet file / clipboard
- `settings_allowlist` → `settings.json` permissions

**Flow:** finding → fix generator (Fugu `fugu`, structured) → render as a **unified diff** against the real target → user previews in the terminal overlay → approve (`Y`) → apply with **timestamped backup** + a provenance marker (`# generated by WARDEN finding <id>`) + `artifacts` row. **Rollback** restores the backup. All applies are local-only; WARDEN never commits, pushes, or opens PRs.

---

## 9A. RADAR — the sixth organ: fleet locator & navigator

> *See every agent on your machine. Warp to any one of them — visibly, never blindly.*

### 9A.0 The problem & the governing principle

You run a fleet — on the dev machine right now: multiple `claude` processes on `ttys084/086/090`, Codex.app, Cursor, VS Code, all live simultaneously. You lose track of *where* each one physically is, and which are *waiting on you*. The naive "go to agent" — focus some window — **feels broken**, because the spatial jump is illegible: a window just appears and you've lost your bearings.

**Governing principle: never teleport blind. Locate *visibly*, then navigate — and when clean navigation isn't possible, degrade into an honest "here is exactly where it is," never a mystery jump.** Every RADAR behavior below derives from this one rule.

### 9A.1 The correlation problem (and why it's solvable here)

A session is a *logical* artifact (a transcript). A window is a *physical* artifact. RADAR must bind them: **`SessionId → OS process → physical location (window/tab/pane) → screen geometry → focusable target`.** Verified signal availability on the real machine:

| Signal | Source | Gives us |
|---|---|---|
| `TERM_PROGRAM` | process env (`ps eww`) | **which emulator** (`Apple_Terminal`, `iTerm.app`, `WezTerm`, `ghostty`, `vscode`…) → which focus backend |
| `TERM_SESSION_ID` | process env | **exact Apple Terminal tab UUID** — Terminal's AppleScript dictionary addresses tabs by this. Zero-guess focus. |
| `ITERM_SESSION_ID` | process env | exact iTerm2 session — addressable via iTerm Python/AppleScript |
| `$TMUX`, `TMUX_PANE` | process env | running inside tmux → pane address via `tmux list-panes -a` (two-layer: host window + pane) |
| controlling **TTY** | `ps -o tty=` | maps to emulator tab / tmux pane when no session-id env |
| **cwd** | `lsof -d cwd` / `proc_pidinfo` | project match + disambiguation + the locate-only card |
| cmdline, start time, PPID chain | `ps` | identity card + walk child→shell→emulator |
| window bounds / owner PID / display | `CGWindowListCopyWindowInfo` (swift ✓) | **the rectangle to highlight**, which monitor, on-screen vs other-Space |

**The primary link is not `lsof` on the transcript** — verified finding: Claude Code *appends-and-closes* the `.jsonl`, so the fd isn't reliably held. The exact link is **hook self-registration** (§9A.4): the agent reports its own location from the inside. Process/cwd/TTY correlation is the fallback; Fugu is the tie-breaker (§9A.6).

### 9A.2 Architecture — three sub-services

```
        ┌──────────────────────────── RADAR ────────────────────────────┐
        │                                                                │
 hooks  │   ┌─────────────────┐   reconcile (~2s + on-event)             │
 ──POST─┼──▶│ A) FLEET TRACKER │  process scan ⋈ sessions ⋈ locations     │
        │   │  Agent Proc Table│  ⋈ status  → AgentInstance[]             │
 ps/    │   └────────┬─────────┘                                          │
 lsof ──┼────────────┤                                                    │
        │            ▼                                                    │
 Core   │   ┌─────────────────┐   window bounds, display, Space,         │
 Graphics┼─▶│ B) SPATIAL       │   visibility (visible|other-space|       │
        │   │    RESOLVER      │   minimized|offscreen|cloud)             │
        │   └────────┬─────────┘                                          │
        │            ▼                                                    │
        │   ┌─────────────────┐   locate-before-leap viz +               │
        │   │ C) NAVIGATOR     │   4-tier actuation ladder + Fugu narrate │
        │   │   ("Warp")       │──▶ AppleScript / tmux / AX / cloud-link  │
        │   └─────────────────┘                                          │
        └────────────────────────────────────────────────────────────────┘
```

**A) Fleet Tracker.** Maintains the live *Agent Process Table*, reconciled on a cheap ~2 s poll and on hook events. For each agent it resolves the binding once and keeps it warm, so **Warp is pre-computed, never derived on click**. Detects birth/death of agent processes (PIDs are ephemeral) and re-binds.

**B) Spatial Resolver.** For a bound process, derives on-screen geometry via `CGWindowListCopyWindowInfo` (bounds + `kCGWindowOwnerPID` + display) and classifies **visibility**: `visible` (current Space) · `other_space` · `minimized` · `offscreen` · `cloud` · `dead`. `CGWindowList` returns current-Space windows by default; absence there + presence in the all-windows list ⇒ `other_space`/`minimized`.

**C) Navigator.** Executes the Warp sequence (§9A.5) at the highest confidence tier available (§9A.5 ladder), narrated by Fugu.

### 9A.3 Data model

```rust
struct AgentInstance {
    id: AgentInstanceId,
    session_id: Option<SessionId>,     // ← bind to EYES/BRAIN (may be None pre-correlation)
    harness: Harness,
    pid: i32, ppid: i32,
    location: PhysicalLocation,
    status: AgentStatus,
    confidence: f32,                   // binding confidence (1.0 = hook self-registered)
    project: Option<ProjectRef>,
    last_activity: DateTime,
    bound_via: BindSource,             // HookSelfRegister | EnvSessionId | TtyMatch | CwdHeuristic | FuguDisambig
}

enum PhysicalLocation {
    TerminalTab { app: String, term_session_id: Option<String>, tty: String },
    TmuxPane    { client_tty: String, host: Box<PhysicalLocation>, target: String /* sess:win.pane */ },
    GuiWindow   { app: String, window_id: u32, title: String },   // Cursor/VS Code/Claude.app
    Cloud       { provider: String, url: Option<String> },         // Hermes
    Unknown     { tty: Option<String>, cwd: Option<PathBuf> },
}

enum AgentStatus { Working, AwaitingInput, AwaitingPermission, Idle, Errored, Exited }

struct Geometry { rect: Rect, display_id: u32, visibility: Visibility }
```

### 9A.4 Binding — the correlation ladder (exact → heuristic → judged)

1. **Hook self-registration (exact, confidence 1.0).** WARDEN's hooks (§10) already fire *inside* the agent process. At session start the hook reports a `LOCATION` payload to WARDEN's socket: `{session_id, pid, term_program, term_session_id|iterm_session_id, tmux_pane, tty, cwd}`. The agent literally tells WARDEN where it lives. Covers Claude Code & Codex once hooks are installed.
2. **Env session-id match (exact, ~1.0).** For agents started before hooks, or hookless: scan candidate agent processes, read `TERM_SESSION_ID`/`ITERM_SESSION_ID`/`TMUX_PANE` from env, match the emulator's scripting API. Exact tab without a hook.
3. **TTY → pane/tab (high).** `tmux list-panes -a` (pane_tty/pane_pid) or emulator tab-by-tty.
4. **cwd + recency heuristic (medium).** Bind by matching process cwd to the session's project + "transcript whose mtime advances while this PID lives." Used when the above are unavailable.
5. **Fugu disambiguation (judged).** ≥2 candidates and no exact signal → §9A.6.

`bound_via` + `confidence` are stored and surfaced — the UI shows *how sure* RADAR is, which itself defuses "broken."

### 9A.5 The Warp action — locate-before-leap + the honesty ladder

**The sequence (the anti-"feels broken" core):**
1. WARDEN's transparent always-on-top overlay **dims the screen**.
2. A **glowing rectangle snaps onto the target window's real bounds** (from the Spatial Resolver), with a label: `claude · auth-refactor · Terminal · Desktop 2 · idle 4m`.
3. A **beam animates** from the fleet card to that rectangle (~300 ms — slow enough to read, fast enough to feel snappy).
4. **Then** focus fires (the right backend, below).
5. Highlight fades on the now-focused window. **You always saw where you were going before you got there.**

**The actuation ladder — degrade, never fake:**

| Tier | Condition | Action |
|---|---|---|
| **1 · Exact** | hook self-reg, or env session-id, or tmux pane known | highlight → focus **exact tab/pane**. One click, you're there. |
| **2 · Window** | window known, sub-pane not addressable | highlight → raise window + hint ("pane 2, top-right") |
| **3 · App** | app + TTY known, emulator unscriptable | bring app forward + **identity card** (cwd, cmdline, started-at) to spot it in ~2 s |
| **4 · Locate-only** | no AX permission, or **cloud agent**, or dead | rich card: host · pid · tty · cwd · terminal · last activity — or "runs in the cloud → open in browser" / "exited 2 m ago → open transcript" |

**Edge states (explicit, never silent):** `other_space` → directional cue "→ Desktop 3", trigger Space switch (AX raise crosses Spaces), *then* highlight once visible; `minimized` → "minimized in Dock" + un-minimize; `offscreen`/other monitor → highlight draws on the correct display (WARDEN spawns the highlight window on that display); `dead` → "this agent exited" + offer transcript.

**Focus backends:**
- **Apple Terminal** → AppleScript: select the tab whose `tty`/session matches, `set frontmost`.
- **iTerm2** → AppleScript/Python: `select` the session by `ITERM_SESSION_ID`.
- **tmux** → focus host emulator window, then `tmux select-window` + `select-pane` (+ `switch-client` if needed).
- **VS Code / Cursor** → AX `AXRaise` on the workspace window (title match) — or `code -r`/`cursor` URL where available.
- **Universal fallback** → Accessibility `AXRaise` on the `CGWindow` owned by the PID — works for any app with no scripting dictionary.

### 9A.6 Where Fugu earns its keep ("Fugu figures out the how")

Deterministic signals lead; Fugu applies **judgment** only where determinism runs out, always with a confidence + reason:
- **Disambiguation** (`fugu`, structured): given the candidate bundle (each candidate's cwd, cmdline, recent transcript fingerprint, last-activity, window title), return `{chosen_pid, confidence, reason}`. Low confidence ⇒ RADAR drops to Tier 3/4 rather than risk a wrong jump.
- **Navigation narration**: the human line shown during Warp — *"Jumping to auth-refactor — Terminal, Desktop 2, idle 4 min, **waiting on you**."*
- **Recovery guidance**: when actuation fails, compose the precise "find-it-yourself" card from whatever identity signals exist.

### 9A.7 Status model & the proactive payoff

Status is derived from the live transcript tail / hook events: last event is an assistant `tool_use` awaiting result → **Working**; assistant text ended the turn, no new user turn → **AwaitingInput**; permission-mode prompt pending → **AwaitingPermission**; mid-turn idle > N min → **Idle/stalled**; unhandled `ToolResult(Error)` → **Errored**; process gone → **Exited**.

This powers (a) a **live fleet map** — every agent as a node with harness icon, project, status, location label, last activity, *click → Warp* — and (b) a killer proactive ping that ties into §10: **"⚠ 3 agents finished and waiting on you."** → click → Warp to the first.

### 9A.8 Permissions, IPC, risks

- **macOS TCC permissions** (one-time, user-granted, explained in onboarding): **Accessibility** (window raise / AX control), **Automation/AppleEvents** per app (Terminal, iTerm2, Cursor) for scripted tab selection, **Screen Recording** (only if precise window titles/captures are needed for highlight labels). RADAR degrades to Tier 4 (locate-only) if a permission is denied — it never breaks, it just explains.
- **IPC additions** — Commands: `list_fleet()`, `locate_agent(id)`, `warp_to_agent(id)`. Events: `fleet.update`, `agent.status`, `warp.progress`.
- **Risks** (also in §21): R8 cross-Space highlight timing; R9 unscriptable/niche emulators → Tier 3 floor; R10 TCC-permission friction → onboarding flow + graceful Tier-4 fallback; R11 PID-reuse races → re-verify binding at Warp time, not just from cache.

---

## 10. Live interjection

**Mechanism — hooks + local socket.**
1. WARDEN installs lightweight hook entries into Claude Code `settings.json` (`UserPromptSubmit`, `PreToolUse`, `Stop`) and the Codex equivalent. Each hook is a tiny forwarder that POSTs the event JSON to WARDEN's local IPC socket (`~/.warden/warden.sock`, or `127.0.0.1:<port>`). *(Install is opt-in and shown as a diff, like any Forge artifact.)*
2. Daemon maintains a **live feature state** per active session. On each hooked event it runs **deterministic detectors only** (sub-millisecond) against that state.
3. On a trigger (e.g. 3rd consecutive Grep in main context; `Stop` after edits with no test run), WARDEN raises a **non-blocking** overlay toast: *"⚠ WARDEN — context bloat forming. Delegate this search? [⌥Space]"*.
4. Optional deeper check: async `fugu` low-latency call; result shown when ready (never blocks the agent).

**Noise control:** per-pattern cooldown, debounce, session-level mute, global "do not disturb." `interjections` table tracks shown/dismissed/muted to suppress nagging and feed the profile.

**Coverage by harness:** Claude Code & Codex = true live hooks. Cursor & Hermes = **near**-real-time via poll/tail (no hook surface) → diagnosis-after, not in-the-instant.

---

## 11. FACE — the Tauri app

### 11.1 Rust core (the daemon)
Runs as a **menubar agent** (always on, no dock). Modules:
- `ingest` (adapter registry + watchers), `store` (rusqlite + FTS5 + migrations), `featurizer`, `detectors`, `brain` (Fugu client: `reqwest` + SSE), `forge`, `hooks` (socket server), `scheduler`, `voice`, `screen`, `ipc`.
- Crates: `tauri` v2, `tauri-plugin-global-shortcut`, `tauri-plugin-positioner`, `notify`, `rusqlite`(bundled+FTS5), `tokio`, `reqwest`, `serde`, `tray-icon`, screen via `screencapturekit`/`xcap`, audio via `cpal`, STT via `whisper-rs`, OCR via Apple Vision bridge (`objc2`) with `tesseract` fallback.
- **RADAR modules:** `fleet` (process scan via `sysinfo` + `libproc`/`proc_pidinfo` for cwd/env, reconciler), `spatial` (`core-graphics` `CGWindowListCopyWindowInfo` for bounds/owner/display via `objc2`), `navigator` (AppleScript via `osascript`/`osakit`, tmux shell-out, Accessibility `AXUIElement` raise via `accessibility`/`objc2`), `warp_overlay` (per-display Tauri highlight windows).

### 11.2 The overlay window
Borderless, transparent, always-on-top, summoned by global hotkey (default `⌥Space`, configurable; documented conflict note). Tauri window config: `transparent:true, decorations:false, alwaysOnTop:true, skipTaskbar:true, visibleOnAllWorkspaces:true`. Click-through when idle; focus + raise on summon; dismiss on `Esc`/blur. Positioned via `tauri-plugin-positioner`.

### 11.3 IPC contract
- **Commands (web→Rust, `#[tauri::command]`):** `query_profile`, `get_findings`, `get_diagnosis`, `run_diagnosis(scope)`, `apply_artifact(id)`, `revert_artifact(id)`, `ask(query, mode)`, `start_voice()/stop_voice()`, `capture_screen()`, `set_config(...)`, `mute_pattern(id)`, **`list_fleet()`, `locate_agent(id)`, `warp_to_agent(id)`** (RADAR, §9A).
- **Events (Rust→web):** `ingest.progress`, `fugu.delta`, `fugu.usage` (orchestration tokens), `diagnosis.ready`, `interjection`, `voice.partial`, `artifact.applied`, **`fleet.update`, `agent.status`, `warp.progress`** (RADAR).

### 11.4 Frontend (web overlay)
- Stack: Vite + TypeScript. **Terminal renderer:** custom canvas/DOM green-phosphor terminal (optional scanline/CRT bloom), typewriter output. **War-room visualizer:** WebGL (Three.js; optional react-three-fiber) — agent nodes pulse on `fugu.delta`, sized by orchestration tokens. **Sequencing:** anime.js for the boot sequence and diagnosis reveal.
- Screens: Boot → Idle (ambient status) → Ask (voice/text) → War room (live Fugu) → Diagnosis (ranked holes + evidence) → Fix preview (diff + `[Y/n]`) → **Fleet map (RADAR: live agent nodes + status + `WARP →`)** → **Warp overlay (full-screen dim + target-window highlight + beam, §9A.5)**.
- Aesthetic: homebrew terminal, green-on-black, Matrix-coded boot. Motion is purposeful, not decorative — it reflects real backend state.

---

## 12. VOICE + screen Q&A

- **STT:** local `whisper-rs` (`base.en` default, configurable). Push-to-talk on the hotkey (wake-word optional, later). Streams partials to the overlay (`voice.partial`).
- **Routing:** transcript → intent router → `ask(query, mode)`:
  - *"what's wrong with my workflow?"* → diagnosis (cached or fresh).
  - *"why did my last <harness> session suck?"* → targeted single-session Fugu analysis.
  - *"what's on my screen?"* → screen path.
- **Screen Q&A:** on explicit invocation only — capture active display (ScreenCaptureKit), OCR (Apple Vision), optionally attach the image to Fugu (Responses API multimodal), fused with WARDEN's knowledge of the current session → answer.
- **TTS:** macOS `AVSpeechSynthesizer` (via bridge) for spoken answers; toggleable. (Local neural TTS like Piper is a later upgrade.)
- **Privacy:** mic and screen capture are **invocation-gated**, never continuous; a visible indicator shows when either is active.

---

## 13. Orchestration scheduler & runtime

Triggers for analysis: **session close** (debounced), **idle** (no input N min — cheap window to spend tokens), **on-demand** (ask), **daily rollup**. Concurrency-limited, budget-capped, cache-aware. Live interjection runs on the hot path (deterministic only); deep Fugu work always runs off the hot path so it never slows an agent.

---

## 14. Security, privacy, secrets

- **On-device by default.** All transcripts, features, findings, artifacts live in `~/.warden`. No cloud account, no telemetry.
- **Egress = Fugu only.** The sole outbound data is what the Brain sends to `api.sakana.ai`: trimmed, **redacted** (configurable secret/PII scrubbing) evidence excerpts for *nominated* findings — never whole transcripts by default. A **"what gets sent" preview** + per-project opt-out are first-class.
- **Secrets:** Fugu (and optional Hermes) keys in macOS **Keychain**, never in plaintext config.
- **Hooks & file writes** are always shown as diffs and applied only on approval, with backups. WARDEN never runs `git push`, never opens MRs/PRs.
- **RADAR permissions (TCC):** Accessibility (window raise/control), per-app Automation/AppleEvents (Terminal, iTerm2, Cursor), optional Screen Recording (precise window titles). All one-time, user-granted, explained in onboarding; denial degrades RADAR to Tier-4 locate-only, never breaks it. RADAR only *reads* process metadata and *focuses* windows — it never reads other apps' window contents beyond what's needed for the highlight label, and never injects input into them.
- Optional **SQLCipher** encryption at rest.

---

## 15. Configuration

`~/.warden/config.toml`:
```toml
[general]      hotkey = "Alt+Space"; theme = "phosphor-green"; tts = true
[adapters]     claude_code = true; codex = true; cursor = true
               cursor_poll_secs = 15
[adapters.hermes]   enabled = false; api_base = ""; # key in Keychain
[[generic_adapter]] name = "opencode"; root = "~/.opencode"; mapping = "..."
[brain]        diagnose_model = "fugu-ultra"; effort = "xhigh"
               verify_model  = "fugu"; daily_budget_usd = 5.0
[privacy]      redact = ["api_key","token","password","email"]; send_whole_transcript = false
[interjection] enabled = true; cooldown_secs = 300; muted_patterns = []
[radar]        enabled = true; poll_secs = 2; warp_animation_ms = 300
               highlight = true; min_warp_confidence = 0.6   # below → Tier-3/4, no jump
               notify_waiting_agents = true                   # "N agents waiting on you"
```
A settings pane in the overlay mirrors this.

---

## 16. Error handling & resilience

- **Adapter failure** is isolated (one bad source never stalls others); unknown record shapes → `SystemNotice` fallback, never a dropped session. Schema drift is logged, not fatal.
- **Fugu failure:** ret/backoff with jitter on 429/5xx; on budget cap or hard failure, degrade to detector-only findings and say so in the UI.
- **Partial data:** featurizer tolerates missing fields (e.g. no `turn_duration`) by marking features unknown, not zero.
- **Crash safety:** watermarks + idempotent ingest mean restart resumes cleanly.

---

## 17. Testing strategy

- **Fixtures:** anonymized real transcripts per harness checked into `tests/fixtures/`.
- **Adapter tests:** raw fixture → expected IR (golden).
- **Featurizer tests:** IR → expected feature vector (golden, version-pinned).
- **Detector tests:** crafted sessions that must/must-not trip each pattern.
- **Brain tests:** Fugu client mocked with recorded SSE responses; schema-validation tests on structured outputs.
- **Forge tests:** finding → diff → apply → revert round-trip on temp files.
- **E2E:** seeded sample DB → full pipeline → asserts a diagnosis with evidence; overlay smoke test.

---

## 18. Tech stack (pinned choices)

| Layer | Choice |
|---|---|
| Shell | Tauri v2 (Rust core + web overlay), macOS Apple-Silicon |
| Store | SQLite (rusqlite, bundled, FTS5); optional SQLCipher |
| Async | Tokio; `reqwest` (+ SSE) for Fugu |
| Watch | `notify` (fs), poll loop for SQLite/API sources |
| Hotkey/Window | `tauri-plugin-global-shortcut`, `tauri-plugin-positioner` |
| Voice | `whisper-rs` (STT), AVSpeechSynthesizer (TTS) |
| Screen/OCR | ScreenCaptureKit/`xcap`, Apple Vision (`objc2`) / `tesseract` |
| Frontend | Vite + TS, custom canvas terminal, Three.js, anime.js |
| Engine | Sakana Fugu — `fugu-ultra` (diagnose/coach), `fugu` (verify/live) |

---

## 19. Build decomposition & milestones

Umbrella spec → 8 milestone sub-specs, each its own spec→plan→build cycle. Build order puts the **demo-critical path first**; v1 scope (per approved design) includes all of M0–M7, with voice and RADAR.

| Milestone | Sub-spec | Delivers | Demo state |
|---|---|---|---|
| **M0** | Spine | IR + Claude Code adapter + SQLite store + featurizer | data → features |
| **M1** | Brain | Fugu Diagnostician→Coach→Verifier over CC data | verified diagnosis JSON |
| **M2** | Face | Tauri overlay, hotkey, terminal, war-room viz; answers *"what's wrong with my workflow?"* | **★ first jaw-drop, end-to-end** |
| **M3** | **RADAR** | Fleet Tracker + Spatial Resolver + Navigator (Warp) + fleet map + "N waiting on you" | **★ second jaw-drop — see & Warp to every agent** |
| **M4** | Forge | artifact gen + diff + apply/revert | one-keystroke fixes |
| **M5** | Live | hook IPC + deterministic interjection (+ feeds RADAR exact location) | catches you in the act |
| **M6** | Voice | local STT/TTS + screen Q&A | ask aloud |
| **M7** | Adapters+ | Codex, Cursor, Hermes, Generic | full fleet coverage |

**Hook channel note:** RADAR (M3) and Live interjection (M5) share the hook IPC socket (§10). M3 introduces the minimal **hook self-registration** payload (location identity) for exact-tier Warp; M5 extends the same channel with live detector checks. RADAR ships a deterministic process/cwd/TTY binding first, so it demos *before* hooks land, with exact-tab precision arriving as self-registration comes online.

**Recommended first detailed plan:** the **M0+M1+M2 vertical slice** (Claude Code only) — it proves the hardest assumption (raw transcripts → a real, verified, evidence-cited diagnosis) and produces the jaw-drop on day one. **RADAR (M3) is the natural second slice** — it leans on the process/window probing already validated (§9A.1) and is the most viscerally impressive feature; if you'd rather lead with the fleet-map "Warp," M3 can be pulled ahead of M2's diagnosis depth. Then widen.

---

## 20. The jaw-drop demo (90 seconds, M2)

```
▌ ⌥Space ▌                      black screen, single green cursor blinks
  WARDEN v0.1 — mounting 168 sessions across 4 harnesses… online.
> (aloud) "what's wrong with how I use my agents?"
  ┌─ WAR ROOM ───────────────────────────────┐
  │ ◆ Diagnostician  analyzing 47 sessions…  │   nodes pulse on real
  │ ◆ Verifier       cross-checking…         │   fugu.delta + orch tokens
  └──────────────────────────────────────────┘
  ▌ HOLE #1 — CONTEXT BLOAT          severity ▰▰▰▰▱   seen in 31% of sessions
    You search & read in the main context instead of delegating.
    Est. cost: ~1.2M tokens / week.
    Evidence: 14 sessions ▸  FIX: CLAUDE.md block + /search-first hook
    Apply? [Y/n]
> Y     ✓ wrote ~/.claude/CLAUDE.md (+ backup)   ✓ hook installed
        next session, the live hook catches you starting to do it again.
```

### 20.1 The RADAR beat (M3) — "Warp to agent"

```
> "show me my fleet"
  ┌─ FLEET ─ 6 agents ──────────────────────────────────────────────┐
  │ ◇ claude  auth-refactor   ● working        Terminal · Desktop 2  │
  │ ◆ claude  warden-spec     ⏸ WAITING ON YOU  iTerm · Desktop 1    │
  │ ◇ codex   migrations      ● working        Codex.app            │
  │ ◆ claude  flaky-tests     ⚠ errored        tmux main:2.1         │
  │ ◇ cursor  dashboard       ● working        Cursor · Display 2    │
  │ ◇ hermes  research        ☁ cloud          → open in browser     │
  └─────────────────────────────────────────────────────────────────┘
  ⚠ 2 agents are finished and waiting on you.
> [↵ on "warden-spec"]   WARP →
  screen dims · a green rectangle snaps around the iTerm window on Desktop 1
  "Jumping to warden-spec — iTerm, Desktop 1, idle 4m, waiting on you"
  beam flies to it · iTerm raises · exact tab selected · highlight fades.
  You saw exactly where you went. Nothing felt broken.
```

---

## 21. Open questions & risks

- **R1 — Hermes schema (assumed).** The only adapter whose remote schema is unverified; M7 confirms it. Until then, schema-discovery mapper + lossless `meta`.
- **R2 — War-room honesty.** Fugu exposes orchestration token *aggregates*, not per-agent streams. We visualize intensity truthfully; we do not fabricate per-agent theater. (Accepted constraint.)
- **R3 — Hotkey conflict.** `⌥Space` can insert a non-breaking space in text fields; default is configurable and documented. Alternative default under consideration: `⌃⌥W`.
- **R4 — Cursor poll vs lock.** Read-only+immutable open avoids locks; poll cadence trades freshness for cost. No live-hook surface in Cursor.
- **R5 — Live hook install.** Editing users' `settings.json` to add forwarding hooks is opt-in and diffed; must coexist with existing hooks (append, never clobber).
- **R6 — Fugu cost.** `fugu-ultra` orchestration tokens are billed as normal tokens; detector pre-filtering + caching + daily budget caps keep spend bounded.
- **R7 — Whisper/Vision footprint.** Local models add binary size; ship lazily-downloaded models, not bundled.
- **R8 — RADAR cross-Space highlight timing.** A target on another Space can't be highlighted in place; sequence is *cue → switch Space → highlight once visible*, which adds latency. Accepted; the cue keeps it legible.
- **R9 — Unscriptable/niche emulators.** Some terminals expose no scripting/remote-control surface; RADAR floors at **Tier 3** (app-forward + identity card) for those rather than faking precision. Per-emulator backends added over time.
- **R10 — TCC permission friction.** Accessibility/Automation/Screen-Recording prompts can deter users; mitigated by an onboarding flow that explains each, requests lazily (only when first needed), and **degrades to Tier-4 locate-only** on denial — RADAR never hard-fails.
- **R11 — PID reuse / stale binding.** PIDs are recycled; a cached binding could point at the wrong process. Mitigation: bindings carry a `(pid, start_time)` identity and are **re-verified at Warp time**, not trusted from cache alone.

---

*End of master spec. Next: spec self-review (§ inline), then user review, then a detailed implementation plan for the M0→M2 vertical slice (RADAR/M3 a strong alternative first slice).*
