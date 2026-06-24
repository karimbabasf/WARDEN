use super::{Adapter, SessionBatch};
use crate::ir::*;
use crate::store::Store;
use crate::util::{
    default_claude_projects, hash64, parse_ts, repo_root, stable_id, truncate_chars,
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ClaudeCodeAdapter {
    pub root: PathBuf,
    pub store: Store,
    pub max_files: Option<usize>,
}
impl ClaudeCodeAdapter {
    pub fn new(store: Store) -> Self {
        Self {
            root: default_claude_projects(),
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
impl Adapter for ClaudeCodeAdapter {
    fn harness(&self) -> Harness {
        Harness::ClaudeCode
    }
    fn detect(&self) -> Result<Vec<PathBuf>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut paths: Vec<PathBuf> = WalkDir::new(&self.root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path().extension().map(|x| x == "jsonl").unwrap_or(false)
            })
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
                    tracing::warn!(path=%p.display(), error=?e, "skipping malformed Claude transcript")
                }
            }
        }
        Ok(out)
    }

    fn parse_range(
        &self,
        path: &std::path::Path,
        bytes: &[u8],
        start_offset: u64,
        raw_hash: u64,
    ) -> Result<Vec<SessionBatch>> {
        // offset 0 → full parse (the first record carries `sessionId`).
        // offset > 0 → incremental tail: `bytes` is the slice from `start_offset`
        // to EOF. The slice has no first-record `sessionId`, so the session is
        // derived from the file stem and every line's local offset is shifted to
        // an ABSOLUTE position by `start_offset` (matching the full-parse ids).
        let batch = if start_offset == 0 {
            parse_file(path, bytes, raw_hash)?
        } else {
            let external_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| stable_id(&[&path.to_string_lossy()]));
            parse_slice(path, bytes, start_offset, raw_hash, Some(external_id))?
        };
        Ok(vec![batch])
    }

    fn roots(&self) -> Vec<std::path::PathBuf> {
        vec![self.root.clone()]
    }
}

/// RADAR (Task 2): the sidecar metadata Claude writes next to each subagent
/// transcript at `<session>/subagents/agent-<id>.meta.json`. `agent_id` is not in
/// the file — it is derived from the `agent-<id>` filename stem so the hierarchy
/// pass (Task 3) can key children by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentMeta {
    pub agent_type: String,
    pub description: String,
    pub tool_use_id: String,
    pub agent_id: String,
}

/// Read and parse a Claude subagent sidecar `agent-<id>.meta.json`. The three
/// payload fields (`agentType`, `description`, `toolUseId`) are read leniently
/// (missing → empty string, never an error), while `agent_id` comes from the
/// `agent-<id>` filename stem. Returns an error only when the file cannot be read
/// or is not valid JSON.
pub fn read_subagent_meta(path: &Path) -> Result<SubagentMeta> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parse meta {}", path.display()))?;
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(SubagentMeta {
        agent_type: s("agentType"),
        description: s("description"),
        tool_use_id: s("toolUseId"),
        agent_id: subagent_agent_id(path),
    })
}

/// Derive the subagent id from an `agent-<id>.{jsonl,meta.json}` path: strip a
/// leading `agent-` from the file stem (and a trailing `.meta` for the sidecar).
pub fn subagent_agent_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    // `agent-abc.meta.json` → file_stem is `agent-abc.meta`; drop the `.meta`.
    let stem = stem.strip_suffix(".meta").unwrap_or(&stem);
    stem.strip_prefix("agent-").unwrap_or(stem).to_string()
}

