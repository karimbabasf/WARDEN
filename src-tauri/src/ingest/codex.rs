//! Codex transcript adapter.
//!
//! Codex rollouts live at `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl`
//! (plus `~/.codex/archived_sessions/**`). Each line is an envelope
//! `{timestamp, type, payload}`. We dispatch on the envelope `type` and then on
//! `payload.type`, mapping each record to the canonical IR (see the record→IR
//! table in `docs/.../m2-face.md` §2.3).
//!
//! Two correctness rules are baked in and documented at their call sites:
//!
//! * **Dedup** — Codex logs the same user / assistant text under BOTH an
//!   `event_msg/{user,agent}_message` record AND a `response_item/message`
//!   record. We treat the `event_msg` form as canonical and SKIP
//!   `response_item/message` so prompts/answers are not double-counted. The
//!   reasoning / function_call / function_call_output `response_item`s have no
//!   `event_msg` twin and are kept.
//! * **Tokens** — `event_msg/token_count` carries both a cumulative
//!   `total_token_usage` and a per-event `last_token_usage` under `payload.info`.
//!   We emit the per-event *delta* (`last_token_usage`); `info` may be `null`
//!   early in a session, in which case no `TokenUsage` is emitted.

use super::{Adapter, SessionBatch};
use crate::ir::*;
use crate::store::Store;
use crate::util::{
    default_codex_archived_sessions, default_codex_sessions, hash64, parse_ts, repo_root,
    stable_id, truncate_chars,
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct CodexAdapter {
    pub root: PathBuf,
    pub archived_root: PathBuf,
    pub store: Store,
    pub max_files: Option<usize>,
}

impl CodexAdapter {
    pub fn new(store: Store) -> Self {
        Self {
            root: default_codex_sessions(),
            archived_root: default_codex_archived_sessions(),
            store,
            max_files: None,
        }
    }
    pub fn with_root(root: PathBuf, archived_root: PathBuf, store: Store) -> Self {
        Self {
            root,
            archived_root,
            store,
            max_files: None,
        }
    }
}

impl Adapter for CodexAdapter {
    fn harness(&self) -> Harness {
        Harness::Codex
    }

    fn detect(&self) -> Result<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for root in [&self.root, &self.archived_root] {
            if !root.exists() {
                continue;
            }
            paths.extend(
                WalkDir::new(root)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_type().is_file()
                            && e.path().extension().map(|x| x == "jsonl").unwrap_or(false)
                            && e.path()
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.starts_with("rollout-"))
                                .unwrap_or(false)
                    })
                    .map(|e| e.into_path()),
            );
        }
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
            if self
                .store
                .source_raw_hash(&p)?
                .is_some_and(|h| h == raw_hash)
            {
                continue;
            }
            match parse_file(&p, &bytes, raw_hash) {
                Ok(b) => out.push(b),
                Err(e) => {
                    tracing::warn!(path=%p.display(), error=?e, "skipping malformed Codex rollout")
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
        // offset 0 → full parse (session_meta is present, resolves external_id).
        // offset > 0 → incremental tail: `bytes` is the slice starting at
        // `start_offset`. No `session_meta` is in the slice, so the session is
        // derived from the path (`external_id_from_filename`) and every line's
        // local offset is shifted to an ABSOLUTE position by `start_offset`.
        let batch = if start_offset == 0 {
            parse_file(path, bytes, raw_hash)?
        } else {
            parse_slice(
                path,
                bytes,
                start_offset,
                raw_hash,
                Some(external_id_from_filename(path)),
            )?
        };
        Ok(vec![batch])
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.root.clone(), self.archived_root.clone()]
    }
}

/// Derive a session external id from the rollout filename, e.g.
/// `rollout-2026-06-19T09-33-00-019ee0ba-...uuid....jsonl` → the trailing uuid.
/// Used as a fallback when a batch lacks a `session_meta` record (a tail parse,
/// landing in Task 4); for offset-0 full parses `session_meta.payload.id` wins.
pub fn external_id_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    // Filename layout: rollout-<ISO timestamp with dashes>-<uuid>. The uuid is the
    // last 5 dash-separated groups; recovering it cheaply: take everything after the
    // "rollout-" prefix and after the ISO date+time. Simplest robust heuristic:
    // the uuid groups are hex; grab the last run that looks like a uuid tail.
    if let Some(idx) = stem.find("rollout-") {
        let rest = &stem[idx + "rollout-".len()..];
        // A v7 uuid is 36 chars (8-4-4-4-12). Take the trailing 36 if present.
        if rest.len() >= 36 {
            let tail = &rest[rest.len() - 36..];
            if tail.split('-').count() == 5 {
                return tail.to_string();
            }
        }
    }
    if stem.is_empty() {
        stable_id(&[&path.to_string_lossy()])
    } else {
        stem
    }
}

