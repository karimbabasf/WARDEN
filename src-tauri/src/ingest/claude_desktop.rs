//! Claude Desktop "local agent mode" adapter.
//!
//! The Claude **desktop app** runs its in-app workflows through an embedded
//! `claude-code` SDK and persists each run under
//! `~/Library/Application Support/Claude/local-agent-mode-sessions/`
//! `<workspace>/<context>/local_<uuid>/audit.jsonl` (+ a sibling
//! `local_<uuid>.json` and a nested `.claude/projects/*.jsonl`). The
//! ingestible signal is **`audit.jsonl`**: one JSON object per line in the
//! Agent-SDK `stream-json` shape — `{type, message, session_id,
//! parent_tool_use_id, uuid, _audit_timestamp, …}` — where `message` is a
//! standard Anthropic message object. (The nested `.claude/projects/*.jsonl`
//! uses a different `{content, operation}` shape and is intentionally ignored.)
//!
//! Two facts shape this adapter, distinguishing it from the Claude **Code** CLI
//! adapter even though it reports the same `Harness::ClaudeCode` (the desktop app
//! IS Claude — emerald — distinguished only by an `origin` sub-label):
//!
//! * **Subagents are interleaved, not separate files.** One `audit.jsonl` holds
//!   MULTIPLE `session_id`s — the orchestrator plus every subagent it spawned —
//!   interleaved chronologically. So one file maps to *several* IR sessions
//!   (split by `session_id`), and the parent→child link is resolved structurally
//!   in [`link_desktop_subagents_in_store`]: the session whose id matches the
//!   `local_<uuid>` directory name is the root; the other co-resident session
//!   ids are its subagents. (`parent_tool_use_id` is present in the schema but
//!   null in practice, so it is NOT relied upon — see the design note.)
//! * **No pid registry, no archive move.** Liveness is therefore freshness-based
//!   (audit-file mtime), exactly like the Codex adapter — wired in `radar`.

use super::{Adapter, SessionBatch};
use crate::ir::*;
use crate::store::Store;
use crate::util::{default_claude_desktop_sessions, hash64, parse_ts, stable_id, truncate_chars};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ClaudeDesktopAdapter {
    pub root: PathBuf,
    pub store: Store,
    pub max_files: Option<usize>,
}

impl ClaudeDesktopAdapter {
    pub fn new(store: Store) -> Self {
        Self {
            root: default_claude_desktop_sessions(),
            store,
            max_files: None,
        }
    }
    pub fn with_root(root: PathBuf, store: Store) -> Self {
        Self {
            root,
            store,
            max_files: None,
        }
    }
}

impl Adapter for ClaudeDesktopAdapter {
    fn harness(&self) -> Harness {
        // The desktop app IS Claude: same emerald identity, distinguished only by
        // the `origin: "Claude Desktop"` sub-label carried on each session's meta.
        Harness::ClaudeCode
    }

    fn detect(&self) -> Result<Vec<PathBuf>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut paths: Vec<PathBuf> = WalkDir::new(&self.root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_audit_file(e.path()))
            .map(|e| e.into_path())
            .collect();
        paths.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        paths.reverse();
        if let Some(n) = self.max_files {
            paths.truncate(n);
        }
        Ok(paths)
    }

    fn backfill(&self) -> Result<Vec<SessionBatch>> {
        let mut out = Vec::new();
        for p in self.detect()? {
            let bytes = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
            let raw_hash = hash64(&bytes);
            // Every session row from this file stores the same whole-file hash, so a
            // single source_raw_hash(path) check deduplicates the whole file.
            if self
                .store
                .source_raw_hash(&p)?
                .is_some_and(|h| h == raw_hash)
            {
                continue;
            }
            match parse_audit(&p, &bytes, 0, raw_hash) {
                Ok(mut batches) => out.append(&mut batches),
                Err(e) => {
                    tracing::warn!(path=%p.display(), error=?e, "skipping malformed desktop audit log")
                }
            }
        }
        Ok(out)
    }

    fn parse_range(
        &self,
        path: &Path,
        bytes: &[u8],
        start_offset: u64,
        raw_hash: u64,
    ) -> Result<Vec<SessionBatch>> {
        // The desktop tree also contains nested `.claude/projects/*.jsonl` files (a
        // different schema) and `local_<uuid>.json` sidecars. The live watcher fires
        // on any `*.jsonl` under our root, so ignore everything that is not an
        // `audit.jsonl` — those carry no agent stream we model.
        if !is_audit_file(path) {
            return Ok(vec![]);
        }
        parse_audit(path, bytes, start_offset, raw_hash)
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.root.clone()]
    }
}