/// Collect the subagent sidecar metas for the detected transcripts. A subagent
/// transcript lives at `<session>/subagents/agent-<id>.jsonl` with a sibling
/// `agent-<id>.meta.json`; for every such transcript we read its meta. Paths that
/// are not subagent transcripts, or whose meta is missing/malformed, are skipped
/// (no error — schema drift never aborts ingest).
pub fn collect_subagent_metas(paths: &[PathBuf]) -> Vec<SubagentMeta> {
    let mut metas = Vec::new();
    for p in paths {
        let in_subagents = p
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n == "subagents")
            .unwrap_or(false);
        let is_agent_file = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("agent-"))
            .unwrap_or(false);
        if !in_subagents || !is_agent_file {
            continue;
        }
        let meta_path = p.with_extension("meta.json");
        if let Ok(meta) = read_subagent_meta(&meta_path) {
            metas.push(meta);
        }
    }
    metas
}

pub fn ingest_all(
    store: &Store,
    root: Option<PathBuf>,
    max_files: Option<usize>,
) -> Result<(usize, usize)> {
    let mut a =
        ClaudeCodeAdapter::with_root(root.unwrap_or_else(default_claude_projects), store.clone());
    a.max_files = max_files;
    // Subagent metas are read from the same detected file set so the Task-3
    // linkage pass can pair children (subagents/agent-<id>.jsonl) to the parent
    // whose Agent/Task tool-call id == meta.toolUseId.
    let detected = a.detect().unwrap_or_default();
    let metas = collect_subagent_metas(&detected);
    let mut batches = a.backfill()?;

    // RADAR (Task 3): resolve parent↔child links from the freshly parsed batches,
    // patch the in-memory `SubagentSpawn.child_session` so it is no longer always
    // `None`, then persist. Linkage is recorded after the rows exist.
    let pairs = crate::radar::hierarchy::link_claude_subagents(&batches, &metas);
    if !pairs.is_empty() {
        use std::collections::HashMap;
        // parent session id → its single linked child (one Agent dispatch → one
        // subagent transcript). Enough to fill the spawn event's child_session.
        let child_by_parent: HashMap<&str, &str> = pairs
            .iter()
            .map(|(c, p)| (p.as_str(), c.as_str()))
            .collect();
        for b in &mut batches {
            if let Some(child) = child_by_parent.get(b.session.id.as_str()) {
                for e in &mut b.events {
                    if let Event::SubagentSpawn { child_session, .. } = &mut e.event {
                        if child_session.is_none() {
                            *child_session = Some((*child).to_string());
                        }
                    }
                }
            }
        }
    }

    // child session id → its sidecar meta (description/agentType), so the linkage
    // pass can thread the subagent's identity onto its row for the RADAR forest.
    // A subagent batch's `agent_id` (from its `subagents/agent-<id>.jsonl` path)
    // joins to the meta of the same `agent_id`.
    let mut child_sid_to_meta: HashMap<&str, &SubagentMeta> = HashMap::new();
    {
        let mut agent_to_child: HashMap<String, &str> = HashMap::new();
        for b in &batches {
            let is_subagent = b
                .session
                .source_path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n == "subagents")
                .unwrap_or(false);
            if is_subagent {
                agent_to_child
                    .insert(subagent_agent_id(&b.session.source_path), b.session.id.as_str());
            }
        }
        for m in &metas {
            if let Some(child_sid) = agent_to_child.get(&m.agent_id) {
                child_sid_to_meta.insert(*child_sid, m);
            }
        }
    }

    let mut sessions = 0;
    let mut events = 0;
    for b in &batches {
        events += b.events.len();
        store.upsert_session_batch(&b.session, &b.turns, &b.events, b.offset)?;
        sessions += 1;
    }
    // Persist linkage now that both parent and child rows exist, and thread the
    // subagent's description/agentType onto the child row (label/role for RADAR).
    for (child, parent) in &pairs {
        store.link_child_session(child, parent)?;
        if let Some(meta) = child_sid_to_meta.get(child.as_str()) {
            store.merge_session_meta(
                child,
                &json!({ "description": meta.description, "agentType": meta.agent_type }),
            )?;
        }
    }
    Ok((sessions, events))
}

