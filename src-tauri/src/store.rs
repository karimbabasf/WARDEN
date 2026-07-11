use crate::ir::*;
use crate::util::ensure_parent;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    pub path: PathBuf,
}

/// RADAR (Fix #3): the cached, pre-calibration raw token sums for one session's
/// estimated composition. These depend ONLY on the session's transcript content
/// (not on the live exact total, which is applied as a deterministic calibration
/// step afterward), so they are safely cacheable by `(session_id, change_key)`.
/// Caching them lets a recompute skip re-tokenizing every unchanged transcript —
/// during an FS storm only the one session being written misses the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadarTokenCounts {
    pub turn1_total: u64,
    pub first_user_tokens: u64,
    pub conversation: u64,
    pub tool_output: u64,
    pub thinking: u64,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        ensure_parent(path.as_ref())?;
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("open sqlite {}", path.as_ref().display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let s = Store {
            conn: Arc::new(Mutex::new(conn)),
            path: path.as_ref().to_path_buf(),
        };
        s.migrate()?;
        Ok(s)
    }
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Store {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        };
        s.migrate()?;
        Ok(s)
    }
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("sqlite mutex poisoned")
    }
    pub fn migrate(&self) -> Result<()> {
        let c = self.conn();
        c.execute_batch(r#"
        PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,harness TEXT NOT NULL,external_id TEXT NOT NULL,project_json TEXT,model_ids_json TEXT NOT NULL,started_at TEXT NOT NULL,ended_at TEXT,source_path TEXT NOT NULL,raw_hash INTEGER NOT NULL,ingested_at TEXT NOT NULL,meta_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS turns(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,parent_id TEXT,role TEXT NOT NULL,idx INTEGER NOT NULL,started_at TEXT NOT NULL,duration_ms INTEGER,is_sidechain INTEGER NOT NULL,FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY,turn_id TEXT NOT NULL,session_id TEXT NOT NULL,ts TEXT NOT NULL,kind TEXT NOT NULL,payload_json TEXT NOT NULL,raw_ref TEXT NOT NULL,FOREIGN KEY(turn_id) REFERENCES turns(id) ON DELETE CASCADE);
        CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(event_id UNINDEXED, session_id UNINDEXED, text, content='');
        CREATE TABLE IF NOT EXISTS watermarks(source_path TEXT PRIMARY KEY, offset INTEGER NOT NULL DEFAULT 0,rowid INTEGER NOT NULL DEFAULT 0,cursor TEXT,updated_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS radar_token_cache(session_id TEXT PRIMARY KEY,change_key INTEGER NOT NULL,turn1_total INTEGER NOT NULL,first_user_tokens INTEGER NOT NULL,conversation INTEGER NOT NULL,tool_output INTEGER NOT NULL,thinking INTEGER NOT NULL,updated_at TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_events_session_kind ON events(session_id, kind);
        CREATE INDEX IF NOT EXISTS idx_turns_session_idx ON turns(session_id, idx);
        INSERT OR REPLACE INTO schema_meta(key,value) VALUES('schema_version','1');
        "#)?;
        // RADAR: parent→child session linkage. SQLite has no
        // `ADD COLUMN IF NOT EXISTS`, so guard via table_info to stay idempotent
        // when migrate() re-runs against a DB created before this column existed.
        let has_parent_col: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name='parent_session_id'")?
            .query_row([], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_parent_col {
            c.execute_batch("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT;")?;
        }
        Ok(())
    }
    /// RADAR: record that `child_id`'s session was spawned by `parent_id`. Sets
    /// the child row's `parent_session_id`; a no-op if the child id is unknown.
    pub fn link_child_session(&self, child_id: &str, parent_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET parent_session_id=? WHERE id=?",
            params![parent_id, child_id],
        )?;
        Ok(())
    }
    /// RADAR: merge `patch`'s top-level keys into a session's `meta_json` (incoming
    /// values win), persisting the result. Used to thread a Claude subagent's
    /// sidecar `description`/`agentType` onto its child row when the parent link is
    /// recorded, so `radar` can surface them as label/role. A no-op if the id is
    /// unknown or `patch` is not a JSON object. Idempotent (re-applying the same
    /// patch is a stable write).
    pub fn merge_session_meta(&self, id: &str, patch: &serde_json::Value) -> Result<()> {
        let Some(patch_obj) = patch.as_object() else {
            return Ok(());
        };
        let c = self.conn();
        let existing: Option<String> = c
            .query_row("SELECT meta_json FROM sessions WHERE id=?", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        let Some(existing) = existing else {
            return Ok(()); // unknown id → no-op
        };
        let mut meta: serde_json::Value =
            serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({}));
        let map = match meta.as_object_mut() {
            Some(m) => m,
            None => {
                meta = serde_json::json!({});
                meta.as_object_mut().expect("json object literal just assigned above")
            }
        };
        for (k, v) in patch_obj {
            map.insert(k.clone(), v.clone());
        }
        c.execute(
            "UPDATE sessions SET meta_json=? WHERE id=?",
            params![serde_json::to_string(&meta)?, id],
        )?;
        Ok(())
    }
    /// The session that spawned `child_id`, or `None` when unset/NULL or the
    /// child id is unknown.
    pub fn parent_of(&self, child_id: &str) -> Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT parent_session_id FROM sessions WHERE id=?",
                [child_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|o| o.flatten())
            .map_err(Into::into)
    }
    pub fn upsert_session_batch(
        &self,
        session: &Session,
        turns: &[Turn],
        events: &[EventRecord],
        watermark_offset: u64,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        // Merge-safe upsert: a later *tail* parse (offset>0) synthesizes a session
        // with sparse fields (no session_meta → empty model_ids, default meta, a
        // later started_at, null project). It must NOT clobber the good values an
        // earlier backfill wrote. So each field is merged monotonically:
        //   project/model_ids/meta → keep existing when the incoming value is the
        //     "empty" sentinel (null / '[]' / default meta);
        //   started_at → MIN (true session start wins); ended_at → MAX (extend);
        //   raw_hash/ingested_at → always take the latest (reflect current file).
        // Backfill is unaffected: it has no prior row, or arrives with full data.
        let existing_meta_json: Option<String> = tx
            .query_row(
                "SELECT meta_json FROM sessions WHERE id=?",
                [&session.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let meta_json = merged_meta_json_for_upsert(existing_meta_json.as_deref(), &session.meta)?;
        tx.execute(
            "INSERT INTO sessions(id,harness,external_id,project_json,model_ids_json,started_at,ended_at,source_path,raw_hash,ingested_at,meta_json) VALUES(?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET \
               project_json=CASE WHEN excluded.project_json IN ('null','') THEN sessions.project_json ELSE excluded.project_json END, \
               model_ids_json=CASE WHEN excluded.model_ids_json IN ('[]','') THEN sessions.model_ids_json ELSE excluded.model_ids_json END, \
               started_at=MIN(sessions.started_at, excluded.started_at), \
               ended_at=MAX(COALESCE(sessions.ended_at, excluded.ended_at), COALESCE(excluded.ended_at, sessions.ended_at)), \
               raw_hash=excluded.raw_hash, \
               ingested_at=excluded.ingested_at, \
               meta_json=excluded.meta_json",
            params![session.id, session.harness.as_str(), session.external_id, serde_json::to_string(&session.project)?, serde_json::to_string(&session.model_ids)?, session.started_at.to_rfc3339(), session.ended_at.map(|d| d.to_rfc3339()), session.source_path.to_string_lossy(), session.raw_hash as i64, session.ingested_at.to_rfc3339(), meta_json],
        )?;
        for t in turns {
            tx.execute("INSERT INTO turns(id,session_id,parent_id,role,idx,started_at,duration_ms,is_sidechain) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET duration_ms=COALESCE(excluded.duration_ms, turns.duration_ms), idx=excluded.idx", params![t.id,t.session_id,t.parent_id,format!("{:?}",t.role).to_lowercase(),t.index,t.started_at.to_rfc3339(),t.duration_ms.map(|x| x as i64), if t.is_sidechain{1}else{0}])?;
        }
        for e in events {
            let payload = serde_json::to_string(&e.event)?;
            let raw = serde_json::to_string(&e.raw_ref)?;
            let text = e.event.searchable_text();
            tx.execute("INSERT OR REPLACE INTO events(id,turn_id,session_id,ts,kind,payload_json,raw_ref) VALUES(?,?,?,?,?,?,?)", params![e.id,e.turn_id,e.session_id,e.ts.to_rfc3339(),e.event.kind_name(),payload,raw])?;
            tx.execute("INSERT INTO events_fts(rowid,event_id,session_id,text) VALUES((SELECT rowid FROM events WHERE id=?1),?1,?2,?3) ON CONFLICT DO NOTHING", params![e.id,e.session_id,text]).ok();
        }
        tx.execute("INSERT INTO watermarks(source_path,offset,rowid,cursor,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(source_path) DO UPDATE SET offset=excluded.offset, updated_at=excluded.updated_at", params![session.source_path.to_string_lossy(), watermark_offset as i64, 0i64, Option::<String>::None, Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(())
    }
    /// Byte watermark for a source file: the absolute offset up to which we have
    /// already ingested. Returns 0 when the file has never been seen, so callers
    /// read the whole file on first sight and only `bytes[offset..]` thereafter.
    pub fn watermark_offset(&self, path: &Path) -> Result<u64> {
        Ok(self
            .conn()
            .query_row(
                "SELECT offset FROM watermarks WHERE source_path=?",
                [path.to_string_lossy().to_string()],
                |r| Ok(r.get::<_, i64>(0)? as u64),
            )
            .optional()?
            .unwrap_or(0))
    }
    pub fn source_raw_hash(&self, path: &Path) -> Result<Option<u64>> {
        let rows = {
            let c = self.conn();
            let mut st = c.prepare(
                "SELECT harness,external_id,raw_hash,meta_json FROM sessions WHERE source_path=?",
            )?;
            let rows = st.query_map([path.to_string_lossy().to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if let Some(expected) = claude_subagent_external_id_from_path(path) {
            if let Some((_, _, hash, _)) = rows.iter().find(|(harness, external_id, _, _)| {
                harness == "claude_code" && external_id == &expected
            }) {
                return Ok(Some(*hash));
            }
            if rows
                .iter()
                .any(|(harness, _, _, _)| harness == "claude_code")
            {
                return Ok(None);
            }
        }
        if rows.iter().any(|(harness, _, _, _)| harness == "codex")
            && codex_rollout_has_session_identity(path)
        {
            if let Some((_, _, hash, _)) = rows
                .iter()
                .find(|(harness, _, _, meta)| harness == "codex" && codex_meta_has_identity(meta))
            {
                return Ok(Some(*hash));
            }
            return Ok(None);
        }
        Ok(rows.first().map(|(_, _, hash, _)| *hash))
    }
    /// RADAR (Fix #3): fetch a session's cached estimated-composition token sums,
    /// but ONLY when the stored `change_key` matches `change_key` (the session's
    /// current content hash). A mismatch (transcript changed) or absent row returns
    /// `None`, signalling the caller to re-tokenize. `u64` round-trips through
    /// SQLite's `i64` by bit reinterpretation (same as `raw_hash`/watermark offset).
    pub fn radar_token_cache_get(
        &self,
        session_id: &str,
        change_key: u64,
    ) -> Result<Option<RadarTokenCounts>> {
        self.conn()
            .query_row(
                "SELECT turn1_total,first_user_tokens,conversation,tool_output,thinking \
                 FROM radar_token_cache WHERE session_id=? AND change_key=?",
                params![session_id, change_key as i64],
                |r| {
                    Ok(RadarTokenCounts {
                        turn1_total: r.get::<_, i64>(0)? as u64,
                        first_user_tokens: r.get::<_, i64>(1)? as u64,
                        conversation: r.get::<_, i64>(2)? as u64,
                        tool_output: r.get::<_, i64>(3)? as u64,
                        thinking: r.get::<_, i64>(4)? as u64,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    /// RADAR (Fix #3): upsert a session's estimated-composition token sums under its
    /// current `change_key`. One row per session (PK `session_id`), so a re-tokenize
    /// after a content change replaces the stale row in place.
    pub fn radar_token_cache_put(
        &self,
        session_id: &str,
        change_key: u64,
        counts: &RadarTokenCounts,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO radar_token_cache\
             (session_id,change_key,turn1_total,first_user_tokens,conversation,tool_output,thinking,updated_at) \
             VALUES(?,?,?,?,?,?,?,?)",
            params![
                session_id,
                change_key as i64,
                counts.turn1_total as i64,
                counts.first_user_tokens as i64,
                counts.conversation as i64,
                counts.tool_output as i64,
                counts.thinking as i64,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }
    pub fn sessions(&self) -> Result<Vec<Session>> {
        let c = self.conn();
        let mut st=c.prepare("SELECT id,harness,external_id,project_json,model_ids_json,started_at,ended_at,source_path,raw_hash,ingested_at,meta_json FROM sessions ORDER BY started_at DESC")?;
        let rows = st.query_map([], row_session)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn session_events(&self, sid: &str) -> Result<Vec<(Turn, EventRecord)>> {
        let c = self.conn();
        let mut st=c.prepare("SELECT t.id,t.session_id,t.parent_id,t.role,t.idx,t.started_at,t.duration_ms,t.is_sidechain,e.id,e.ts,e.payload_json,e.raw_ref FROM events e JOIN turns t ON e.turn_id=t.id WHERE e.session_id=? ORDER BY e.ts, CAST(json_extract(e.raw_ref,'$.offset') AS INTEGER), e.id")?;
        let rows = st.query_map([sid], |r| {
            let role = parse_role(r.get::<_, String>(3)?.as_str());
            let turn = Turn {
                id: r.get(0)?,
                session_id: r.get(1)?,
                parent_id: r.get(2)?,
                role,
                index: r.get::<_, i64>(4)? as u32,
                started_at: parse_dt(&r.get::<_, String>(5)?),
                duration_ms: r.get::<_, Option<i64>>(6)?.map(|x| x as u64),
                is_sidechain: r.get::<_, i64>(7)? != 0,
            };
            let payload: String = r.get(10)?;
            let raw: String = r.get(11)?;
            let ev = EventRecord {
                id: r.get(8)?,
                turn_id: turn.id.clone(),
                session_id: turn.session_id.clone(),
                ts: parse_dt(&r.get::<_, String>(9)?),
                event: serde_json::from_str(&payload).expect("event payload was serialized by us; valid JSON"),
                raw_ref: serde_json::from_str(&raw).expect("raw_ref was serialized by us; valid JSON"),
            };
            Ok((turn, ev))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn counts(&self) -> Result<(u32, u64, u32)> {
        let c = self.conn();
        let s: i64 = c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        Ok((s as u32, e as u64, 0))
    }
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
fn parse_role(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "tool" => Role::Tool,
        _ => Role::System,
    }
}

fn merged_meta_json_for_upsert(
    existing: Option<&str>,
    incoming: &serde_json::Value,
) -> Result<String> {
    serde_json::to_string(&merged_meta_for_upsert(existing, incoming)).map_err(Into::into)
}

fn merged_meta_for_upsert(
    existing: Option<&str>,
    incoming: &serde_json::Value,
) -> serde_json::Value {
    let Some(existing) = existing else {
        return incoming.clone();
    };
    let existing_value = serde_json::from_str::<serde_json::Value>(existing)
        .unwrap_or_else(|_| serde_json::json!({}));
    let (Some(existing_obj), Some(incoming_obj)) =
        (existing_value.as_object(), incoming.as_object())
    else {
        return if incoming.is_null() {
            existing_value
        } else {
            incoming.clone()
        };
    };
    if incoming_obj.is_empty() || incoming_is_empty_ignored_sentinel(incoming_obj) {
        return existing_value;
    }

    let mut merged = existing_obj.clone();
    for (key, value) in incoming_obj {
        if value.is_null() {
            continue;
        }
        if key == "ignored_record_types" {
            merge_ignored_record_types(&mut merged, value);
            continue;
        }
        if value.as_str() == Some("")
            && merged.get(key).and_then(serde_json::Value::as_str) != Some("")
        {
            continue;
        }
        merged.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(merged)
}

fn incoming_is_empty_ignored_sentinel(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    map.len() == 1
        && map
            .get("ignored_record_types")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|ignored| ignored.is_empty())
}

fn merge_ignored_record_types(
    merged: &mut serde_json::Map<String, serde_json::Value>,
    incoming: &serde_json::Value,
) {
    let Some(incoming) = incoming.as_object() else {
        return;
    };
    let target = merged
        .entry("ignored_record_types".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !target.is_object() {
        *target = serde_json::json!({});
    }
    let target = target.as_object_mut().expect("target was normalized");
    for (key, value) in incoming {
        let incoming_count = value.as_u64().unwrap_or(1);
        let existing_count = target
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        target.insert(
            key.clone(),
            serde_json::json!(existing_count.max(incoming_count)),
        );
    }
}

fn claude_subagent_external_id_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("agent-") {
        return None;
    }
    let under_subagents = path.components().any(|c| {
        matches!(
            c,
            std::path::Component::Normal(part) if part == "subagents"
        )
    });
    under_subagents.then(|| stem.to_string())
}

fn codex_meta_has_identity(meta_json: &str) -> bool {
    let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_json) else {
        return false;
    };
    [
        "thread_source",
        "parent_thread_id",
        "originator",
        "agent_nickname",
    ]
    .iter()
    .any(|key| meta.get(*key).is_some_and(|v| !v.is_null()))
}

fn codex_rollout_has_session_identity(path: &Path) -> bool {
    let is_rollout = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"));
    if !is_rollout {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Some(first_line) = bytes.split(|b| *b == b'\n').find(|line| !line.is_empty()) else {
        return false;
    };
    let Ok(line) = std::str::from_utf8(first_line) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    v.get("type").and_then(serde_json::Value::as_str) == Some("session_meta")
        && [
            "thread_source",
            "parent_thread_id",
            "originator",
            "agent_nickname",
        ]
        .iter()
        .any(|key| v.pointer(&format!("/payload/{key}")).is_some())
}

fn row_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let h: String = r.get(1)?;
    Ok(Session {
        id: r.get(0)?,
        harness: match h.as_str() {
            "claude_code" => Harness::ClaudeCode,
            "codex" => Harness::Codex,
            "cursor" => Harness::Cursor,
            "hermes" => Harness::Hermes,
            x => Harness::Generic(x.to_string()),
        },
        external_id: r.get(2)?,
        project: serde_json::from_str(&r.get::<_, String>(3)?).ok().flatten(),
        model_ids: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        started_at: parse_dt(&r.get::<_, String>(5)?),
        ended_at: r.get::<_, Option<String>>(6)?.map(|s| parse_dt(&s)),
        source_path: PathBuf::from(r.get::<_, String>(7)?),
        raw_hash: r.get::<_, i64>(8)? as u64,
        ingested_at: parse_dt(&r.get::<_, String>(9)?),
        meta: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or(serde_json::Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed one session of `harness` with `n_events` UserPrompt events under a
    /// single turn, via the real upsert path.
    fn seed_session(store: &Store, id: &str, harness: Harness, n_events: usize) {
        let now = Utc::now();
        let session = Session {
            id: id.into(),
            harness,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let turn = Turn {
            id: format!("{id}-t0"),
            session_id: id.into(),
            parent_id: None,
            role: Role::User,
            index: 0,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let events = (0..n_events)
            .map(|i| EventRecord {
                id: format!("{id}-e{i}"),
                turn_id: turn.id.clone(),
                session_id: id.into(),
                ts: now,
                event: Event::UserPrompt {
                    text: format!("prompt {i}"),
                    attachments: vec![],
                    is_meta: false,
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: i as u64,
                    line: i as u32,
                },
            })
            .collect::<Vec<_>>();
        store
            .upsert_session_batch(&session, &[turn], &events, 0)
            .unwrap();
    }

    /// Fix #3 — INCREMENTAL token cache: the radar token-count cache is keyed by
    /// `(session_id, change_key)`. A `put` then a `get` with the SAME change-key
    /// returns the stored counts; a `get` with a DIFFERENT change-key (the
    /// transcript changed) returns `None` so the caller re-tokenizes. A second `put`
    /// upserts in place (one row per session).
    #[test]
    fn radar_token_cache_round_trips_by_change_key() {
        let store = Store::memory().unwrap();
        seed_session(&store, "s1", Harness::Codex, 1);

        // Miss before anything is cached.
        assert_eq!(store.radar_token_cache_get("s1", 0xAABB).unwrap(), None);

        let counts = RadarTokenCounts {
            turn1_total: 8000,
            first_user_tokens: 500,
            conversation: 3000,
            tool_output: 1500,
            thinking: 200,
        };
        store.radar_token_cache_put("s1", 0xAABB, &counts).unwrap();

        // Hit with the matching change-key.
        assert_eq!(
            store.radar_token_cache_get("s1", 0xAABB).unwrap(),
            Some(counts),
            "matching change-key returns the cached counts"
        );
        // Miss with a different change-key (content changed → must re-tokenize).
        assert_eq!(
            store.radar_token_cache_get("s1", 0x9999).unwrap(),
            None,
            "a changed change-key is a cache miss"
        );

        // Upsert in place: new key + new counts overwrite the single row.
        let counts2 = RadarTokenCounts {
            turn1_total: 9000,
            first_user_tokens: 600,
            conversation: 4000,
            tool_output: 1000,
            thinking: 0,
        };
        store.radar_token_cache_put("s1", 0x9999, &counts2).unwrap();
        assert_eq!(
            store.radar_token_cache_get("s1", 0x9999).unwrap(),
            Some(counts2),
            "the new change-key now hits with the updated counts"
        );
        assert_eq!(
            store.radar_token_cache_get("s1", 0xAABB).unwrap(),
            None,
            "the old change-key no longer hits (row was replaced, not duplicated)"
        );
    }

    #[test]
    fn upsert_tail_meta_preserves_codex_subagent_identity() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let source_path = PathBuf::from("/tmp/rollout-child.jsonl");
        let session = Session {
            id: "child".into(),
            harness: Harness::Codex,
            external_id: "child".into(),
            project: None,
            model_ids: vec!["openai".into()],
            started_at: now,
            ended_at: None,
            source_path: source_path.clone(),
            raw_hash: 1,
            ingested_at: now,
            meta: serde_json::json!({
                "thread_source": "subagent",
                "parent_thread_id": "parent",
                "agent_nickname": "Russell",
                "originator": "Codex Desktop"
            }),
        };
        store.upsert_session_batch(&session, &[], &[], 100).unwrap();

        let mut tail = session.clone();
        tail.raw_hash = 2;
        tail.ingested_at = now + chrono::Duration::seconds(1);
        tail.meta = serde_json::json!({
            "ignored_record_types": {
                "event_msg/web_search_end": 1
            }
        });
        store.upsert_session_batch(&tail, &[], &[], 120).unwrap();

        let got = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == "child")
            .unwrap();
        assert_eq!(
            got.meta
                .get("thread_source")
                .and_then(serde_json::Value::as_str),
            Some("subagent"),
            "tail parses must not erase Codex subagent identity metadata"
        );
        assert_eq!(
            got.meta
                .get("parent_thread_id")
                .and_then(serde_json::Value::as_str),
            Some("parent")
        );
        assert_eq!(
            got.meta
                .pointer("/ignored_record_types/event_msg~1web_search_end")
                .and_then(serde_json::Value::as_u64),
            Some(1),
            "tail diagnostics should be merged instead of replacing identity"
        );
        assert_eq!(store.watermark_offset(&source_path).unwrap(), 120);
    }

    #[test]
    fn source_raw_hash_for_claude_subagent_requires_agent_identity() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let source_path =
            PathBuf::from("/tmp/root-session/subagents/workflows/wf_123/agent-child123.jsonl");
        let stale = Session {
            id: "stale-child".into(),
            harness: Harness::ClaudeCode,
            external_id: "root-session".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: source_path.clone(),
            raw_hash: 7,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&stale, &[], &[], 100).unwrap();

        assert_eq!(
            store.source_raw_hash(&source_path).unwrap(),
            None,
            "mis-keyed nested Claude subagents must be reparsed instead of hash-skipped forever"
        );

        let mut repaired = stale.clone();
        repaired.id = "repaired-child".into();
        repaired.external_id = "agent-child123".into();
        repaired.raw_hash = 9;
        store
            .upsert_session_batch(&repaired, &[], &[], 100)
            .unwrap();

        assert_eq!(
            store.source_raw_hash(&source_path).unwrap(),
            Some(9),
            "once the agent-file identity exists, unchanged subagents can use the normal hash skip"
        );
    }

    #[test]
    fn source_raw_hash_for_codex_rollout_requires_session_identity_meta() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir
            .path()
            .join("rollout-2026-06-25T12-00-00-019f0040-0000-7000-8000-000000000001.jsonl");
        std::fs::write(
            &source_path,
            "{\"timestamp\":\"2026-06-25T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019f0040-0000-7000-8000-000000000001\",\"cwd\":\"/tmp/Codex\",\"originator\":\"Codex Desktop\",\"thread_source\":\"subagent\",\"parent_thread_id\":\"parent\"}}\n",
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let stale = Session {
            id: "stale-codex".into(),
            harness: Harness::Codex,
            external_id: "019f0040-0000-7000-8000-000000000001".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: source_path.clone(),
            raw_hash: 7,
            ingested_at: now,
            meta: serde_json::json!({
                "ignored_record_types": {
                    "event_msg/web_search_end": 1
                }
            }),
        };
        store.upsert_session_batch(&stale, &[], &[], 100).unwrap();

        assert_eq!(
            store.source_raw_hash(&source_path).unwrap(),
            None,
            "Codex rows whose live tail erased session_meta identity must be reparsed once"
        );

        let mut repaired = stale.clone();
        repaired.raw_hash = 9;
        repaired.meta = serde_json::json!({
            "thread_source": "subagent",
            "parent_thread_id": "parent",
            "originator": "Codex Desktop",
            "ignored_record_types": {
                "event_msg/web_search_end": 1
            }
        });
        store
            .upsert_session_batch(&repaired, &[], &[], 100)
            .unwrap();

        assert_eq!(store.source_raw_hash(&source_path).unwrap(), Some(9));
    }

    #[test]
    fn link_child_session_sets_and_reads_parent() {
        let store = Store::memory().unwrap();
        store.migrate().unwrap();
        // Two minimal sessions persisted via the real upsert path (seed_session
        // builds a valid Session with the given id and zero events).
        seed_session(&store, "parent", Harness::ClaudeCode, 0);
        seed_session(&store, "child", Harness::ClaudeCode, 0);
        store.link_child_session("child", "parent").unwrap();
        assert_eq!(
            store.parent_of("child").unwrap(),
            Some("parent".to_string())
        );
        assert_eq!(store.parent_of("parent").unwrap(), None);
    }

}