/// True for the one ingestible file per desktop session: `…/local_<uuid>/audit.jsonl`.
pub fn is_audit_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("audit.jsonl")
}

/// The orchestrator session id for a desktop audit log, taken from its parent
/// directory `local_<uuid>`. This id matches the `session_id` of the root agent in
/// the file; every other `session_id` in the same file is one of its subagents.
pub fn desktop_root_session_id(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("local_"))
        .map(|s| s.to_string())
}

/// True when a stored session is a Claude Desktop "local agent mode" session — the
/// marker the radar uses to route it to freshness-based liveness (it has no pid).
pub fn is_desktop_session(s: &Session) -> bool {
    s.meta
        .get("desktop")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Resolve the desktop subagent hierarchy over every persisted desktop session and
/// record the links via [`Store::link_child_session`].
///
/// Heuristic (documented as best-effort — the SDK records no explicit parent
/// pointer here): group desktop sessions by their `audit.jsonl` `source_path`; the
/// session whose `external_id` equals the `local_<uuid>` directory name is the
/// orchestrator/root; every other co-resident session is a subagent of that root.
/// Idempotent (re-recording the same parent is a plain UPDATE), so startup/live
/// ingest can call it before RADAR refreshes its cached forest. Returns the number
/// of links recorded. A group with no session matching its directory uuid is left
/// flat (no fabricated parent).
pub fn link_desktop_subagents_in_store(store: &Store) -> Result<usize> {
    let sessions = store.sessions()?;
    let desktop: Vec<&Session> = sessions.iter().filter(|s| is_desktop_session(s)).collect();
    let pairs = crate::radar::hierarchy::link_desktop_subagents(&desktop);
    let mut recorded = 0;
    for (child_sid, parent_sid) in pairs {
        store.link_child_session(&child_sid, &parent_sid)?;
        recorded += 1;
    }
    Ok(recorded)
}

/// Classify a desktop tool-use name into the IR tool kind.
fn classify_tool(name: &str) -> ToolKind {
    if name == "Task" || name == "Agent" {
        ToolKind::SubagentTask
    } else if name.contains("__") || name.starts_with("mcp_") {
        ToolKind::Mcp
    } else {
        ToolKind::Builtin
    }
}

/// One raw `audit.jsonl` line, retaining its absolute byte offset + line number so
/// every emitted `RawRef` points back at the on-disk record.
struct Rec {
    offset: u64,
    line: u32,
    ts: DateTime<Utc>,
    rec_type: String,
    value: Value,
}

/// Parse a desktop `audit.jsonl` (or an appended tail slice) into one
/// [`SessionBatch`] per distinct `session_id` present in the bytes.
///
/// * `base_offset` is added to every line's local byte position so `RawRef.offset`
///   and the returned watermark are ABSOLUTE positions in the file (0 for a full
///   parse; the saved watermark for a tail). Each batch's `offset` is the file EOF.
/// * Sessions are split by each line's own `session_id` (no filename fallback is
///   needed — unlike Codex, every desktop line is self-identifying), so a tail
///   slice lands on the SAME `stable_id` as the original backfill and merges.
fn parse_audit(
    path: &Path,
    bytes: &[u8],
    base_offset: u64,
    raw_hash: u64,
) -> Result<Vec<SessionBatch>> {
    let root_session_id = desktop_root_session_id(path);

    // 1) Split into self-identifying records, preserving file order.
    let mut offset = base_offset;
    let mut line_no = 0u32;
    // Preserve first-seen session order for a stable, source-ordered forest.
    let mut order: Vec<String> = Vec::new();
    let mut by_session: std::collections::HashMap<String, Vec<Rec>> = std::collections::HashMap::new();
    for raw_line in bytes.split_inclusive(|b| *b == b'\n') {
        line_no += 1;
        let line_len = raw_line.len() as u64;
        let line_off = offset;
        offset += line_len;
        let line = String::from_utf8_lossy(raw_line);
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // a half-written tail line is skipped (retried on the next write)
        };
        // Attribute the line to its own session_id; fall back to the directory's
        // root id, then to a path-stable id, so a line is never dropped.
        let sid_ext = v
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| root_session_id.clone())
            .unwrap_or_else(|| stable_id(&[&path.to_string_lossy()]));
        let ts = parse_ts(v.get("_audit_timestamp"));
        let rec_type = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if !by_session.contains_key(&sid_ext) {
            order.push(sid_ext.clone());
        }
        by_session.entry(sid_ext).or_default().push(Rec {
            offset: line_off,
            line: line_no,
            ts,
            rec_type,
            value: v,
        });
    }

    if by_session.is_empty() {
        anyhow::bail!("no audit records");
    }

    // 2) Build one SessionBatch per session_id.
    let mut batches = Vec::with_capacity(order.len());
    for external_id in order {
        let recs = by_session.remove(&external_id).expect("session present");
        batches.push(build_session_batch(
            path,
            raw_hash,
            offset,
            &external_id,
            root_session_id.as_deref(),
            recs,
        ));
    }
    Ok(batches)
}