/// RADAR (Finding 2): re-derive Claude subagent parent→child links over every
/// persisted session and record them via `Store::link_child_session`. Derived
/// purely from store state — each parent's `SubagentSpawn.child_session` (filled
/// during ingest) names its child — so it is safe to run idempotently on each radar
/// recompute (re-recording the same parent is a plain UPDATE). Returns the number
/// of links (re)recorded. A spawn whose `child_session` is still `None` (child not
/// yet ingested) is skipped — children are never fabricated.
pub fn link_claude_subagents_in_store(store: &Store) -> Result<usize> {
    let sessions = store.sessions()?;
    let mut recorded = 0;
    for s in &sessions {
        if !matches!(s.harness, Harness::ClaudeCode) {
            continue;
        }
        let events = store.session_events(&s.id).unwrap_or_default();
        for (_, e) in events {
            if let Event::SubagentSpawn {
                child_session: Some(child),
                ..
            } = e.event
            {
                store.link_child_session(&child, &s.id)?;
                recorded += 1;
            }
        }
    }
    Ok(recorded)
}

/// Full (offset-0) parse: the first record carries `sessionId`, which resolves
/// the external id (falling back to the file stem).
fn parse_file(path: &Path, bytes: &[u8], raw_hash: u64) -> Result<SessionBatch> {
    parse_slice(path, bytes, 0, raw_hash, None)
}