/// Full (offset-0) parse: `session_meta` is present, so the external id is
/// resolved from the records (falling back to the filename uuid).
fn parse_file(path: &Path, bytes: &[u8], raw_hash: u64) -> Result<SessionBatch> {
    parse_slice(path, bytes, 0, raw_hash, None)
}

/// RADAR (Task 5): resolve Codex Desktop `parent_thread_id` links over every
/// persisted Codex session and record them via `Store::link_child_session`.
///
/// The pure resolver ([`crate::radar::hierarchy::link_codex_subagents`]) returns
/// `(child_external_id, parent_external_id)` pairs; this maps both back to their
/// stored session ids before persisting. Idempotent (re-recording the same parent
/// is a plain UPDATE), so startup/live ingest can call it before RADAR refreshes its
/// cached forest. Returns the number of links recorded. VS Code Codex sessions yield
/// no pairs (they are never `thread_source == "subagent"`).
pub fn link_codex_subagents_in_store(store: &Store) -> Result<usize> {
    let sessions = store.sessions()?;
    // external_id → session id, restricted to Codex rows (Claude shares the table).
    let mut ext_to_sid = std::collections::HashMap::<&str, &str>::new();
    for s in &sessions {
        if matches!(s.harness, Harness::Codex) {
            ext_to_sid.insert(s.external_id.as_str(), s.id.as_str());
        }
    }
    let codex_sessions: Vec<Session> = sessions
        .iter()
        .filter(|s| matches!(s.harness, Harness::Codex))
        .cloned()
        .collect();
    let pairs = crate::radar::hierarchy::link_codex_subagents(&codex_sessions);
    let mut recorded = 0;
    for (child_ext, parent_ext) in pairs {
        if let (Some(child_sid), Some(parent_sid)) = (
            ext_to_sid.get(child_ext.as_str()),
            ext_to_sid.get(parent_ext.as_str()),
        ) {
            store.link_child_session(child_sid, parent_sid)?;
            recorded += 1;
        }
    }
    Ok(recorded)
}