fn build_session_batch(
    path: &Path,
    raw_hash: u64,
    eof_offset: u64,
    external_id: &str,
    root_session_id: Option<&str>,
    recs: Vec<Rec>,
) -> SessionBatch {
    let sid = stable_id(&["claude_desktop", external_id, &path.to_string_lossy()]);
    let is_root = root_session_id == Some(external_id);

    let mut turns: Vec<Turn> = Vec::new();
    let mut events: Vec<EventRecord> = Vec::new();
    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut project: Option<ProjectRef> = None;
    let mut started: Option<DateTime<Utc>> = None;
    let mut ended: Option<DateTime<Utc>> = None;
    let mut idx = 0u32;
    let mut cur_turn: Option<String> = None;

    // Open a new turn of `role` and make it current.
    let open_turn = |role: Role,
                         ts: DateTime<Utc>,
                         idx: &mut u32,
                         turns: &mut Vec<Turn>,
                         cur_turn: &mut Option<String>|
     -> String {
        *idx += 1;
        let tid = stable_id(&[&sid, "turn", &idx.to_string()]);
        turns.push(Turn {
            id: tid.clone(),
            session_id: sid.clone(),
            parent_id: None,
            role,
            index: *idx,
            started_at: ts,
            duration_ms: None,
            is_sidechain: !is_root, // a subagent's turns are sidechain work
        });
        *cur_turn = Some(tid.clone());
        tid
    };

    for rec in &recs {
        let ts = rec.ts;
        if started.map(|s| ts < s).unwrap_or(true) {
            started = Some(ts);
        }
        if ended.map(|e| ts > e).unwrap_or(true) {
            ended = Some(ts);
        }
        // Best-effort project cwd: the SDK `system` init line carries it.
        if project.is_none() {
            if let Some(cwd) = rec
                .value
                .get("cwd")
                .and_then(Value::as_str)
                .or_else(|| rec.value.pointer("/message/cwd").and_then(Value::as_str))
            {
                let cwdp = PathBuf::from(cwd);
                project = Some(ProjectRef {
                    repo_root: crate::util::repo_root(&cwdp),
                    cwd: cwdp,
                    git_branch: None,
                });
            }
        }

        let raw = RawRef {
            source_path: path.to_path_buf(),
            offset: rec.offset,
            line: rec.line,
        };

        match rec.rec_type.as_str() {
            "assistant" => {
                let tid = open_turn(Role::Assistant, ts, &mut idx, &mut turns, &mut cur_turn);
                let message = rec.value.get("message").cloned().unwrap_or(Value::Null);
                if let Some(model) = message.get("model").and_then(Value::as_str) {
                    if !model.trim().is_empty() {
                        models.insert(model.to_string());
                    }
                }
                map_assistant_content(&message, &sid, &tid, ts, &raw, &mut events);
                map_usage(&message, &sid, &tid, ts, &raw, &mut events, &models);
            }
            "user" => {
                let tid = open_turn(Role::User, ts, &mut idx, &mut turns, &mut cur_turn);
                let message = rec.value.get("message").cloned().unwrap_or(Value::Null);
                map_user_content(&message, &sid, &tid, ts, &raw, &mut events);
            }
            // system / result / rate_limit_event / anything else → a SystemNotice so
            // schema drift never drops a session. These carry no agent ACTION, so
            // they never affect the working/idle liveness verdict.
            other => {
                let tid = current_or_open(
                    &mut cur_turn,
                    &mut turns,
                    &mut idx,
                    &sid,
                    is_root,
                    ts,
                );
                events.push(EventRecord {
                    id: stable_id(&[&sid, "notice", &rec.line.to_string()]),
                    turn_id: tid,
                    session_id: sid.clone(),
                    ts,
                    event: Event::SystemNotice {
                        subtype: other.to_string(),
                        data: rec.value.get("message").cloned().unwrap_or(Value::Null),
                    },
                    raw_ref: raw,
                });
            }
        }
    }

    let meta = json!({
        // Emerald stays (harness == claude_code); this is the sub-label the FACE
        // renders to tell a desktop workflow apart from a CLI session.
        "originator": "Claude Desktop",
        // Structural marker the radar keys on for freshness-based (no-pid) liveness.
        "desktop": true,
        "is_root": is_root,
        "workflow_dir": root_session_id.unwrap_or_default(),
    });

    SessionBatch {
        session: Session {
            id: sid,
            harness: Harness::ClaudeCode,
            external_id: external_id.to_string(),
            project,
            model_ids: models.into_iter().collect(),
            started_at: started.unwrap_or_else(Utc::now),
            ended_at: ended,
            source_path: path.to_path_buf(),
            raw_hash,
            ingested_at: Utc::now(),
            meta,
        },
        turns,
        events,
        offset: eof_offset,
    }
}