/// Core parser shared by the full parse and the incremental tail.
///
/// * `base_offset` is added to every line's local byte position so `RawRef.offset`
///   and the returned watermark are ABSOLUTE positions in the on-disk file (0 for
///   a full parse; the saved watermark for a tail, where `bytes` is the slice from
///   that offset to EOF).
/// * `external_id_override` short-circuits session-id resolution. A tail slice has
///   no first-record `sessionId`, so the caller passes the file-stem id. Because a
///   Claude transcript filename IS its session UUID, this yields the SAME
///   `stable_id` as the full parse — so tail events attach to the backfilled row.
fn parse_slice(
    path: &Path,
    bytes: &[u8],
    base_offset: u64,
    raw_hash: u64,
    external_id_override: Option<String>,
) -> Result<SessionBatch> {
    let mut offset = base_offset;
    let mut line_no = 0u32;
    let mut raw_records: Vec<(u64, u32, Value)> = Vec::new();
    // Parse directly from the bytes we already read (avoids a redundant file re-read).
    // `offset` tracks the ABSOLUTE byte position of each line's start (= base_offset
    // + bytes seen so far), preserving RawRef semantics across incremental tails.
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
    let first = raw_records
        .first()
        .map(|(_, _, v)| v)
        .context("empty jsonl")?;
    let external_id = external_id_override.unwrap_or_else(|| {
        first
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| stable_id(&[&path.to_string_lossy()]))
    });
    let sid = stable_id(&["claude_code", &external_id, &path.to_string_lossy()]);
    let mut turns = Vec::new();
    let mut events = Vec::new();
    let mut models = BTreeSet::new();
    let mut started = None;
    let mut ended = None;
    let mut project = None;
    let mut idx = 0u32;
    let mut uuid_to_turn = HashMap::<String, String>::new();
    let mut duration_by_parent = HashMap::<String, u64>::new();
    let mut meta = json!({"ignored_record_types":{}});
    for (off, ln, v) in &raw_records {
        let ts = parse_ts(v.get("timestamp"));
        if started.map(|s| ts < s).unwrap_or(true) {
            started = Some(ts)
        };
        if ended.map(|e| ts > e).unwrap_or(true) {
            ended = Some(ts)
        };
        if project.is_none() {
            if let Some(cwd) = v.get("cwd").and_then(Value::as_str) {
                let cwdp = PathBuf::from(cwd);
                project = Some(ProjectRef {
                    cwd: cwdp.clone(),
                    repo_root: repo_root(&cwdp),
                    git_branch: v
                        .get("gitBranch")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
        match v.get("type").and_then(Value::as_str).unwrap_or("unknown") {
            "user" | "assistant" => {
                idx += 1;
                let uuid = v
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| stable_id(&[&sid, &idx.to_string(), &ln.to_string()]));
                let tid = stable_id(&[&sid, &uuid]);
                uuid_to_turn.insert(uuid.clone(), tid.clone());
                let role = if v.get("type").and_then(Value::as_str) == Some("user") {
                    Role::User
                } else {
                    Role::Assistant
                };
                let parent = v.get("parentUuid").and_then(Value::as_str).and_then(|p| {
                    uuid_to_turn
                        .get(p)
                        .cloned()
                        .or_else(|| Some(stable_id(&[&sid, p])))
                });
                let dur = duration_by_parent.remove(&uuid);
                turns.push(Turn {
                    id: tid.clone(),
                    session_id: sid.clone(),
                    parent_id: parent,
                    role: role.clone(),
                    index: idx,
                    started_at: ts,
                    duration_ms: dur,
                    is_sidechain: v
                        .get("isSidechain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
                let raw = RawRef {
                    source_path: path.to_path_buf(),
                    offset: *off,
                    line: *ln,
                };
                if role == Role::User {
                    map_user(&mut events, &sid, &tid, ts, raw, v);
                } else {
                    map_assistant(&mut events, &sid, &tid, ts, raw.clone(), v, &mut models);
                    if let Some(src) = v
                        .get("sourceToolAssistantUuid")
                        .or_else(|| v.get("sourceToolAssistantUUID"))
                        .and_then(Value::as_str)
                    {
                        events.push(EventRecord {
                            id: stable_id(&[&tid, "spawn", src]),
                            turn_id: tid.clone(),
                            session_id: sid.clone(),
                            ts,
                            event: Event::SubagentSpawn {
                                source_assistant_uuid: src.to_string(),
                                child_session: None,
                            },
                            raw_ref: raw,
                        });
                    }
                }
            }
            "system" => {
                if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") {
                    if let (Some(parent), Some(d)) = (
                        v.get("parentUuid").and_then(Value::as_str),
                        v.get("durationMs").and_then(Value::as_u64),
                    ) {
                        if let Some(tid) = uuid_to_turn.get(parent).cloned() {
                            if let Some(turn) = turns.iter_mut().find(|t| t.id == tid) {
                                turn.duration_ms = Some(d);
                            } else {
                                duration_by_parent.insert(parent.to_string(), d);
                            }
                        } else {
                            duration_by_parent.insert(parent.to_string(), d);
                        }
                    }
                }
                idx += 1;
                let tid = stable_id(&[&sid, "system", &ln.to_string()]);
                turns.push(Turn {
                    id: tid.clone(),
                    session_id: sid.clone(),
                    parent_id: None,
                    role: Role::System,
                    index: idx,
                    started_at: ts,
                    duration_ms: None,
                    is_sidechain: v
                        .get("isSidechain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
                events.push(EventRecord {
                    id: stable_id(&[&tid, "notice"]),
                    turn_id: tid,
                    session_id: sid.clone(),
                    ts,
                    event: Event::SystemNotice {
                        subtype: v
                            .get("subtype")
                            .and_then(Value::as_str)
                            .unwrap_or("system")
                            .to_string(),
                        data: v.clone(),
                    },
                    raw_ref: RawRef {
                        source_path: path.to_path_buf(),
                        offset: *off,
                        line: *ln,
                    },
                });
            }
            "file-history-snapshot" => {
                idx += 1;
                let tid = stable_id(&[&sid, "snapshot", &ln.to_string()]);
                turns.push(Turn {
                    id: tid.clone(),
                    session_id: sid.clone(),
                    parent_id: None,
                    role: Role::System,
                    index: idx,
                    started_at: ts,
                    duration_ms: None,
                    is_sidechain: false,
                });
                events.push(EventRecord {
                    id: stable_id(&[&tid, "files"]),
                    turn_id: tid,
                    session_id: sid.clone(),
                    ts,
                    event: Event::FileSnapshot {
                        files: parse_snapshot(v.get("snapshot")),
                    },
                    raw_ref: RawRef {
                        source_path: path.to_path_buf(),
                        offset: *off,
                        line: *ln,
                    },
                });
            }
            "mode" | "permission-mode" => {
                idx += 1;
                let tid = stable_id(&[&sid, "mode", &ln.to_string()]);
                turns.push(Turn {
                    id: tid.clone(),
                    session_id: sid.clone(),
                    parent_id: None,
                    role: Role::System,
                    index: idx,
                    started_at: ts,
                    duration_ms: None,
                    is_sidechain: false,
                });
                let mode = v
                    .get("mode")
                    .or_else(|| v.get("permissionMode"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                events.push(EventRecord {
                    id: stable_id(&[&tid, "mode"]),
                    turn_id: tid,
                    session_id: sid.clone(),
                    ts,
                    event: Event::ModeChange { mode },
                    raw_ref: RawRef {
                        source_path: path.to_path_buf(),
                        offset: *off,
                        line: *ln,
                    },
                });
            }
            other => {
                let obj = meta
                    .get_mut("ignored_record_types")
                    .unwrap()
                    .as_object_mut()
                    .unwrap();
                let n = obj.get(other).and_then(Value::as_u64).unwrap_or(0) + 1;
                obj.insert(other.to_string(), json!(n));
            }
        }
    }
    Ok(SessionBatch {
        session: Session {
            id: sid,
            harness: Harness::ClaudeCode,
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
fn map_user(
    events: &mut Vec<EventRecord>,
    sid: &str,
    tid: &str,
    ts: chrono::DateTime<Utc>,
    raw: RawRef,
    v: &Value,
) {
    let msg = &v["message"];
    let content = &msg["content"];
    let is_meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
    if let Some(s) = content.as_str() {
        events.push(EventRecord {
            id: stable_id(&[tid, "prompt"]),
            turn_id: tid.to_string(),
            session_id: sid.to_string(),
            ts,
            event: Event::UserPrompt {
                text: s.to_string(),
                attachments: vec![],
                is_meta,
            },
            raw_ref: raw,
        });
    } else if let Some(arr) = content.as_array() {
        let mut prompt_parts = Vec::new();
        for (i, b) in arr.iter().enumerate() {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let text = block_text(b.get("content"));
                    let status = if b.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Ok
                    };
                    events.push(EventRecord {
                        id: stable_id(&[tid, "tool_result", &i.to_string()]),
                        turn_id: tid.to_string(),
                        session_id: sid.to_string(),
                        ts,
                        event: Event::ToolResult {
                            call_id: b
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            status,
                            bytes: text.len() as u64,
                            summary: Some(truncate_chars(&text, 500)),
                        },
                        raw_ref: raw.clone(),
                    });
                }
                Some("text") => prompt_parts.push(block_text(b.get("text"))),
                _ => {}
            }
        }
        if !prompt_parts.is_empty() {
            events.push(EventRecord {
                id: stable_id(&[tid, "prompt"]),
                turn_id: tid.to_string(),
                session_id: sid.to_string(),
                ts,
                event: Event::UserPrompt {
                    text: prompt_parts.join("\n"),
                    attachments: vec![],
                    is_meta,
                },
                raw_ref: raw,
            });
        }
    }
}
fn map_assistant(
    events: &mut Vec<EventRecord>,
    sid: &str,
    tid: &str,
    ts: chrono::DateTime<Utc>,
    raw: RawRef,
    v: &Value,
    models: &mut BTreeSet<String>,
) {
    let msg = &v["message"];
    if let Some(m) = msg.get("model").and_then(Value::as_str) {
        models.insert(m.to_string());
    }
    if let Some(text) = msg.get("content").and_then(Value::as_str) {
        events.push(EventRecord {
            id: stable_id(&[tid, "text"]),
            turn_id: tid.to_string(),
            session_id: sid.to_string(),
            ts,
            event: Event::AssistantText {
                text: text.to_string(),
            },
            raw_ref: raw.clone(),
        });
    } else if let Some(arr) = msg.get("content").and_then(Value::as_array) {
        for (i, b) in arr.iter().enumerate() {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => events.push(EventRecord {
                    id: stable_id(&[tid, "text", &i.to_string()]),
                    turn_id: tid.to_string(),
                    session_id: sid.to_string(),
                    ts,
                    event: Event::AssistantText {
                        text: block_text(b.get("text")),
                    },
                    raw_ref: raw.clone(),
                }),
                Some("thinking") => {
                    let text = block_text(b.get("thinking").or_else(|| b.get("text")));
                    events.push(EventRecord {
                        id: stable_id(&[tid, "thinking", &i.to_string()]),
                        turn_id: tid.to_string(),
                        session_id: sid.to_string(),
                        ts,
                        event: Event::Thinking {
                            tokens: (text.len() / 4) as u32,
                        },
                        raw_ref: raw.clone(),
                    });
                }
                Some("tool_use") | Some("server_tool_use") => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("unknown");
                    let kind = if name == "Task" {
                        ToolKind::SubagentTask
                    } else if b.get("type").and_then(Value::as_str) == Some("server_tool_use") {
                        ToolKind::Mcp
                    } else {
                        ToolKind::Builtin
                    };
                    events.push(EventRecord {
                        id: stable_id(&[tid, "tool", &i.to_string()]),
                        turn_id: tid.to_string(),
                        session_id: sid.to_string(),
                        ts,
                        event: Event::ToolCall {
                            tool: name.to_string(),
                            input: b.get("input").cloned().unwrap_or(Value::Null),
                            call_id: b
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            kind,
                        },
                        raw_ref: raw.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    if let Some(u) = msg.get("usage") {
        let model = msg
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        events.push(EventRecord {
            id: stable_id(&[tid, "usage"]),
            turn_id: tid.to_string(),
            session_id: sid.to_string(),
            ts,
            event: Event::TokenUsage {
                input: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
                output: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
                cache_creation: u
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .or_else(|| u.get("cache_creation").and_then(Value::as_u64))
                    .unwrap_or(0) as u32,
                cache_read: u
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                model,
                orchestration: None,
            },
            raw_ref: raw,
        });
    }
}
fn block_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| block_text(Some(x)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(o)) => o
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| Value::Object(o.clone()).to_string()),
        Some(x) => x.to_string(),
        None => String::new(),
    }
}
fn parse_snapshot(v: Option<&Value>) -> Vec<FileEdit> {
    match v {
        Some(Value::Object(o)) => o
            .keys()
            .take(500)
            .map(|k| FileEdit {
                path: k.clone(),
                old_hash: None,
                new_hash: None,
                lines_changed: None,
            })
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| {
                x.get("path").and_then(Value::as_str).map(|p| FileEdit {
                    path: p.to_string(),
                    old_hash: None,
                    new_hash: None,
                    lines_changed: None,
                })
            })
            .collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::tempdir;
    #[test]
    fn parses_minimal_claude_jsonl() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, r#"{"type":"user","uuid":"u1","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"fix tests"},"cwd":"/tmp","gitBranch":"main"}
{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","model":"claude","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}},{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":2}}}
"#).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();
        assert_eq!(b.turns.len(), 2);
        assert!(b
            .events
            .iter()
            .any(|e| matches!(e.event,Event::ToolCall{ref tool,..} if tool=="Bash")));
        assert_eq!(b.session.model_ids, vec!["claude".to_string()]);
    }
    #[test]
    fn attaches_turn_duration_even_when_notice_arrives_after_turn() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, r#"{"type":"user","uuid":"u1","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"fix tests"}}
{"type":"system","subtype":"turn_duration","parentUuid":"u1","durationMs":1234,"timestamp":"2026-01-01T00:00:02Z"}
"#).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();

        let user_turn = b.turns.iter().find(|t| t.role == Role::User).unwrap();
        assert_eq!(user_turn.duration_ms, Some(1234));
    }

    #[test]
    fn parses_assistant_string_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, r#"{"type":"assistant","uuid":"a1","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","model":"claude","content":"plain answer"}}
"#).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_file(&p, &bytes, hash64(&bytes)).unwrap();

        assert!(b
            .events
            .iter()
            .any(|e| matches!(&e.event, Event::AssistantText { text } if text == "plain answer")));
    }

    /// Incremental tail parse (Claude): appending one `assistant` line and
    /// parsing only the appended slice at the original EOF offset yields only the
    /// appended events, each with an ABSOLUTE `RawRef.offset` ≥ the original EOF.
    #[test]
    fn parse_range_incremental_offset_yields_only_appended_events() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let original = b"{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n";
        std::fs::write(&p, original as &[u8]).unwrap();
        let original_eof = original.len() as u64;

        // Append one assistant line carrying a single AssistantText.
        let appended = b"{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":\"appended answer\"}}\n";
        let mut full = original.to_vec();
        full.extend_from_slice(appended);

        let store = Store::memory().unwrap();
        let adapter = ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store);
        let slice = &full[original_eof as usize..];
        let batches = adapter
            .parse_range(&p, slice, original_eof, hash64(&full))
            .expect("tail parse_range ok");
        assert_eq!(batches.len(), 1);
        let b = &batches[0];

        // Only the appended AssistantText event is present (the original user line is not in the slice).
        assert!(
            b.events
                .iter()
                .any(|e| matches!(&e.event, Event::AssistantText { text } if text == "appended answer")),
            "appended AssistantText must be parsed"
        );
        assert!(
            !b.events.iter().any(|e| matches!(e.event, Event::UserPrompt { .. })),
            "the original user prompt is outside the slice and must not reappear"
        );

        // Every appended event's offset is ABSOLUTE (≥ original EOF).
        for e in &b.events {
            assert!(
                e.raw_ref.offset >= original_eof,
                "RawRef.offset must be absolute (start_offset + local); got {} < {}",
                e.raw_ref.offset,
                original_eof
            );
            assert_eq!(
                full[e.raw_ref.offset as usize], b'{',
                "absolute offset must point at the appended line start"
            );
        }
        // Watermark advances to the new EOF.
        assert_eq!(b.offset, full.len() as u64, "watermark advances to new EOF");
    }

    /// Task 2 (RADAR): a subagent transcript under `<session>/subagents/` is
    /// discoverable by `detect()` (WalkDir recurses), and its sidecar
    /// `agent-<id>.meta.json` parses into the three fields plus an `agent_id`
    /// derived from the `agent-<id>` filename stem.
    #[test]
    fn detect_includes_subagents_and_meta_parses() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj").join("session-1");
        let subs = proj.join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        // The subagent transcript + its sidecar meta.
        let jsonl = subs.join("agent-abc.jsonl");
        std::fs::write(
            &jsonl,
            "{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"sub\",\"isSidechain\":true,\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"go\"}}\n",
        )
        .unwrap();
        let meta_path = subs.join("agent-abc.meta.json");
        std::fs::write(
            &meta_path,
            r#"{"agentType":"Explore","description":"map frontend","toolUseId":"toolu_01"}"#,
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let adapter = ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store);
        let detected = adapter.detect().unwrap();
        assert!(
            detected.iter().any(|p| p == &jsonl),
            "detect() must include the subagent transcript under subagents/; got {detected:?}"
        );

        let meta = read_subagent_meta(&meta_path).expect("meta parses");
        assert_eq!(meta.agent_type, "Explore");
        assert_eq!(meta.description, "map frontend");
        assert_eq!(meta.tool_use_id, "toolu_01");
        assert_eq!(meta.agent_id, "abc", "agent_id derived from agent-<id> stem");
    }

    /// Task 3 end-to-end: a parent transcript that issues a `Task` tool-call
    /// (id `toolu_xyz`) plus a child subagent transcript at
    /// `subagents/agent-<id>.jsonl` with a matching meta → after `ingest_all`,
    /// the store records the child's parent and the spawn event carries the child.
    #[test]
    fn ingest_all_links_claude_subagent_to_parent() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        let session = proj.join("019sess");
        let subs = session.join("subagents");
        std::fs::create_dir_all(&subs).unwrap();

        // Parent transcript: an assistant turn with a Task tool_use + a sidechain
        // spawn pointer so a SubagentSpawn event is emitted.
        let parent_jsonl = session.join("019sess.jsonl");
        std::fs::write(&parent_jsonl, "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"019sess\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"sourceToolAssistantUuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_xyz\",\"name\":\"Task\",\"input\":{}}]}}\n").unwrap();

        // Child subagent transcript + sidecar meta keyed by the same toolUseId.
        let child_jsonl = subs.join("agent-deadbeef.jsonl");
        std::fs::write(&child_jsonl, "{\"type\":\"user\",\"uuid\":\"cu\",\"sessionId\":\"019sess-sub\",\"isSidechain\":true,\"agentId\":\"deadbeef\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"content\":\"work\"}}\n").unwrap();
        std::fs::write(
            subs.join("agent-deadbeef.meta.json"),
            r#"{"agentType":"Explore","description":"sweep the radar module","toolUseId":"toolu_xyz"}"#,
        )
        .unwrap();

        let store = Store::memory().unwrap();
        ingest_all(&store, Some(proj.clone()), None).unwrap();

        // Resolve the two session ids the parser assigned (stable_id over path).
        let parent_sid = stable_id(&[
            "claude_code",
            "019sess",
            &parent_jsonl.to_string_lossy(),
        ]);
        let child_sid = stable_id(&[
            "claude_code",
            "019sess-sub",
            &child_jsonl.to_string_lossy(),
        ]);

        assert_eq!(
            store.parent_of(&child_sid).unwrap(),
            Some(parent_sid.clone()),
            "child session's parent must be persisted"
        );

        // Finding 1: the subagent's description/agentType are threaded onto the
        // child row's meta when the link is persisted (label/role for RADAR).
        let child_meta = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == child_sid)
            .expect("child session row present")
            .meta;
        assert_eq!(
            child_meta.get("description").and_then(|v| v.as_str()),
            Some("sweep the radar module"),
            "child meta carries the sidecar description"
        );
        assert_eq!(
            child_meta.get("agentType").and_then(|v| v.as_str()),
            Some("Explore"),
            "child meta carries the sidecar agentType"
        );

        // The parent's SubagentSpawn event now carries the child session id.
        let spawn_child = store
            .session_events(&parent_sid)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match e.event {
                Event::SubagentSpawn { child_session, .. } => Some(child_session),
                _ => None,
            })
            .expect("a SubagentSpawn event must exist on the parent");
        assert_eq!(
            spawn_child,
            Some(child_sid),
            "SubagentSpawn.child_session must be patched (no longer None)"
        );
    }

    #[test]
    fn ingest_is_idempotent_by_hash() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("projects");
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("s.jsonl");
        std::fs::write(&p,"{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hello\"}}\n").unwrap();
        let store = Store::memory().unwrap();
        assert_eq!(ingest_all(&store, Some(root.clone()), None).unwrap().0, 1);
        assert_eq!(ingest_all(&store, Some(root), None).unwrap().0, 0);
    }
}
