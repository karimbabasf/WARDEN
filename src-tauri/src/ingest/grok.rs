//! Grok CLI adapter.
//!
//! Layout: `~/.grok/sessions/<percent-encoded cwd>/<session uuid>/`, holding
//! `chat_history.jsonl` (the conversation) and `events.jsonl` (a timestamped
//! lifecycle stream).
//!
//! **`events.jsonl` is the source, not `chat_history.jsonl`,** and the reason is
//! not a preference. `chat_history.jsonl` carries NO TIMESTAMP on any record: it
//! is a replayable message array, so every `EventRecord` built from it would need
//! a time WARDEN invented. `events.jsonl` timestamps everything to the
//! millisecond and additionally states what `chat_history` cannot:
//!
//! * `turn_started` / `turn_ended` bound a turn explicitly, so this harness needs
//!   none of the `stop_reason` inference that Claude and Codex do;
//! * `tool_started` / `tool_completed` carry an outcome and a duration;
//! * `permission_requested` / `permission_resolved` say, directly, that the agent
//!   is STOPPED ON THE OPERATOR. That is WARDEN's third globe state arriving as a
//!   fact rather than as a heuristic, which neither built adapter gets.
//!
//! The cost of the choice is message TEXT and token counts, which live only in
//! `chat_history.jsonl`. A Grok globe therefore renders its liveness, its current
//! action and its awaiting state honestly, and reports no context size. Showing a
//! guessed size would be the dishonest half of that trade, so it shows none.
//!
//! THE WORKING DIRECTORY IS IN THE PATH, not in any record. That matters beyond
//! naming: `radar::procs` matches a session to a live process by directory, so a
//! Grok session whose cwd failed to decode could never be resolved against the
//! process table and would hang on the board after the agent exited.

use super::{Adapter, SessionBatch};
use crate::ir::*;
use crate::store::Store;
use crate::util::{default_grok_sessions, hash64};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct GrokAdapter {
    pub root: PathBuf,
    pub store: Store,
    pub max_files: Option<usize>,
}