/// Map an assistant `message.content` block array into IR events (text / thinking /
/// tool_use). The `message` is a standard Anthropic object.
fn map_assistant_content(
    message: &Value,
    sid: &str,
    tid: &str,
    ts: DateTime<Utc>,
    raw: &RawRef,
    events: &mut Vec<EventRecord>,
) {
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        // `content` can also be a bare string in some SDK frames.
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                events.push(EventRecord {
                    id: stable_id(&[sid, "text", &raw.line.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::AssistantText { text: text.into() },
                    raw_ref: raw.clone(),
                });
            }
        }
        return;
    };
    for (i, block) in blocks.iter().enumerate() {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
        match btype {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                events.push(EventRecord {
                    id: stable_id(&[sid, "text", &raw.line.to_string(), &i.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::AssistantText { text: text.into() },
                    raw_ref: raw.clone(),
                });
            }
            "thinking" => {
                let text = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                events.push(EventRecord {
                    id: stable_id(&[sid, "thinking", &raw.line.to_string(), &i.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::Thinking {
                        tokens: (text.len() / 4) as u32,
                    },
                    raw_ref: raw.clone(),
                });
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("unknown");
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                events.push(EventRecord {
                    id: stable_id(&[sid, "tool", &call_id, &raw.line.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::ToolCall {
                        tool: name.to_string(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                        call_id,
                        kind: classify_tool(name),
                    },
                    raw_ref: raw.clone(),
                });
            }
            _ => {} // redacted_thinking / image / unknown blocks carry no IR action
        }
    }
}

/// Emit a single `TokenUsage` event from an assistant `message.usage`, if present.
fn map_usage(
    message: &Value,
    sid: &str,
    tid: &str,
    ts: DateTime<Utc>,
    raw: &RawRef,
    events: &mut Vec<EventRecord>,
    models: &BTreeSet<String>,
) {
    let Some(usage) = message.get("usage") else {
        return;
    };
    let g = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| models.iter().next().cloned())
        .unwrap_or_default();
    events.push(EventRecord {
        id: stable_id(&[sid, "usage", &raw.line.to_string()]),
        turn_id: tid.to_string(),
        session_id: sid.to_string(),
        ts,
        event: Event::TokenUsage {
            input: g("input_tokens"),
            output: g("output_tokens"),
            cache_creation: g("cache_creation_input_tokens"),
            cache_read: g("cache_read_input_tokens"),
            model,
            orchestration: None,
        },
        raw_ref: raw.clone(),
    });
}

/// Map a user `message.content` into IR events. A user frame is EITHER an operator
/// prompt (string / text blocks) OR the SDK feeding tool results back as
/// `tool_result` blocks — the latter become `ToolResult` events keyed by
/// `tool_use_id` so they pair with the assistant's `ToolCall`.
fn map_user_content(
    message: &Value,
    sid: &str,
    tid: &str,
    ts: DateTime<Utc>,
    raw: &RawRef,
    events: &mut Vec<EventRecord>,
) {
    // content as a bare string → a plain operator prompt.
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            push_user_prompt(text, sid, tid, ts, raw, events);
        }
        return;
    }
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    let mut prompt_text = String::new();
    for (i, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    if !prompt_text.is_empty() {
                        prompt_text.push('\n');
                    }
                    prompt_text.push_str(t);
                }
            }
            "tool_result" => {
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let is_error = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let summary = tool_result_text(block.get("content"));
                events.push(EventRecord {
                    id: stable_id(&[sid, "tool_result", &call_id, &raw.line.to_string(), &i.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::ToolResult {
                        call_id,
                        status: if is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Ok
                        },
                        bytes: summary.len() as u64,
                        summary: Some(truncate_chars(&summary, 500)),
                    },
                    raw_ref: raw.clone(),
                });
            }
            _ => {} // image / unknown
        }
    }
    if !prompt_text.is_empty() {
        push_user_prompt(&prompt_text, sid, tid, ts, raw, events);
    }
}