/// Core parser shared by the full parse and the incremental tail.
///
/// * `base_offset` is added to every line's local byte position so `RawRef.offset`
///   and the returned watermark are ABSOLUTE positions in the on-disk file. For a
///   full parse it is 0; for a tail parse it is the saved watermark (`bytes` is the
///   slice from that offset to EOF).
/// * `external_id_override` short-circuits session-id resolution. A tail slice has
///   no `session_meta`, so the caller passes the path-derived id; this keeps the
///   tail's `Session::id` identical to the original full-parse session id (same
///   `stable_id` inputs), so events/turns attach to the already-backfilled row.
fn parse_slice(
    path: &Path,
    bytes: &[u8],
    base_offset: u64,
    raw_hash: u64,
    external_id_override: Option<String>,
) -> Result<SessionBatch> {
    // Track the byte position of each line's start into `offset` so RawRef and the
    // persisted watermark agree (FSEvents coalesces writes — we seek to this byte
    // offset and read to EOF). `line_no` is 1-based within the slice. `offset`
    // starts at `base_offset` so positions are absolute in the file.
    let mut offset = base_offset;
    let mut line_no = 0u32;
    let mut raw_records: Vec<(u64, u32, Value)> = Vec::new();
    for raw_line in bytes.split_inclusive(|b| *b == b'\n') {
        line_no += 1;
        let line_len = raw_line.len() as u64;
        let line = String::from_utf8_lossy(raw_line);
        let line = line.trim_end_matches(['\n', '\r']);
        if !line.trim().is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                raw_records.push((offset, line_no, v));
            }
        }
        offset += line_len;
    }
    if raw_records.is_empty() {
        anyhow::bail!("empty jsonl");
    }

    // Resolve the session external id. A tail parse passes an override (the
    // session_meta record is not in the slice); otherwise take it from the first
    // `session_meta` record, falling back to the rollout filename uuid.
    let external_id = external_id_override.unwrap_or_else(|| {
        raw_records
            .iter()
            .find_map(|(_, _, v)| {
                if v.get("type").and_then(Value::as_str) == Some("session_meta") {
                    v.pointer("/payload/id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| external_id_from_filename(path))
    });

    let sid = stable_id(&["codex", &external_id, &path.to_string_lossy()]);

    let mut turns: Vec<Turn> = Vec::new();
    let mut events: Vec<EventRecord> = Vec::new();
    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut started = None;
    let mut ended = None;
    let mut project: Option<ProjectRef> = None;
    let mut idx = 0u32;
    // The id of the Turn currently accepting events. task_started opens it,
    // task_complete/turn_aborted close it. Records that arrive with no open turn
    // (e.g. turn_context before task_started) open a fresh synthetic turn.
    let mut cur_turn: Option<String> = None;
    let mut meta = json!({"ignored_record_types": {}});

    // Open a new Turn and make it current. `role` is Assistant for model turns,
    // System for mode/boundary turns.
    macro_rules! open_turn {
        ($ts:expr, $role:expr, $seed:expr) => {{
            idx += 1;
            let tid = stable_id(&[&sid, $seed, &idx.to_string()]);
            turns.push(Turn {
                id: tid.clone(),
                session_id: sid.clone(),
                parent_id: None,
                role: $role,
                index: idx,
                started_at: $ts,
                duration_ms: None,
                is_sidechain: false,
            });
            cur_turn = Some(tid.clone());
            tid
        }};
    }

    for (off, ln, v) in &raw_records {
        let ts = parse_ts(v.get("timestamp"));
        if started.map(|s| ts < s).unwrap_or(true) {
            started = Some(ts);
        }
        if ended.map(|e| ts > e).unwrap_or(true) {
            ended = Some(ts);
        }
        let raw = RawRef {
            source_path: path.to_path_buf(),
            offset: *off,
            line: *ln,
        };
        let rec_type = v.get("type").and_then(Value::as_str).unwrap_or("unknown");
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);
        let pt = payload.get("type").and_then(Value::as_str).unwrap_or("");
        remember_model_context_window(&mut meta, &payload);

        match rec_type {
            "session_meta" => {
                if let Some(prov) = payload.get("model_provider").and_then(Value::as_str) {
                    models.insert(prov.to_string());
                }
                if project.is_none() {
                    if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                        let cwdp = PathBuf::from(cwd);
                        project = Some(ProjectRef {
                            cwd: cwdp.clone(),
                            repo_root: repo_root(&cwdp),
                            git_branch: None,
                        });
                    }
                }
                // RADAR (Task 4): preserve the Codex Desktop multi-agent linkage +
                // honesty fields so the hierarchy pass (Task 5) can pair
                // `parent_thread_id → id` and the viz can keep VS Code Codex flat
                // (subagents exist only for `originator == "Codex Desktop"`).
                if let Some(obj) = meta.as_object_mut() {
                    for key in [
                        "thread_source",
                        "parent_thread_id",
                        "agent_role",
                        "agent_nickname",
                        "multi_agent_version",
                        "originator",
                    ] {
                        if let Some(val) = payload.get(key) {
                            if !val.is_null() {
                                obj.insert(key.to_string(), val.clone());
                            }
                        }
                    }
                }
            }
            "turn_context" => {
                // A new turn boundary. Surface the collaboration mode (defaults to
                // "default") and open a fresh System turn to carry the ModeChange.
                let mode = payload
                    .pointer("/collaboration_mode/mode")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_string();
                // NB: `model_ids` is the harness *provider* identity (from session_meta's
                // `model_provider`), not the per-turn model — do not collect `payload.model`
                // here or the provider id gets polluted with turn-level model names.
                let tid = open_turn!(ts, Role::System, "mode");
                events.push(EventRecord {
                    id: stable_id(&[&tid, "mode"]),
                    turn_id: tid,
                    session_id: sid.clone(),
                    ts,
                    event: Event::ModeChange { mode },
                    raw_ref: raw,
                });
            }
            "event_msg" => match pt {
                "task_started" => {
                    // Open the assistant turn that subsequent events attach to.
                    open_turn!(ts, Role::Assistant, "turn");
                }
                "task_complete" => {
                    cur_turn = None;
                }
                "turn_aborted" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "abort"]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::Error {
                            source: "codex".to_string(),
                            message: "turn_aborted".to_string(),
                        },
                        raw_ref: raw,
                    });
                    cur_turn = None;
                }
                "user_message" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    let text = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "prompt", &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::UserPrompt {
                            text,
                            attachments: vec![],
                            is_meta: false,
                        },
                        raw_ref: raw,
                    });
                }
                "agent_message" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    let text = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "text", &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::AssistantText { text },
                        raw_ref: raw,
                    });
                }
                "token_count" => {
                    // Per-event delta only. `info` is null early in a session — skip then.
                    if let Some(last) = payload.pointer("/info/last_token_usage") {
                        let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                        let model = models.iter().next().cloned().unwrap_or_default();
                        events.push(EventRecord {
                            id: stable_id(&[&tid, "usage", &ln.to_string()]),
                            turn_id: tid,
                            session_id: sid.clone(),
                            ts,
                            event: Event::TokenUsage {
                                input: last
                                    .get("input_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0) as u32,
                                output: last
                                    .get("output_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0) as u32,
                                cache_creation: 0,
                                cache_read: last
                                    .get("cached_input_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                                    as u32,
                                model,
                                orchestration: None,
                            },
                            raw_ref: raw,
                        });
                    }
                }
                "patch_apply_end" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    let files = payload
                        .get("changes")
                        .and_then(Value::as_object)
                        .map(|o| {
                            o.keys()
                                .take(500)
                                .map(|k| FileEdit {
                                    path: k.clone(),
                                    ..Default::default()
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "files", &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::FileSnapshot { files },
                        raw_ref: raw,
                    });
                }
                _ => record_unknown(
                    &mut meta,
                    &mut events,
                    &mut turns,
                    &mut cur_turn,
                    &mut idx,
                    &sid,
                    ts,
                    raw,
                    rec_type,
                    pt,
                    &payload,
                ),
            },
            "response_item" => match pt {
                // Dedup: the canonical text already arrived via event_msg.
                "message" => {}
                "reasoning" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    // Plaintext reasoning lives in `summary[].text` when present; otherwise
                    // only `encrypted_content` exists and we record 0 thinking tokens.
                    let text = payload
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "thinking", &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::Thinking {
                            tokens: (text.len() / 4) as u32,
                        },
                        raw_ref: raw,
                    });
                }
                "function_call" | "custom_tool_call" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    // Codex MCP tools are named `server__tool`; everything else is opaque.
                    let kind = if name.contains("__") {
                        ToolKind::Mcp
                    } else {
                        ToolKind::Unknown
                    };
                    // `arguments` is a JSON-encoded *string*; parse it back to a value.
                    let input = payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .or_else(|| payload.get("arguments").cloned())
                        .unwrap_or(Value::Null);
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "tool", &call_id, &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::ToolCall {
                            tool: name,
                            input,
                            call_id,
                            kind,
                        },
                        raw_ref: raw,
                    });
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let tid = current_or_open(&mut cur_turn, &mut turns, &mut idx, &sid, ts);
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    // `output` is a plain string; Codex embeds the process exit status in it.
                    let output = payload
                        .get("output")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_default();
                    let status = if output_is_error(&payload, &output) {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Ok
                    };
                    events.push(EventRecord {
                        id: stable_id(&[&tid, "tool_result", &call_id, &ln.to_string()]),
                        turn_id: tid,
                        session_id: sid.clone(),
                        ts,
                        event: Event::ToolResult {
                            call_id,
                            status,
                            bytes: output.len() as u64,
                            summary: Some(truncate_chars(&output, 500)),
                        },
                        raw_ref: raw,
                    });
                }
                _ => record_unknown(
                    &mut meta,
                    &mut events,
                    &mut turns,
                    &mut cur_turn,
                    &mut idx,
                    &sid,
                    ts,
                    raw,
                    rec_type,
                    pt,
                    &payload,
                ),
            },
            _ => record_unknown(
                &mut meta,
                &mut events,
                &mut turns,
                &mut cur_turn,
                &mut idx,
                &sid,
                ts,
                raw,
                rec_type,
                pt,
                &payload,
            ),
        }
    }

    Ok(SessionBatch {
        session: Session {
            id: sid,
            harness: Harness::Codex,
            external_id,
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
        offset,
    })
}