impl GrokAdapter {
    pub fn new(store: Store) -> Self {
        Self {
            root: default_grok_sessions(),
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

impl Adapter for GrokAdapter {
    fn harness(&self) -> Harness {
        Harness::Grok
    }

    fn detect(&self) -> Result<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = Vec::new();
        if !self.root.exists() {
            return Ok(paths);
        }
        // Exactly two levels: <encoded cwd>/<uuid>/events.jsonl. Walking with a
        // fixed depth rather than recursively is deliberate, because the session
        // directory also holds `terminal/` and `recap_requests/` subtrees whose
        // contents are not transcripts.
        for cwd_dir in std::fs::read_dir(&self.root)?.flatten() {
            if !cwd_dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(sessions) = std::fs::read_dir(cwd_dir.path()) else {
                continue;
            };
            for session in sessions.flatten() {
                let events = session.path().join("events.jsonl");
                if events.is_file() {
                    paths.push(events);
                }
            }
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
            if self.store.source_raw_hash(&p)?.is_some_and(|h| h == raw_hash) {
                continue;
            }
            match parse_slice(&p, &bytes, 0, raw_hash) {
                Ok(b) => out.push(b),
                Err(e) => {
                    tracing::warn!(path=%p.display(), error=?e, "skipping malformed Grok session")
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
        Ok(vec![parse_slice(path, bytes, start_offset, raw_hash)?])
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.root.clone()]
    }
}

/// Decode a percent-encoded path segment, e.g. `%2FUsers%2Fk` to `/Users/k`.
///
/// Hand-written rather than pulled from a crate: this decodes ONE directory name
/// per session and adding a dependency for it would be the larger cost. Invalid
/// escapes are passed through verbatim, because a directory name WARDEN cannot
/// decode is still the honest identifier for that session, and refusing it would
/// drop the whole session over a naming quirk.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `<root>/<encoded cwd>/<uuid>/events.jsonl` to (session uuid, decoded cwd).
///
/// Both come from the PATH because no record in `events.jsonl` carries the
/// directory, and only some carry the session id.
pub fn identity_from_path(path: &Path) -> (String, Option<PathBuf>) {
    let session_dir = path.parent();
    let external_id = session_dir
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let cwd = session_dir
        .and_then(|d| d.parent())
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .map(|enc| PathBuf::from(percent_decode(enc)));
    (external_id, cwd)
}

/// Grok's `tool_started` and `tool_completed` carry a TOOL NAME and no call id,
/// so a call and its result are joined by name in arrival order.
///
/// FIFO per name, not a global queue: Grok runs tools concurrently (a real
/// session here starts `read_file` and completes `list_dir` half a second later),
/// so a single queue would pair each result with whatever happened to be oldest
/// and mislabel every overlapping call. Per name, a result can only ever close a
/// call of its own kind.
#[derive(Default)]
struct CallJoin {
    pending: HashMap<String, Vec<String>>,
    next: u64,
}

impl CallJoin {
    fn start(&mut self, tool: &str) -> String {
        self.next += 1;
        let id = format!("grok-call-{}", self.next);
        self.pending
            .entry(tool.to_string())
            .or_default()
            .push(id.clone());
        id
    }
    /// The id of the oldest unfinished call of this tool, if any. A completion
    /// with no start (the file was tailed from an offset that split the pair)
    /// yields `None` and is dropped rather than inventing a call.
    fn finish(&mut self, tool: &str) -> Option<String> {
        let q = self.pending.get_mut(tool)?;
        if q.is_empty() {
            return None;
        }
        Some(q.remove(0))
    }
}

fn ts_of(v: &Value) -> Option<DateTime<Utc>> {
    v.get("ts")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Grok phases that say nothing a globe can render.
///
/// `phase_changed` is 1712 of the 1906 records in a real session here, roughly 90%
/// of the file. Storing it would multiply the event table for no signal: the same
/// state is already carried by the tool and turn records around it.
fn is_noise(kind: &str) -> bool {
    matches!(kind, "phase_changed" | "first_token" | "loop_started")
}

/// Parse a slice of `events.jsonl` into one [`SessionBatch`].
///
/// `start_offset` shifts every line's recorded position to an ABSOLUTE one, so an
/// incremental tail and a full read produce identical `raw_ref`s. Follows the same
/// byte-watermark contract as the Claude and Codex adapters.
pub fn parse_slice(
    path: &Path,
    bytes: &[u8],
    start_offset: u64,
    raw_hash: u64,
) -> Result<SessionBatch> {
    let (external_id, cwd) = identity_from_path(path);
    let text = String::from_utf8_lossy(bytes);

    let mut turns: Vec<Turn> = Vec::new();
    let mut events: Vec<EventRecord> = Vec::new();
    let mut join = CallJoin::default();
    let mut models: Vec<String> = Vec::new();
    let mut turn_index: u32 = 0;
    let mut current_turn: Option<String> = None;
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut offset = start_offset;
    let mut line_no: u32 = 0;

    let session_id = format!("grok:{external_id}");

    for line in text.split_inclusive('\n') {
        let this_offset = offset;
        offset += line.len() as u64;
        line_no += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue; // schema drift never drops a session
        };
        let Some(kind) = v.get("type").and_then(Value::as_str) else {
            continue;
        };
        if is_noise(kind) {
            continue;
        }
        let Some(ts) = ts_of(&v) else {
            continue; // a record with no time cannot be placed on a timeline
        };
        if first_ts.is_none() {
            first_ts = Some(ts);
        }

        // Every event needs a turn to hang off. A slice that begins mid-session
        // has no `turn_started` in it, so one is synthesized rather than dropping
        // the tail, which is the whole point of an incremental read.
        if current_turn.is_none() {
            let id = format!("{session_id}:t{turn_index}");
            turns.push(Turn {
                id: id.clone(),
                session_id: session_id.clone(),
                parent_id: None,
                role: Role::User,
                index: turn_index,
                started_at: ts,
                duration_ms: None,
                is_sidechain: false,
            });
            current_turn = Some(id);
        }
        let turn_id = current_turn.clone().unwrap_or_default();

        let event = match kind {
            "turn_started" => {
                if let Some(m) = v.get("model_id").and_then(Value::as_str) {
                    if !models.iter().any(|x| x == m) {
                        models.push(m.to_string());
                    }
                }
                Event::UserPrompt {
                    // The prompt TEXT lives in chat_history.jsonl, which this
                    // adapter does not read. An empty string is the honest value:
                    // the turn happened, its wording is not in this file.
                    text: String::new(),
                    attachments: vec![],
                    is_meta: false,
                }
            }
            "turn_ended" => {
                let outcome = v
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                // Close the turn so the next event opens a fresh one.
                current_turn = None;
                turn_index += 1;
                // `turn_complete: Some(true)` is a FACT here, not the inference
                // Claude and Codex need: Grok states the boundary outright.
                Event::AssistantText {
                    text: String::new(),
                    turn_complete: Some(outcome == "completed"),
                }
            }
            "tool_started" => {
                let tool = v
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Event::ToolCall {
                    tool: tool.to_string(),
                    // Grok logs no arguments in this stream. `null` rather than an
                    // empty object, so a reader can tell "not recorded" from
                    // "called with nothing".
                    input: Value::Null,
                    call_id: join.start(tool),
                    kind: ToolKind::Unknown,
                }
            }
            "tool_completed" => {
                let tool = v
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let Some(call_id) = join.finish(tool) else {
                    continue;
                };
                let ok = v.get("outcome").and_then(Value::as_str) == Some("success");
                Event::ToolResult {
                    call_id,
                    status: if ok { ToolStatus::Ok } else { ToolStatus::Error },
                    bytes: 0,
                    summary: v
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .map(|d| format!("{tool} in {d}ms")),
                }
            }
            // THE AWAITING SIGNAL. Grok states outright that it is blocked on a
            // human, which no other harness here does: Claude's registry has to be
            // read for it and Codex has no record of it at all. Carried as a
            // notice pair so `radar::awaiting` can resolve it without this adapter
            // reaching into the radar layer.
            "permission_requested" | "permission_resolved" => Event::SystemNotice {
                subtype: kind.to_string(),
                data: json!({
                    "tool_name": v.get("tool_name").and_then(Value::as_str).unwrap_or_default(),
                    "decision": v.get("decision").and_then(Value::as_str),
                }),
            },
            other => Event::SystemNotice {
                subtype: other.to_string(),
                data: v.clone(),
            },
        };

        events.push(EventRecord {
            id: format!("{session_id}:e{}", events.len()),
            turn_id,
            session_id: session_id.clone(),
            ts,
            event,
            raw_ref: RawRef {
                source_path: path.to_path_buf(),
                offset: this_offset,
                line: line_no,
            },
        });
    }

    let started_at = first_ts.unwrap_or_else(Utc::now);
    let session = Session {
        id: session_id,
        harness: Harness::Grok,
        external_id,
        project: cwd.map(|cwd| ProjectRef {
            cwd,
            repo_root: None,
            git_branch: None,
        }),
        model_ids: models,
        started_at,
        ended_at: None,
        source_path: path.to_path_buf(),
        raw_hash,
        ingested_at: Utc::now(),
        meta: json!({ "originator": "Grok CLI" }),
    };

    Ok(SessionBatch {
        session,
        turns,
        events,
        offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events_path(dir: &Path) -> PathBuf {
        let p = dir
            .join("%2FUsers%2Fk%2FDeveloper%2FWARDEN")
            .join("019f72e8-1c5d-7983-84b8-a16b0a776873");
        std::fs::create_dir_all(&p).unwrap();
        p.join("events.jsonl")
    }

    #[test]
    fn percent_decode_restores_a_path() {
        assert_eq!(percent_decode("%2FUsers%2Fkarimbaba"), "/Users/karimbaba");
        assert_eq!(percent_decode("plain"), "plain");
    }

    /// A name WARDEN cannot decode is still that session's identifier, so it is
    /// passed through instead of failing the whole session.
    #[test]
    fn percent_decode_passes_through_a_broken_escape() {
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("trailing%"), "trailing%");
    }

    /// The cwd is what `radar::procs` matches a live process against, so losing it
    /// means a Grok globe that never closes. It comes from the PATH, since no
    /// record in the file carries it.
    #[test]
    fn identity_comes_from_the_path() {
        let p = Path::new("/r/%2FUsers%2Fk%2Fdev/019f-uuid/events.jsonl");
        let (id, cwd) = identity_from_path(p);
        assert_eq!(id, "019f-uuid");
        assert_eq!(cwd, Some(PathBuf::from("/Users/k/dev")));
    }

    #[test]
    fn parses_a_turn_with_a_tool_call_and_result() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        std::fs::write(
            &p,
            r#"{"ts":"2026-07-18T01:47:22.675Z","type":"turn_started","session_id":"019f72e8-1c5d-7983-84b8-a16b0a776873","turn_number":0,"model_id":"grok-4.5"}
{"ts":"2026-07-18T01:47:22.680Z","type":"phase_changed","phase":"waiting_for_model"}
{"ts":"2026-07-18T01:47:28.158Z","type":"tool_started","tool_name":"read_file"}
{"ts":"2026-07-18T01:47:28.762Z","type":"tool_completed","tool_name":"read_file","duration_ms":2,"outcome":"success"}
{"ts":"2026-07-18T01:47:31.910Z","type":"turn_ended","outcome":"completed"}
"#,
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_slice(&p, &bytes, 0, 7).unwrap();

        assert_eq!(b.session.harness, Harness::Grok);
        assert_eq!(b.session.external_id, "019f72e8-1c5d-7983-84b8-a16b0a776873");
        assert_eq!(
            b.session.project.as_ref().map(|p| p.cwd.clone()),
            Some(PathBuf::from("/Users/k/Developer/WARDEN"))
        );
        assert_eq!(b.session.model_ids, vec!["grok-4.5"]);
        assert_eq!(b.offset, bytes.len() as u64);

        // phase_changed is dropped; the other four survive.
        assert_eq!(b.events.len(), 4);
        let kinds: Vec<&str> = b.events.iter().map(|e| e.event.kind_name()).collect();
        assert_eq!(
            kinds,
            vec!["user_prompt", "tool_call", "tool_result", "assistant_text"]
        );

        // The call and its result share an id, which is what makes the tool read
        // as finished rather than forever in flight.
        let call_id = match &b.events[1].event {
            Event::ToolCall { call_id, tool, .. } => {
                assert_eq!(tool, "read_file");
                call_id.clone()
            }
            other => panic!("expected a tool call, got {other:?}"),
        };
        match &b.events[2].event {
            Event::ToolResult { call_id: c, status, .. } => {
                assert_eq!(c, &call_id);
                assert_eq!(status, &ToolStatus::Ok);
            }
            other => panic!("expected a tool result, got {other:?}"),
        }
        // Grok STATES the turn boundary, so this is a fact and not an inference.
        match &b.events[3].event {
            Event::AssistantText { turn_complete, .. } => {
                assert_eq!(turn_complete, &Some(true))
            }
            other => panic!("expected assistant text, got {other:?}"),
        }
    }

    /// Grok runs tools concurrently, so results do not arrive in start order. A
    /// single FIFO queue would pair `list_dir`'s completion with `read_file`'s
    /// call and mislabel both.
    #[test]
    fn joins_overlapping_tool_calls_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        std::fs::write(
            &p,
            r#"{"ts":"2026-07-18T01:47:22.675Z","type":"turn_started","model_id":"grok-4.5"}
{"ts":"2026-07-18T01:47:28.100Z","type":"tool_started","tool_name":"read_file"}
{"ts":"2026-07-18T01:47:28.150Z","type":"tool_started","tool_name":"list_dir"}
{"ts":"2026-07-18T01:47:28.700Z","type":"tool_completed","tool_name":"list_dir","outcome":"success"}
{"ts":"2026-07-18T01:47:29.000Z","type":"tool_completed","tool_name":"read_file","outcome":"error"}
"#,
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_slice(&p, &bytes, 0, 7).unwrap();

        let read_call = match &b.events[1].event {
            Event::ToolCall { call_id, .. } => call_id.clone(),
            o => panic!("{o:?}"),
        };
        let list_call = match &b.events[2].event {
            Event::ToolCall { call_id, .. } => call_id.clone(),
            o => panic!("{o:?}"),
        };
        // list_dir completes FIRST and must close the list_dir call, not read_file's.
        match &b.events[3].event {
            Event::ToolResult { call_id, status, .. } => {
                assert_eq!(call_id, &list_call);
                assert_eq!(status, &ToolStatus::Ok);
            }
            o => panic!("{o:?}"),
        }
        match &b.events[4].event {
            Event::ToolResult { call_id, status, .. } => {
                assert_eq!(call_id, &read_call);
                assert_eq!(status, &ToolStatus::Error);
            }
            o => panic!("{o:?}"),
        }
    }

    /// The awaiting pair survives ingest, which is the signal no other harness
    /// here reports outright.
    #[test]
    fn keeps_the_permission_pair() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        std::fs::write(
            &p,
            r#"{"ts":"2026-07-18T01:47:28.158Z","type":"permission_requested","tool_name":"run_terminal_cmd"}
{"ts":"2026-07-18T01:47:40.000Z","type":"permission_resolved","tool_name":"run_terminal_cmd","decision":"allow","wait_ms":11842}
"#,
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_slice(&p, &bytes, 0, 7).unwrap();
        let subs: Vec<String> = b
            .events
            .iter()
            .filter_map(|e| match &e.event {
                Event::SystemNotice { subtype, .. } => Some(subtype.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(subs, vec!["permission_requested", "permission_resolved"]);
    }

    /// A tail read from a byte offset must produce ABSOLUTE positions, or a
    /// watermark re-read would double-count. Same contract as the other adapters.
    #[test]
    fn an_incremental_tail_records_absolute_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        let head = "{\"ts\":\"2026-07-18T01:47:22.675Z\",\"type\":\"turn_started\",\"model_id\":\"grok-4.5\"}\n";
        let tail = "{\"ts\":\"2026-07-18T01:47:28.158Z\",\"type\":\"tool_started\",\"tool_name\":\"read_file\"}\n";
        std::fs::write(&p, format!("{head}{tail}")).unwrap();

        let b = parse_slice(&p, tail.as_bytes(), head.len() as u64, 7).unwrap();
        assert_eq!(b.events.len(), 1);
        assert_eq!(b.events[0].raw_ref.offset, head.len() as u64);
        assert_eq!(b.offset, (head.len() + tail.len()) as u64);
    }

    /// A malformed line must never drop the session around it.
    #[test]
    fn schema_drift_skips_the_line_not_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        std::fs::write(
            &p,
            "not json at all\n{\"ts\":\"2026-07-18T01:47:22.675Z\",\"type\":\"turn_started\",\"model_id\":\"grok-4.5\"}\n{\"type\":\"tool_started\",\"tool_name\":\"no_timestamp\"}\n",
        )
        .unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let b = parse_slice(&p, &bytes, 0, 7).unwrap();
        assert_eq!(b.session.model_ids, vec!["grok-4.5"]);
        assert_eq!(b.events.len(), 1, "the untimed record is dropped, not the session");
    }

    #[test]
    fn detect_finds_events_files_two_levels_down() {
        let dir = tempfile::tempdir().unwrap();
        let p = events_path(dir.path());
        std::fs::write(&p, "").unwrap();
        // Noise that must not be picked up as a transcript.
        let noise = p.parent().unwrap().join("terminal");
        std::fs::create_dir_all(&noise).unwrap();
        std::fs::write(noise.join("scrollback.jsonl"), "").unwrap();

        let store = Store::memory().unwrap();
        let a = GrokAdapter::with_root(dir.path().to_path_buf(), store);
        let found = a.detect().unwrap();
        assert_eq!(found, vec![p]);
    }

    #[test]
    fn detect_on_a_missing_root_is_empty() {
        let store = Store::memory().unwrap();
        let a = GrokAdapter::with_root(PathBuf::from("/nope/not/here"), store);
        assert!(a.detect().unwrap().is_empty());
    }
}