fn push_user_prompt(
    text: &str,
    sid: &str,
    tid: &str,
    ts: DateTime<Utc>,
    raw: &RawRef,
    events: &mut Vec<EventRecord>,
) {
    events.push(EventRecord {
        id: stable_id(&[sid, "prompt", &raw.line.to_string()]),
        turn_id: tid.to_string(),
        session_id: sid.to_string(),
        ts,
        event: Event::UserPrompt {
            text: text.to_string(),
            attachments: vec![],
            is_meta: false,
        },
        raw_ref: raw.clone(),
    });
}

/// Flatten a `tool_result.content` (string, or an array of `{type:"text",text}`
/// blocks) into a single summary string.
fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Return the open turn, or open a fresh one (assistant role) when a non-message
/// record (system/result) arrives before any message turn exists — so its
/// `turn_id` always references a real `Turn` row (events→turns FK).
fn current_or_open(
    cur_turn: &mut Option<String>,
    turns: &mut Vec<Turn>,
    idx: &mut u32,
    sid: &str,
    is_root: bool,
    ts: DateTime<Utc>,
) -> String {
    if let Some(t) = cur_turn.clone() {
        return t;
    }
    *idx += 1;
    let tid = stable_id(&[sid, "turn", &idx.to_string()]);
    turns.push(Turn {
        id: tid.clone(),
        session_id: sid.to_string(),
        parent_id: None,
        role: Role::System,
        index: *idx,
        started_at: ts,
        duration_ms: None,
        is_sidechain: !is_root,
    });
    *cur_turn = Some(tid.clone());
    tid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// A disk-faithful two-session audit log: an orchestrator (root, id == dir uuid)
    /// that prompts a subagent, and the subagent (interleaved, different session_id)
    /// that thinks, calls a tool, gets a result, and answers.
    fn sample_audit() -> String {
        [
            // system init (carries cwd) — orchestrator session
            r#"{"_audit_timestamp":"2026-06-26T09:24:00.000Z","type":"system","session_id":"root-uuid","cwd":"/Users/k/WARDEN","message":{"subtype":"init"}}"#,
            // orchestrator user prompt
            r#"{"_audit_timestamp":"2026-06-26T09:24:01.000Z","type":"user","session_id":"root-uuid","message":{"role":"user","content":"build the thing"}}"#,
            // orchestrator dispatches a Task (subagent)
            r#"{"_audit_timestamp":"2026-06-26T09:24:02.000Z","type":"assistant","session_id":"root-uuid","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"toolu_sub","name":"Task","input":{"prompt":"go"}}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":50}}}"#,
            // subagent thinks + calls a tool (different session_id, interleaved)
            r#"{"_audit_timestamp":"2026-06-26T09:24:03.000Z","type":"assistant","session_id":"sub-uuid","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"let me look","signature":"x"},{"type":"tool_use","id":"toolu_read","name":"Read","input":{"file_path":"/x.rs"}}],"usage":{"input_tokens":200,"output_tokens":10}}}"#,
            // subagent tool result (user frame carrying tool_result)
            r#"{"_audit_timestamp":"2026-06-26T09:24:04.000Z","type":"user","session_id":"sub-uuid","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_read","content":"file body","is_error":false}]}}"#,
            // subagent final answer
            r#"{"_audit_timestamp":"2026-06-26T09:24:05.000Z","type":"assistant","session_id":"sub-uuid","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"text","text":"done reading"}]}}"#,
            // workflow result line (ignored as a SystemNotice)
            r#"{"_audit_timestamp":"2026-06-26T09:24:06.000Z","type":"result","session_id":"root-uuid","message":{"subtype":"success"}}"#,
        ]
        .join("\n")
            + "\n"
    }

    fn write_audit(dir: &Path, body: &str) -> PathBuf {
        let session_dir = dir.join("ws").join("ctx").join("local_root-uuid");
        std::fs::create_dir_all(&session_dir).unwrap();
        let p = session_dir.join("audit.jsonl");
        std::fs::write(&p, body).unwrap();
        p
    }

    /// Golden: one audit log → TWO sessions, split by session_id, with the right
    /// event mapping on each, the root flagged, and emerald-with-desktop meta.
    #[test]
    fn parse_audit_splits_sessions_and_maps_events() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_audit(dir.path(), &sample_audit());
        let bytes = std::fs::read(&p).unwrap();
        let batches = parse_audit(&p, &bytes, 0, hash64(&bytes)).unwrap();

        assert_eq!(batches.len(), 2, "one session per distinct session_id");
        let root = batches
            .iter()
            .find(|b| b.session.external_id == "root-uuid")
            .expect("root session present");
        let sub = batches
            .iter()
            .find(|b| b.session.external_id == "sub-uuid")
            .expect("subagent session present");

        // Harness stays emerald Claude; the desktop sub-label rides on meta.
        assert!(matches!(root.session.harness, Harness::ClaudeCode));
        assert_eq!(
            root.session.meta.get("originator").and_then(Value::as_str),
            Some("Claude Desktop")
        );
        assert_eq!(
            root.session.meta.get("desktop").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            root.session.meta.get("is_root").and_then(Value::as_bool),
            Some(true),
            "the session whose id == the local_<uuid> dir is the root"
        );
        assert_eq!(
            sub.session.meta.get("is_root").and_then(Value::as_bool),
            Some(false)
        );

        // cwd was lifted from the system init line.
        assert_eq!(
            root.session.project.as_ref().map(|pr| pr.cwd.clone()),
            Some(PathBuf::from("/Users/k/WARDEN"))
        );

        // Root events: a UserPrompt, a Task ToolCall (SubagentTask), a TokenUsage,
        // and the trailing `result` as a SystemNotice (no AssistantText → idle tail).
        let root_kinds: Vec<&str> = root.events.iter().map(|e| e.event.kind_name()).collect();
        assert!(root_kinds.contains(&"user_prompt"));
        assert!(root_kinds.contains(&"tool_call"));
        assert!(root_kinds.contains(&"token_usage"));
        assert!(root_kinds.contains(&"system_notice"));
        let task = root
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::ToolCall { tool, kind, .. } => Some((tool.clone(), kind.clone())),
                _ => None,
            })
            .expect("Task tool call");
        assert_eq!(task.0, "Task");
        assert!(matches!(task.1, ToolKind::SubagentTask));

        // Subagent events: Thinking, a Read ToolCall, a ToolResult paired by id, a
        // final AssistantText.
        let read_call = sub.events.iter().any(
            |e| matches!(&e.event, Event::ToolCall { tool, .. } if tool == "Read"),
        );
        assert!(read_call, "subagent Read tool call present");
        let res = sub
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::ToolResult { call_id, status, .. } => Some((call_id.clone(), status.clone())),
                _ => None,
            })
            .expect("tool result present");
        assert_eq!(res.0, "toolu_read", "tool_result pairs by tool_use_id");
        assert!(matches!(res.1, ToolStatus::Ok));
        assert!(
            sub.events
                .iter()
                .any(|e| matches!(&e.event, Event::AssistantText { text } if text == "done reading"))
        );

        // Every event references its file with a 1-based line and an in-file offset.
        for b in &batches {
            for e in &b.events {
                assert_eq!(e.raw_ref.source_path, p);
                assert!(e.raw_ref.line >= 1);
                assert!((e.raw_ref.offset as usize) < bytes.len());
            }
        }
        // Both batches report the file EOF as their watermark.
        assert_eq!(root.offset, bytes.len() as u64);
        assert_eq!(sub.offset, bytes.len() as u64);
    }

    /// End-to-end through the store: after backfill + linkage, the subagent's parent
    /// is the orchestrator root (dir-owner heuristic).
    #[test]
    fn link_desktop_subagents_in_store_nests_under_dir_owner() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_audit(dir.path(), &sample_audit());
        let store = Store::memory().unwrap();
        let adapter = ClaudeDesktopAdapter::with_root(dir.path().to_path_buf(), store.clone());

        for b in adapter.backfill().unwrap() {
            store
                .upsert_session_batch(&b.session, &b.turns, &b.events, b.offset)
                .unwrap();
        }
        let recorded = link_desktop_subagents_in_store(&store).unwrap();
        assert_eq!(recorded, 1, "exactly one subagent linked");

        let root_sid = stable_id(&["claude_desktop", "root-uuid", &p.to_string_lossy()]);
        let sub_sid = stable_id(&["claude_desktop", "sub-uuid", &p.to_string_lossy()]);
        assert_eq!(
            store.parent_of(&sub_sid).unwrap(),
            Some(root_sid.clone()),
            "subagent nests under the dir-owner orchestrator"
        );
        assert_eq!(
            store.parent_of(&root_sid).unwrap(),
            None,
            "the orchestrator is a root"
        );
    }

    /// Backfill is idempotent (whole-file hash dedup + stable ids).
    #[test]
    fn backfill_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        write_audit(dir.path(), &sample_audit());
        let store = Store::memory().unwrap();
        let adapter = ClaudeDesktopAdapter::with_root(dir.path().to_path_buf(), store.clone());

        for b in adapter.backfill().unwrap() {
            store
                .upsert_session_batch(&b.session, &b.turns, &b.events, b.offset)
                .unwrap();
        }
        let first = store.counts().unwrap();
        // A second backfill detects the unchanged file via source_raw_hash → no batches.
        let second_batches = adapter.backfill().unwrap();
        assert!(
            second_batches.is_empty(),
            "unchanged file must be skipped on re-backfill"
        );
        assert_eq!(store.counts().unwrap(), first);
        assert_eq!(first.0, 2, "two sessions ingested from one audit log");
    }

    /// Incremental tail: appending one assistant line for an existing session and
    /// parsing only the appended slice yields that session's batch with an absolute
    /// offset, landing on the SAME stable id as the backfill (so events merge).
    #[test]
    fn parse_range_tail_lands_on_same_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_audit(dir.path(), &sample_audit());
        let original = std::fs::read(&p).unwrap();
        let eof = original.len() as u64;

        let appended = "{\"_audit_timestamp\":\"2026-06-26T09:25:00.000Z\",\"type\":\"assistant\",\"session_id\":\"sub-uuid\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[{\"type\":\"text\",\"text\":\"freshly appended\"}]}}\n";
        let mut full = original.clone();
        full.extend_from_slice(appended.as_bytes());

        let store = Store::memory().unwrap();
        let adapter = ClaudeDesktopAdapter::with_root(dir.path().to_path_buf(), store);
        let slice = &full[eof as usize..];
        let batches = adapter.parse_range(&p, slice, eof, hash64(&full)).unwrap();

        assert_eq!(batches.len(), 1, "tail touches exactly one session");
        let b = &batches[0];
        assert_eq!(b.session.external_id, "sub-uuid");
        assert_eq!(
            b.session.id,
            stable_id(&["claude_desktop", "sub-uuid", &p.to_string_lossy()]),
            "tail-derived session id must equal the backfill session id"
        );
        let ev = b
            .events
            .iter()
            .find(|e| matches!(&e.event, Event::AssistantText { text } if text == "freshly appended"))
            .expect("appended event present");
        assert_eq!(ev.raw_ref.offset, eof, "tail offset must be absolute");
    }

    /// A non-audit `.jsonl` under the desktop tree (the nested `.claude/projects`
    /// schema) is ignored by parse_range — only `audit.jsonl` is ours.
    #[test]
    fn parse_range_ignores_non_audit_jsonl() {
        let store = Store::memory().unwrap();
        let adapter = ClaudeDesktopAdapter::with_root(PathBuf::from("/x"), store);
        let junk = PathBuf::from("/x/ws/ctx/local_root-uuid/.claude/projects/abc/abc.jsonl");
        let bytes = br#"{"content":"x","operation":"y","sessionId":"z"}"#;
        let batches = adapter.parse_range(&junk, bytes, 0, 0).unwrap();
        assert!(batches.is_empty(), "non-audit jsonl must be ignored");
    }

    /// A malformed/half-written line is skipped, not fatal (live-tail robustness).
    #[test]
    fn malformed_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "{}\n{{ this is not json\n",
            r#"{"_audit_timestamp":"2026-06-26T09:24:00.000Z","type":"assistant","session_id":"s","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#
        );
        let p = write_audit(dir.path(), &body);
        let bytes = std::fs::read(&p).unwrap();
        let batches = parse_audit(&p, &bytes, 0, hash64(&bytes)).unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0]
            .events
            .iter()
            .any(|e| matches!(&e.event, Event::AssistantText { text } if text == "hi")));
    }
}