fn remember_model_context_window(meta: &mut Value, payload: &Value) {
    let window = payload
        .get("model_context_window")
        .and_then(Value::as_u64)
        .or_else(|| {
            payload
                .pointer("/info/model_context_window")
                .and_then(Value::as_u64)
        })
        .filter(|n| *n > 0);
    let Some(window) = window else {
        return;
    };
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("model_context_window".to_string(), json!(window));
    }
}

/// Return the currently-open turn id, opening a fresh assistant turn if none is
/// active (records can legitimately arrive before/after a task boundary).
fn current_or_open(
    cur_turn: &mut Option<String>,
    turns: &mut Vec<Turn>,
    idx: &mut u32,
    sid: &str,
    ts: chrono::DateTime<Utc>,
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
        role: Role::Assistant,
        index: *idx,
        started_at: ts,
        duration_ms: None,
        is_sidechain: false,
    });
    *cur_turn = Some(tid.clone());
    tid
}

/// True when a function_call_output marks a failure. Codex has no structured
/// error flag on the output record, so we look for an explicit error field and,
/// failing that, the conventional non-zero process-exit marker in the text.
fn output_is_error(payload: &Value, output: &str) -> bool {
    if payload
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if payload.get("success").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    output.contains("Process exited with code ") && !output.contains("Process exited with code 0")
}

/// Schema drift never drops a session: any record type we don't model becomes a
/// `SystemNotice` whose subtype is `"{type}/{payload.type}"` and whose data is the
/// raw payload. (We also tally the type into `session.meta.ignored_record_types`.)
/// The notice attaches to the current open turn — opening one if needed — so its
/// `turn_id` references a real `Turn` row (the events→turns FK requires this).
#[allow(clippy::too_many_arguments)]
fn record_unknown(
    meta: &mut Value,
    events: &mut Vec<EventRecord>,
    turns: &mut Vec<Turn>,
    cur_turn: &mut Option<String>,
    idx: &mut u32,
    sid: &str,
    ts: chrono::DateTime<Utc>,
    raw: RawRef,
    rec_type: &str,
    payload_type: &str,
    payload: &Value,
) {
    let subtype = if payload_type.is_empty() {
        rec_type.to_string()
    } else {
        format!("{rec_type}/{payload_type}")
    };
    if let Some(obj) = meta
        .get_mut("ignored_record_types")
        .and_then(Value::as_object_mut)
    {
        let n = obj.get(&subtype).and_then(Value::as_u64).unwrap_or(0) + 1;
        obj.insert(subtype.clone(), json!(n));
    }
    let tid = current_or_open(cur_turn, turns, idx, sid, ts);
    events.push(EventRecord {
        id: stable_id(&[sid, "notice", &raw.line.to_string()]),
        turn_id: tid,
        session_id: sid.to_string(),
        ts,
        event: Event::SystemNotice {
            subtype,
            data: payload.clone(),
        },
        raw_ref: raw,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::util::hash64;
    use serde_json::Value;
    use std::path::PathBuf;

    /// Path to a fixture file under `src-tauri/tests/fixtures/`, resolved
    /// relative to the crate manifest so it works regardless of cwd.
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    /// Golden test: parse the hand-built (but disk-faithful) Codex rollout and
    /// assert every branch of the record→IR mapping table, including the dedup
    /// rule (redundant `response_item/message` produces no duplicate prompt),
    /// the per-event `last_token_usage` token rule, and the unknown→SystemNotice
    /// schema-drift rule. The normalized event-kind sequence is compared to the
    /// committed `codex_expected_ir.json`.
    #[test]
    fn golden_codex_rollout_maps_every_branch() {
        let path = fixture("codex_rollout_sample.jsonl");
        let bytes = std::fs::read(&path).expect("read fixture jsonl");
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(fixture("codex_expected_ir.json")).expect("read expected ir"),
        )
        .expect("parse expected ir");

        let store = Store::memory().unwrap();
        let adapter = CodexAdapter::with_root(
            path.parent().unwrap().to_path_buf(),
            path.parent().unwrap().to_path_buf(),
            store,
        );
        let batches = adapter
            .parse_range(&path, &bytes, 0, hash64(&bytes))
            .expect("parse_range ok");

        // Exactly one Session.
        assert_eq!(batches.len(), 1, "expected exactly one session batch");
        let b = &batches[0];

        // harness == Codex.
        assert!(
            matches!(b.session.harness, Harness::Codex),
            "harness must be Codex"
        );

        // external_id from session_meta.payload.id.
        assert_eq!(
            b.session.external_id,
            expected["external_id"].as_str().unwrap(),
            "external_id must come from session_meta payload id"
        );

        // model_ids == ["openai"] (from model_provider).
        let want_models: Vec<String> = expected["model_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(b.session.model_ids, want_models, "model_ids mismatch");
        assert_eq!(
            b.session
                .meta
                .get("model_context_window")
                .and_then(Value::as_u64),
            Some(258_400),
            "Codex model_context_window must be preserved for RADAR window sizing"
        );

        // project.cwd from session_meta.payload.cwd.
        let project = b.session.project.as_ref().expect("project set from cwd");
        assert_eq!(
            project.cwd,
            PathBuf::from("/Users/dev/Projects/sample-project")
        );

        // Normalized event-kind sequence equals the golden file.
        let want_kinds: Vec<String> = expected["event_kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let got_kinds: Vec<String> = b
            .events
            .iter()
            .map(|e| e.event.kind_name().to_string())
            .collect();
        assert_eq!(
            got_kinds, want_kinds,
            "event-kind sequence mismatch\n got: {got_kinds:?}\nwant: {want_kinds:?}"
        );

        // Dedup rule: the redundant response_item/message produced NO extra UserPrompt.
        let prompt_count = b
            .events
            .iter()
            .filter(|e| matches!(e.event, Event::UserPrompt { .. }))
            .count();
        assert_eq!(
            prompt_count, 1,
            "redundant response_item/message must not duplicate the user prompt"
        );

        // The unknown record produced a SystemNotice with the right subtype.
        let notice = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::SystemNotice { subtype, .. } => Some(subtype.clone()),
                _ => None,
            })
            .expect("unknown record must produce a SystemNotice");
        assert_eq!(
            notice,
            expected["system_notice_subtype"].as_str().unwrap(),
            "SystemNotice subtype must be '{{type}}/{{payload.type}}'"
        );

        // The `turn_aborted` record maps to Event::Error{source:"codex",message:"turn_aborted"}.
        let (err_source, err_message) = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::Error { source, message } => Some((source.clone(), message.clone())),
                _ => None,
            })
            .expect("turn_aborted must produce an Error event");
        assert_eq!(err_source, "codex", "Error.source must be 'codex'");
        assert_eq!(
            err_message, "turn_aborted",
            "Error.message must be 'turn_aborted'"
        );

        // TokenUsage equals the per-event last_token_usage numbers.
        let (input, output, cache_read, cache_creation) = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::TokenUsage {
                    input,
                    output,
                    cache_read,
                    cache_creation,
                    ..
                } => Some((*input, *output, *cache_read, *cache_creation)),
                _ => None,
            })
            .expect("a TokenUsage event must exist");
        assert_eq!(
            input,
            expected["token_usage"]["input"].as_u64().unwrap() as u32
        );
        assert_eq!(
            output,
            expected["token_usage"]["output"].as_u64().unwrap() as u32
        );
        assert_eq!(
            cache_read,
            expected["token_usage"]["cache_read"].as_u64().unwrap() as u32
        );
        assert_eq!(
            cache_creation,
            expected["token_usage"]["cache_creation"].as_u64().unwrap() as u32
        );

        // The function_call/function_call_output pair produced a ToolCall + ToolResult
        // sharing call_id, and the MCP-style name ("__") maps to ToolKind::Mcp.
        let (call_tool, call_id, call_kind) = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::ToolCall {
                    tool,
                    call_id,
                    kind,
                    ..
                } => Some((tool.clone(), call_id.clone(), kind.clone())),
                _ => None,
            })
            .expect("a ToolCall event must exist");
        assert_eq!(call_tool, expected["tool_call"]["tool"].as_str().unwrap());
        assert_eq!(call_id, expected["tool_call"]["call_id"].as_str().unwrap());
        assert!(
            matches!(call_kind, ToolKind::Mcp),
            "tool name containing '__' must map to ToolKind::Mcp"
        );

        let (res_call_id, res_status) = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::ToolResult {
                    call_id, status, ..
                } => Some((call_id.clone(), status.clone())),
                _ => None,
            })
            .expect("a ToolResult event must exist");
        assert_eq!(
            res_call_id, call_id,
            "ToolResult.call_id must match ToolCall.call_id"
        );
        assert!(
            matches!(res_status, ToolStatus::Ok),
            "successful output (exit code 0) must be ToolStatus::Ok"
        );

        // Every event carries a RawRef pointing at the fixture file with a real line number.
        for e in &b.events {
            assert_eq!(e.raw_ref.source_path, path, "raw_ref.source_path mismatch");
            assert!(e.raw_ref.line >= 1, "raw_ref.line must be 1-based");
        }
    }

    /// Task 4 (RADAR): a `session_meta` whose payload carries the Codex Desktop
    /// multi-agent fields propagates them into `Session.meta` so the hierarchy and
    /// honest-viz passes can read `thread_source`/`parent_thread_id`/`originator`.
    #[test]
    fn session_meta_captures_multi_agent_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-sub.jsonl");
        std::fs::write(
            &p,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"cwd\":\"/tmp\",\"model_provider\":\"openai\",\"thread_source\":\"subagent\",\"parent_thread_id\":\"P\",\"agent_role\":\"explorer\",\"agent_nickname\":\"Dirac\",\"multi_agent_version\":\"v1\",\"originator\":\"Codex Desktop\"}}\n",
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();
        let m = &b.session.meta;
        assert_eq!(
            m.get("thread_source").and_then(Value::as_str),
            Some("subagent")
        );
        assert_eq!(m.get("parent_thread_id").and_then(Value::as_str), Some("P"));
        assert_eq!(
            m.get("agent_role").and_then(Value::as_str),
            Some("explorer")
        );
        assert_eq!(
            m.get("agent_nickname").and_then(Value::as_str),
            Some("Dirac")
        );
        assert_eq!(
            m.get("multi_agent_version").and_then(Value::as_str),
            Some("v1")
        );
        assert_eq!(
            m.get("originator").and_then(Value::as_str),
            Some("Codex Desktop")
        );
    }

    /// Task 5 end-to-end: two Codex rollouts — a parent (external_id "P") and a
    /// Desktop subagent whose `session_meta` has `thread_source:"subagent"` and
    /// `parent_thread_id:"P"` — link in the store after `link_codex_subagents_in_store`.
    #[test]
    fn link_codex_subagents_in_store_persists_parent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::memory().unwrap();

        // Parent rollout (a normal user session).
        let parent_path = dir.path().join("rollout-P.jsonl");
        std::fs::write(
            &parent_path,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"P\",\"cwd\":\"/tmp\",\"model_provider\":\"openai\",\"thread_source\":\"user\"}}\n",
        )
        .unwrap();
        let pb = parse_file(&parent_path, &std::fs::read(&parent_path).unwrap(), 1).unwrap();
        store
            .upsert_session_batch(&pb.session, &pb.turns, &pb.events, pb.offset)
            .unwrap();

        // Child Desktop subagent rollout pointing at parent thread "P".
        let child_path = dir.path().join("rollout-C.jsonl");
        std::fs::write(
            &child_path,
            "{\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"C\",\"cwd\":\"/tmp\",\"model_provider\":\"openai\",\"thread_source\":\"subagent\",\"parent_thread_id\":\"P\",\"agent_nickname\":\"Dirac\",\"originator\":\"Codex Desktop\"}}\n",
        )
        .unwrap();
        let cb = parse_file(&child_path, &std::fs::read(&child_path).unwrap(), 2).unwrap();
        store
            .upsert_session_batch(&cb.session, &cb.turns, &cb.events, cb.offset)
            .unwrap();

        let recorded = link_codex_subagents_in_store(&store).unwrap();
        assert_eq!(recorded, 1, "exactly one Codex Desktop link recorded");
        assert_eq!(
            store.parent_of(&cb.session.id).unwrap(),
            Some(pb.session.id.clone()),
            "the subagent's parent must be persisted by external_id → session_id mapping"
        );
        // The parent itself has no parent (it is a root).
        assert_eq!(store.parent_of(&pb.session.id).unwrap(), None);
    }

    /// A `token_count` whose `info` is `null` (the real shape early in a session)
    /// must NOT emit a TokenUsage event.
    #[test]
    fn token_count_with_null_info_emits_no_usage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-x.jsonl");
        std::fs::write(
            &p,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"abc\",\"cwd\":\"/tmp\",\"model_provider\":\"openai\"}}\n{\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":null,\"rate_limits\":null}}\n",
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();
        assert!(
            !b.events
                .iter()
                .any(|e| matches!(e.event, Event::TokenUsage { .. })),
            "null info.last_token_usage must not produce a TokenUsage event"
        );
    }

    /// A function_call_output whose text marks a non-zero exit maps to Error.
    #[test]
    fn function_call_output_nonzero_exit_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-y.jsonl");
        std::fs::write(
            &p,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"abc\",\"cwd\":\"/tmp\",\"model_provider\":\"openai\"}}\n{\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"c\",\"output\":\"Process exited with code 1\\nboom\\n\"}}\n",
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();
        let status = b
            .events
            .iter()
            .find_map(|e| match &e.event {
                Event::ToolResult { status, .. } => Some(status.clone()),
                _ => None,
            })
            .expect("ToolResult present");
        assert!(matches!(status, ToolStatus::Error));
    }

    /// Byte-offset watermark contract: the returned `offset` equals the file
    /// length, and each event's RawRef offset points at the start of its line.
    #[test]
    fn offsets_track_line_starts_and_eof() {
        let path = fixture("codex_rollout_sample.jsonl");
        let bytes = std::fs::read(&path).unwrap();
        let b = parse_file(&path, &bytes, hash64(&bytes)).unwrap();
        assert_eq!(
            b.offset,
            bytes.len() as u64,
            "returned watermark must be EOF byte length"
        );
        // For each event, the bytes at [offset..] must begin the JSON line it came from.
        for e in &b.events {
            let off = e.raw_ref.offset as usize;
            assert!(off < bytes.len(), "offset within file");
            assert_eq!(
                bytes[off], b'{',
                "raw_ref.offset must point at a line start"
            );
        }
    }

    /// Ingesting the same rollout twice is idempotent: stable ids dedup so the
    /// second `upsert_session_batch` leaves `counts()` unchanged.
    #[test]
    fn ingest_is_idempotent() {
        let path = fixture("codex_rollout_sample.jsonl");
        let bytes = std::fs::read(&path).unwrap();
        let store = Store::memory().unwrap();

        let b1 = parse_file(&path, &bytes, hash64(&bytes)).unwrap();
        store
            .upsert_session_batch(&b1.session, &b1.turns, &b1.events, b1.offset)
            .unwrap();
        let first = store.counts().unwrap();

        let b2 = parse_file(&path, &bytes, hash64(&bytes)).unwrap();
        store
            .upsert_session_batch(&b2.session, &b2.turns, &b2.events, b2.offset)
            .unwrap();
        let second = store.counts().unwrap();

        assert_eq!(
            first, second,
            "second ingest must not change counts (stable-id dedup)"
        );
        assert_eq!(first.0, 1, "exactly one session ingested");
    }

    /// Incremental tail parse: starting from the EOF offset of the original file,
    /// appending exactly one `agent_message` line and parsing only the appended
    /// slice yields exactly ONE new event whose absolute `RawRef.offset` equals
    /// the original EOF (the start of the appended line) and whose text matches.
    #[test]
    fn parse_range_incremental_offset_yields_only_appended_event() {
        // Use a realistically-named rollout file: its filename encodes the SAME
        // session uuid as the in-file `session_meta.payload.id`. This is what real
        // Codex rollouts do, and it is what lets a tail parse (which derives the id
        // from the filename) land on the SAME session id as the full backfill.
        let src = fixture("codex_rollout_sample.jsonl");
        let original = std::fs::read(&src).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-19T16-33-00-019ee0ba-8295-7ba0-9971-c5af95e77191.jsonl");
        std::fs::write(&path, &original).unwrap();
        let original_eof = original.len() as u64;

        // Parse the original fully to know the baseline event count.
        let base = parse_file(&path, &original, hash64(&original)).unwrap();
        let base_eof = base.offset;
        assert_eq!(base_eof, original_eof, "baseline watermark must be EOF");

        // Append one new agent_message line to the byte buffer (simulating a write).
        let appended_line =
            b"{\"timestamp\":\"2026-06-19T16:50:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"freshly appended\"}}\n";
        let mut full = original.clone();
        full.extend_from_slice(appended_line);

        // Tail parse: feed ONLY the appended slice with start_offset = original EOF.
        let slice = &full[original_eof as usize..];
        let store = Store::memory().unwrap();
        let adapter =
            CodexAdapter::with_root(dir.path().to_path_buf(), dir.path().to_path_buf(), store);
        let batches = adapter
            .parse_range(&path, slice, original_eof, hash64(&full))
            .expect("tail parse_range ok");
        assert_eq!(batches.len(), 1, "tail parse yields one session batch");
        let b = &batches[0];

        // Exactly one new event (the appended agent_message → AssistantText).
        assert_eq!(
            b.events.len(),
            1,
            "exactly one appended event, got kinds: {:?}",
            b.events
                .iter()
                .map(|e| e.event.kind_name())
                .collect::<Vec<_>>()
        );
        let ev = &b.events[0];
        assert!(
            matches!(&ev.event, Event::AssistantText { text } if text == "freshly appended"),
            "appended event must be the AssistantText we wrote"
        );

        // Absolute offset must equal the original EOF (start of the appended line).
        assert_eq!(
            ev.raw_ref.offset, original_eof,
            "RawRef.offset must be ABSOLUTE (start_offset + local), i.e. original EOF"
        );
        // The returned watermark must equal the new full length.
        assert_eq!(
            b.offset,
            full.len() as u64,
            "tail watermark must advance to the new EOF"
        );
        // Session id must match the full-parse session id (derived from the same path).
        assert_eq!(
            b.session.id, base.session.id,
            "tail-derived session id must equal the full-parse session id"
        );
    }

    /// A tail parse has no `session_meta` in its slice, so the derived
    /// `external_id` must fall back to the filename ULID — exercising
    /// `external_id_from_filename`.
    #[test]
    fn external_id_from_filename_recovers_uuid_tail() {
        let p = PathBuf::from(
            "/Users/x/.codex/sessions/2026/06/19/rollout-2026-06-19T09-33-00-019ee0ba-8295-7ba0-9971-c5af95e77191.jsonl",
        );
        assert_eq!(
            external_id_from_filename(&p),
            "019ee0ba-8295-7ba0-9971-c5af95e77191",
            "filename fallback must recover the trailing uuid"
        );
        // No uuid tail → stem is returned verbatim.
        let q = PathBuf::from("/tmp/weird-name.jsonl");
        assert_eq!(external_id_from_filename(&q), "weird-name");
    }

    /// `roots()` returns both the live and archived session roots.
    #[test]
    fn roots_returns_live_and_archived() {
        let store = Store::memory().unwrap();
        let adapter = CodexAdapter::with_root(PathBuf::from("/a"), PathBuf::from("/b"), store);
        let roots = adapter.roots();
        assert_eq!(roots, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
