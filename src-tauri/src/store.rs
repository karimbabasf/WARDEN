use crate::ir::*;
use crate::util::{ensure_parent, stable_id};
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

/// Per-harness rollup of ingested volume. `harness` is the snake_case
/// `Harness::as_str()` value so the war-room can theme it directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessRollup {
    pub harness: String,
    pub sessions: u32,
    pub events: u64,
}

/// The profile shape the overlay HUD consumes: global totals plus a per-harness
/// breakdown. Replaces the bare `CompetenceProfile` previously returned by
/// `query_profile`; the three top-level counts are byte-identical to the old
/// fields so existing consumers keep working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileBreakdown {
    pub session_count: u32,
    pub event_count: u64,
    pub finding_count: u32,
    pub by_harness: Vec<HarnessRollup>,
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
        CREATE TABLE IF NOT EXISTS features(session_id TEXT PRIMARY KEY,vector_json TEXT NOT NULL,computed_at TEXT NOT NULL,featurizer_version TEXT NOT NULL,FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS profile(id INTEGER PRIMARY KEY CHECK(id=1),vector_json TEXT NOT NULL,updated_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS profile_history(ts TEXT NOT NULL,vector_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS findings(id TEXT PRIMARY KEY,pattern_id TEXT NOT NULL,session_ids_json TEXT NOT NULL,severity INTEGER NOT NULL,frequency REAL NOT NULL,est_cost_tokens INTEGER NOT NULL,est_cost_minutes INTEGER NOT NULL,confidence REAL NOT NULL,evidence_json TEXT NOT NULL,status TEXT NOT NULL,created_at TEXT NOT NULL,rationale TEXT NOT NULL,title TEXT NOT NULL,verifier_verdict TEXT);
        CREATE TABLE IF NOT EXISTS diagnoses(id TEXT PRIMARY KEY,created_at TEXT NOT NULL,ranked_findings_json TEXT NOT NULL,do_json TEXT NOT NULL,stop_json TEXT NOT NULL,narrative TEXT NOT NULL,detector_only INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY,finding_id TEXT,kind TEXT NOT NULL,target_path TEXT NOT NULL,diff TEXT NOT NULL,status TEXT NOT NULL,applied_at TEXT,backup_path TEXT,created_at TEXT);
        CREATE TABLE IF NOT EXISTS fugu_runs(id TEXT PRIMARY KEY,stage TEXT NOT NULL,model TEXT NOT NULL,effort TEXT NOT NULL,req_hash TEXT NOT NULL,input_tokens INTEGER NOT NULL,output_tokens INTEGER NOT NULL,orchestration_input_tokens INTEGER NOT NULL,orchestration_output_tokens INTEGER NOT NULL,latency_ms INTEGER NOT NULL,cost_usd REAL NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS interjections(id TEXT PRIMARY KEY,ts TEXT NOT NULL,pattern_id TEXT NOT NULL,session_id TEXT,shown INTEGER NOT NULL,dismissed INTEGER NOT NULL,muted INTEGER NOT NULL);
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
        // M4 Forge: the `artifacts` table was created with 8 columns; apply/revert
        // need two more (the literal block to ensure + the pre-image hash to verify
        // a restore). Same idempotent `pragma_table_info` guard as above so migrate()
        // re-runs cleanly on a DB created before M4.
        let has_block_col: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('artifacts') WHERE name='block'")?
            .query_row([], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_block_col {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN block TEXT NOT NULL DEFAULT '';")?;
        }
        let has_pre_image_col: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('artifacts') WHERE name='pre_image_sha256'")?
            .query_row([], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_pre_image_col {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN pre_image_sha256 TEXT;")?;
        }
        // M4 Forge drift guard: the SHA-256 of the content WARDEN wrote at apply time.
        // Revert compares the current target against this to refuse clobbering
        // out-of-band edits. Idempotent guard so migrate() re-runs on a pre-drift DB.
        let has_post_image_col: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('artifacts') WHERE name='post_image_sha256'")?
            .query_row([], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_post_image_col {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN post_image_sha256 TEXT;")?;
        }
        // `created_at` orders the artifact history. Older DBs (and the legacy
        // 8-column shell) lack it — add it idempotently so ORDER BY created_at works.
        let has_created_col: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('artifacts') WHERE name='created_at'")?
            .query_row([], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_created_col {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN created_at TEXT;")?;
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
                meta.as_object_mut().unwrap()
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
            if let Some((_, _, hash, _)) = rows
                .iter()
                .find(|(harness, external_id, _, _)| {
                    harness == "claude_code" && external_id == &expected
                })
            {
                return Ok(Some(*hash));
            }
            if rows.iter().any(|(harness, _, _, _)| harness == "claude_code") {
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
        let rows = st.query_map([], |r| row_session(r))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn session_events(&self, sid: &str) -> Result<Vec<(Turn, EventRecord)>> {
        let c = self.conn();
        let mut st=c.prepare("SELECT t.id,t.session_id,t.parent_id,t.role,t.idx,t.started_at,t.duration_ms,t.is_sidechain,e.id,e.ts,e.payload_json,e.raw_ref FROM events e JOIN turns t ON e.turn_id=t.id WHERE e.session_id=? ORDER BY t.idx,e.ts,e.id")?;
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
                event: serde_json::from_str(&payload).unwrap(),
                raw_ref: serde_json::from_str(&raw).unwrap(),
            };
            Ok((turn, ev))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    /// Resolve one stored event's ground-truth text for the evidence drill-down
    /// fallback (Task 9). Returns the event's `searchable_text()` (the same
    /// display surface the detectors quote from) plus its `raw_ref.source_path`,
    /// keyed by `(session_id, event_id)` so a sibling session's id collision
    /// cannot leak the wrong row. READ-ONLY — a `SELECT` only. `Ok(None)` when no
    /// such event exists; the caller keeps the honest "no excerpt" placeholder.
    pub fn event_text(
        &self,
        session_id: &str,
        event_id: &str,
    ) -> Result<Option<(String, Option<PathBuf>)>> {
        let c = self.conn();
        let row = c
            .query_row(
                "SELECT payload_json, raw_ref FROM events WHERE session_id=? AND id=?",
                params![session_id, event_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((payload, raw)) = row else {
            return Ok(None);
        };
        let event: Event = serde_json::from_str(&payload)?;
        let source_path = serde_json::from_str::<RawRef>(&raw)
            .ok()
            .map(|r| r.source_path);
        Ok(Some((event.searchable_text(), source_path)))
    }
    pub fn all_features(&self) -> Result<Vec<FeatureVector>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT vector_json FROM features")?;
        let rows = st.query_map([], |r| {
            let s: String = r.get(0)?;
            Ok(serde_json::from_str(&s).unwrap())
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn save_feature(&self, f: &FeatureVector, version: &str) -> Result<()> {
        self.conn().execute("INSERT OR REPLACE INTO features(session_id,vector_json,computed_at,featurizer_version) VALUES(?,?,?,?)", params![f.session_id, serde_json::to_string(f)?, Utc::now().to_rfc3339(), version])?;
        Ok(())
    }
    pub fn save_profile(&self, p: &CompetenceProfile) -> Result<()> {
        let s = serde_json::to_string(p)?;
        let now = Utc::now().to_rfc3339();
        let c = self.conn();
        c.execute(
            "INSERT OR REPLACE INTO profile(id,vector_json,updated_at) VALUES(1,?,?)",
            params![s, now],
        )?;
        c.execute(
            "INSERT INTO profile_history(ts,vector_json) VALUES(?,?)",
            params![now, s],
        )?;
        Ok(())
    }
    pub fn profile(&self) -> Result<CompetenceProfile> {
        let opt: String = self
            .conn()
            .query_row("SELECT vector_json FROM profile WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_default();
        if opt.is_empty() {
            Ok(CompetenceProfile::default())
        } else {
            Ok(serde_json::from_str(&opt)?)
        }
    }
    pub fn counts(&self) -> Result<(u32, u64, u32)> {
        let c = self.conn();
        let s: i64 = c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        let f: i64 = c.query_row("SELECT COUNT(*) FROM findings", [], |r| r.get(0))?;
        Ok((s as u32, e as u64, f as u32))
    }
    /// Global counts plus a per-harness rollup (sessions + their events), grouped
    /// by the stored harness string. The per-harness session/event sums equal the
    /// global `session_count`/`event_count` totals (every session has a harness,
    /// every event belongs to a session). Rows are ordered by session volume so
    /// the dominant harness leads the HUD.
    pub fn profile_with_harness_breakdown(&self) -> Result<ProfileBreakdown> {
        let (session_count, event_count, finding_count) = self.counts()?;
        let c = self.conn();
        // LEFT JOIN so a harness with sessions but zero events still appears with
        // events=0, and the session count stays exact (one row per session).
        let mut st = c.prepare(
            "SELECT s.harness, COUNT(DISTINCT s.id), COUNT(e.id) \
             FROM sessions s LEFT JOIN events e ON e.session_id = s.id \
             GROUP BY s.harness ORDER BY COUNT(DISTINCT s.id) DESC, s.harness ASC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(HarnessRollup {
                harness: r.get::<_, String>(0)?,
                sessions: r.get::<_, i64>(1)? as u32,
                events: r.get::<_, i64>(2)? as u64,
            })
        })?;
        let by_harness = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ProfileBreakdown {
            session_count,
            event_count,
            finding_count,
            by_harness,
        })
    }
    /// Resolve the harness string for a single session, if known. Used by the
    /// brain to tag candidates/verdicts with the harness of the session their
    /// evidence references. `None` when the session id is unknown.
    pub fn session_harness(&self, session_id: &str) -> Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT harness FROM sessions WHERE id=? LIMIT 1",
                [session_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }
    /// Load a persisted finding by id (reconstructed from the `findings` table).
    /// Returns `None` when no such finding has been saved. Used by fix preview.
    pub fn finding_by_id(&self, id: &str) -> Result<Option<Finding>> {
        self.conn()
            .query_row(
                "SELECT id,pattern_id,title,severity,frequency,est_cost_tokens,est_cost_minutes,confidence,rationale,evidence_json,status,verifier_verdict FROM findings WHERE id=? LIMIT 1",
                [id],
                |r| {
                    Ok(Finding {
                        id: r.get(0)?,
                        pattern_id: r.get(1)?,
                        title: r.get(2)?,
                        severity: r.get::<_, i64>(3)? as u8,
                        frequency: r.get(4)?,
                        est_cost_tokens: r.get::<_, i64>(5)? as u64,
                        est_cost_minutes: r.get::<_, i64>(6)? as u64,
                        confidence: r.get(7)?,
                        rationale: r.get(8)?,
                        evidence: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
                        status: r.get(10)?,
                        verifier_verdict: r.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn all_findings(&self) -> Result<Vec<Finding>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT id,pattern_id,title,severity,frequency,est_cost_tokens,est_cost_minutes,confidence,rationale,evidence_json,status,verifier_verdict FROM findings ORDER BY created_at DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Finding {
                id: r.get(0)?,
                pattern_id: r.get(1)?,
                title: r.get(2)?,
                severity: r.get::<_, i64>(3)? as u8,
                frequency: r.get(4)?,
                est_cost_tokens: r.get::<_, i64>(5)? as u64,
                est_cost_minutes: r.get::<_, i64>(6)? as u64,
                confidence: r.get(7)?,
                rationale: r.get(8)?,
                evidence: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
                status: r.get(10)?,
                verifier_verdict: r.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn save_findings(&self, findings: &[Finding]) -> Result<()> {
        let c = self.conn();
        for f in findings {
            let sids: Vec<_> = f.evidence.iter().map(|e| e.session_id.clone()).collect();
            c.execute("INSERT OR REPLACE INTO findings(id,pattern_id,session_ids_json,severity,frequency,est_cost_tokens,est_cost_minutes,confidence,evidence_json,status,created_at,rationale,title,verifier_verdict) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![f.id,f.pattern_id,serde_json::to_string(&sids)?,f.severity as i64,f.frequency,f.est_cost_tokens as i64,f.est_cost_minutes as i64,f.confidence,serde_json::to_string(&f.evidence)?,f.status,Utc::now().to_rfc3339(),f.rationale,f.title,f.verifier_verdict])?;
        }
        Ok(())
    }
    pub fn save_diagnosis(&self, d: &Diagnosis) -> Result<()> {
        self.conn().execute("INSERT OR REPLACE INTO diagnoses(id,created_at,ranked_findings_json,do_json,stop_json,narrative,detector_only) VALUES(?,?,?,?,?,?,?)", params![d.id,d.created_at.to_rfc3339(),serde_json::to_string(&d.ranked_findings)?,serde_json::to_string(&d.do_items)?,serde_json::to_string(&d.stop_items)?,d.narrative, if d.detector_only{1}else{0}])?;
        Ok(())
    }
    pub fn latest_diagnosis(&self) -> Result<Option<Diagnosis>> {
        self.conn().query_row("SELECT id,created_at,ranked_findings_json,do_json,stop_json,narrative,detector_only FROM diagnoses ORDER BY created_at DESC LIMIT 1", [], |r| Ok(Diagnosis{id:r.get(0)?,created_at:parse_dt(&r.get::<_,String>(1)?),ranked_findings:serde_json::from_str(&r.get::<_,String>(2)?).unwrap(),do_items:serde_json::from_str(&r.get::<_,String>(3)?).unwrap(),stop_items:serde_json::from_str(&r.get::<_,String>(4)?).unwrap(),narrative:r.get(5)?,detector_only:r.get::<_,i64>(6)?!=0})).optional().map_err(Into::into)
    }
    pub fn save_fugu_run(
        &self,
        stage: &str,
        model: &str,
        effort: &str,
        req_hash: &str,
        input: u64,
        output: u64,
        oi: u64,
        oo: u64,
        latency_ms: u64,
    ) -> Result<()> {
        let id = stable_id(&[
            stage,
            model,
            req_hash,
            &Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
                .to_string(),
        ]);
        self.conn().execute("INSERT INTO fugu_runs(id,stage,model,effort,req_hash,input_tokens,output_tokens,orchestration_input_tokens,orchestration_output_tokens,latency_ms,cost_usd,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", params![id,stage,model,effort,req_hash,input as i64,output as i64,oi as i64,oo as i64,latency_ms as i64,0.0f64,Utc::now().to_rfc3339()])?;
        Ok(())
    }

    // ── M4 Forge: reversible apply artifacts ──────────────────────────────────
    // Mirror the findings CRUD shape (`save_findings`/`finding_by_id`/`all_findings`).
    // The artifact row is the unit of reversible history: stage persists it PENDING,
    // apply/revert update its status + backup/pre-image columns. `created_at` is
    // recorded on first save so `all_artifacts` can order by it.

    /// Insert-or-replace an artifact row. PENDING on stage; status/backup/pre-image
    /// fields updated through `update_artifact_status`. `created_at` is set only when
    /// the row is new so re-saving (e.g. a status flip) preserves stage ordering.
    pub fn save_artifact(&self, a: &Artifact) -> Result<()> {
        let c = self.conn();
        let existing_created: Option<String> = c
            .query_row(
                "SELECT created_at FROM artifacts WHERE id=?",
                [&a.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let created_at = existing_created.unwrap_or_else(|| Utc::now().to_rfc3339());
        c.execute(
            "INSERT OR REPLACE INTO artifacts(id,finding_id,kind,target_path,diff,status,applied_at,backup_path,block,pre_image_sha256,post_image_sha256,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                a.id,
                a.finding_id,
                a.kind,
                a.target_path,
                a.diff,
                a.status,
                a.applied_at,
                a.backup_path,
                a.block,
                a.pre_image_sha256,
                a.post_image_sha256,
                created_at
            ],
        )?;
        Ok(())
    }

    /// Load a single artifact by id; `None` when absent. Used for state
    /// reconciliation and by apply/revert to resolve the row to act on.
    pub fn artifact_by_id(&self, id: &str) -> Result<Option<Artifact>> {
        self.conn()
            .query_row(ARTIFACT_SELECT_BY_ID, [id], row_to_artifact)
            .optional()
            .map_err(Into::into)
    }

    /// All artifacts staged for a given finding/issue id, newest first.
    pub fn artifacts_for_finding(&self, finding_id: &str) -> Result<Vec<Artifact>> {
        let c = self.conn();
        let mut st = c.prepare(ARTIFACT_SELECT_FOR_FINDING)?;
        let rows = st.query_map([finding_id], row_to_artifact)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Every artifact, newest first (applied_at when set, else created_at).
    pub fn all_artifacts(&self) -> Result<Vec<Artifact>> {
        let c = self.conn();
        let mut st = c.prepare(ARTIFACT_SELECT_ALL)?;
        let rows = st.query_map([], row_to_artifact)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Flip an artifact's lifecycle state and record the apply/revert outcome.
    /// `applied_at`/`backup_path`/`pre_image_sha256`/`post_image_sha256` are written
    /// verbatim (null clears them) so a no-op apply records `applied` with no backup,
    /// and revert keeps the backup path for the audit trail.
    #[allow(clippy::too_many_arguments)]
    pub fn update_artifact_status(
        &self,
        id: &str,
        status: &str,
        applied_at: Option<&str>,
        backup_path: Option<&str>,
        pre_image_sha256: Option<&str>,
        post_image_sha256: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE artifacts SET status=?,applied_at=?,backup_path=?,pre_image_sha256=?,post_image_sha256=? WHERE id=?",
            params![status, applied_at, backup_path, pre_image_sha256, post_image_sha256, id],
        )?;
        Ok(())
    }
}

const ARTIFACT_SELECT_BY_ID: &str = "SELECT id,finding_id,kind,target_path,diff,status,applied_at,backup_path,block,pre_image_sha256,post_image_sha256 FROM artifacts WHERE id=? LIMIT 1";

const ARTIFACT_SELECT_FOR_FINDING: &str = "SELECT id,finding_id,kind,target_path,diff,status,applied_at,backup_path,block,pre_image_sha256,post_image_sha256 FROM artifacts WHERE finding_id=? ORDER BY COALESCE(applied_at, created_at) DESC";

const ARTIFACT_SELECT_ALL: &str = "SELECT id,finding_id,kind,target_path,diff,status,applied_at,backup_path,block,pre_image_sha256,post_image_sha256 FROM artifacts ORDER BY COALESCE(applied_at, created_at) DESC";

fn row_to_artifact(r: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: r.get(0)?,
        finding_id: r.get(1)?,
        kind: r.get(2)?,
        target_path: r.get(3)?,
        diff: r.get(4)?,
        status: r.get(5)?,
        applied_at: r.get(6)?,
        backup_path: r.get(7)?,
        block: r.get(8)?,
        pre_image_sha256: r.get(9)?,
        post_image_sha256: r.get(10)?,
    })
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
    let existing_value =
        serde_json::from_str::<serde_json::Value>(existing).unwrap_or_else(|_| serde_json::json!({}));
    let (Some(existing_obj), Some(incoming_obj)) = (existing_value.as_object(), incoming.as_object())
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
        if value.as_str() == Some("") && merged.get(key).and_then(serde_json::Value::as_str) != Some("")
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
        target.insert(key.clone(), serde_json::json!(existing_count.max(incoming_count)));
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
    ["thread_source", "parent_thread_id", "originator", "agent_nickname"]
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
        && ["thread_source", "parent_thread_id", "originator", "agent_nickname"]
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
            Some(counts.clone()),
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
    fn harness_breakdown_sums_to_totals() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 3);
        seed_session(&store, "c2", Harness::ClaudeCode, 2);
        seed_session(&store, "x1", Harness::Codex, 4);

        let pb = store.profile_with_harness_breakdown().unwrap();

        // Global totals: 3 sessions, 3+2+4 = 9 events.
        assert_eq!(pb.session_count, 3);
        assert_eq!(pb.event_count, 9);

        // Per-harness rows sum back to the global totals.
        let summed_sessions: u32 = pb.by_harness.iter().map(|h| h.sessions).sum();
        let summed_events: u64 = pb.by_harness.iter().map(|h| h.events).sum();
        assert_eq!(summed_sessions, pb.session_count);
        assert_eq!(summed_events, pb.event_count);

        // Two harnesses, keyed by the snake_case as_str() value.
        let claude = pb
            .by_harness
            .iter()
            .find(|h| h.harness == "claude_code")
            .expect("claude_code rollup present");
        assert_eq!(claude.sessions, 2);
        assert_eq!(claude.events, 5);
        let codex = pb
            .by_harness
            .iter()
            .find(|h| h.harness == "codex")
            .expect("codex rollup present");
        assert_eq!(codex.sessions, 1);
        assert_eq!(codex.events, 4);
    }

    #[test]
    fn finding_by_id_round_trips_and_resolves_session_harness() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 1);
        let finding = Finding {
            id: "f1".into(),
            pattern_id: "CONTEXT_BLOAT".into(),
            title: "Context bloat".into(),
            severity: 4,
            frequency: 0.5,
            est_cost_tokens: 10,
            est_cost_minutes: 5,
            confidence: 0.8,
            rationale: "r".into(),
            evidence: vec![EvidenceRef {
                session_id: "c1".into(),
                turn_id: None,
                event_id: None,
                quote: Some("q".into()),
                source_path: None,
            }],
            status: "confirmed".into(),
            verifier_verdict: Some("v".into()),
        };
        store.save_findings(&[finding.clone()]).unwrap();

        let got = store.finding_by_id("f1").unwrap().expect("finding present");
        assert_eq!(got.pattern_id, "CONTEXT_BLOAT");
        assert_eq!(got.evidence.len(), 1);
        assert_eq!(got.evidence[0].session_id, "c1");

        // The session referenced by the finding's evidence resolves to its harness.
        assert_eq!(
            store.session_harness("c1").unwrap().as_deref(),
            Some("claude_code")
        );
        assert_eq!(store.session_harness("missing").unwrap(), None);
        assert!(store.finding_by_id("missing").unwrap().is_none());
    }

    #[test]
    fn event_text_resolves_quote_and_source_path_for_evidence_fallback() {
        let store = Store::memory().unwrap();
        // seed_session writes UserPrompt events whose text is "prompt {i}" and a
        // raw_ref source_path of /tmp/{id}.jsonl — the ground truth the null-quote
        // drill-down must recover.
        seed_session(&store, "c1", Harness::ClaudeCode, 2);

        let (quote, source_path) = store
            .event_text("c1", "c1-e1")
            .unwrap()
            .expect("event present");
        assert_eq!(quote, "prompt 1");
        assert_eq!(
            source_path.as_deref(),
            Some(std::path::Path::new("/tmp/c1.jsonl"))
        );

        // Unknown event id → None (caller keeps the honest placeholder).
        assert!(store.event_text("c1", "c1-e99").unwrap().is_none());
        // Right event id under the wrong session must not leak across sessions.
        assert!(store.event_text("other", "c1-e1").unwrap().is_none());
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
            got.meta.get("thread_source").and_then(serde_json::Value::as_str),
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
        let source_path = PathBuf::from(
            "/tmp/root-session/subagents/workflows/wf_123/agent-child123.jsonl",
        );
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
        assert_eq!(store.parent_of("child").unwrap(), Some("parent".to_string()));
        assert_eq!(store.parent_of("parent").unwrap(), None);
    }

    // ── M4 Forge artifact CRUD + migration ────────────────────────────────────

    fn pending_artifact(id: &str, finding_id: Option<&str>) -> Artifact {
        Artifact {
            id: id.into(),
            finding_id: finding_id.map(Into::into),
            kind: "claude_md_guardrail".into(),
            target_path: "/tmp/warden-forge/CLAUDE.md".into(),
            diff: "--- a\n+++ b\n@@\n+## WARDEN guardrail — X\n".into(),
            block: "\n## WARDEN guardrail — X\n- rule\n".into(),
            status: "pending".into(),
            applied_at: None,
            backup_path: None,
            pre_image_sha256: None,
            post_image_sha256: None,
        }
    }

    #[test]
    fn artifact_round_trips_and_status_transitions_persist() {
        let store = Store::memory().unwrap();
        let a = pending_artifact("art-1", Some("find-1"));
        store.save_artifact(&a).unwrap();

        // get reflects the PENDING row exactly.
        let got = store.artifact_by_id("art-1").unwrap().expect("row present");
        assert_eq!(got.status, "pending");
        assert_eq!(got.finding_id.as_deref(), Some("find-1"));
        assert_eq!(got.block, a.block);
        assert!(got.applied_at.is_none() && got.backup_path.is_none());

        // list (all + by finding).
        assert_eq!(store.all_artifacts().unwrap().len(), 1);
        assert_eq!(store.artifacts_for_finding("find-1").unwrap().len(), 1);
        assert!(store.artifacts_for_finding("nope").unwrap().is_empty());

        // pending → applied records applied_at/backup/pre-image.
        store
            .update_artifact_status(
                "art-1",
                "applied",
                Some("2026-06-25T08:00:00Z"),
                Some("/tmp/warden-forge/.warden-bak/art-1.bak"),
                Some("deadbeef"),
                Some("cafef00d"),
            )
            .unwrap();
        let applied = store.artifact_by_id("art-1").unwrap().unwrap();
        assert_eq!(applied.status, "applied");
        assert_eq!(applied.applied_at.as_deref(), Some("2026-06-25T08:00:00Z"));
        assert_eq!(
            applied.backup_path.as_deref(),
            Some("/tmp/warden-forge/.warden-bak/art-1.bak")
        );
        assert_eq!(applied.pre_image_sha256.as_deref(), Some("deadbeef"));
        assert_eq!(applied.post_image_sha256.as_deref(), Some("cafef00d"));

        // applied → reverted keeps backup_path for the audit trail.
        store
            .update_artifact_status(
                "art-1",
                "reverted",
                Some("2026-06-25T08:00:00Z"),
                Some("/tmp/warden-forge/.warden-bak/art-1.bak"),
                Some("deadbeef"),
                Some("cafef00d"),
            )
            .unwrap();
        let reverted = store.artifact_by_id("art-1").unwrap().unwrap();
        assert_eq!(reverted.status, "reverted");
        assert!(reverted.backup_path.is_some());
    }

    #[test]
    fn artifacts_for_finding_filters_by_finding_id() {
        let store = Store::memory().unwrap();
        store.save_artifact(&pending_artifact("a1", Some("f1"))).unwrap();
        store.save_artifact(&pending_artifact("a2", Some("f1"))).unwrap();
        store.save_artifact(&pending_artifact("a3", Some("f2"))).unwrap();
        store.save_artifact(&pending_artifact("a4", None)).unwrap();

        assert_eq!(store.artifacts_for_finding("f1").unwrap().len(), 2);
        assert_eq!(store.artifacts_for_finding("f2").unwrap().len(), 1);
        assert_eq!(store.all_artifacts().unwrap().len(), 4);
    }

    #[test]
    fn artifact_by_id_absent_returns_none() {
        let store = Store::memory().unwrap();
        assert!(store.artifact_by_id("ghost").unwrap().is_none());
    }

    #[test]
    fn migrate_adds_artifact_columns_on_fresh_db() {
        // A fresh in-memory store already ran migrate(); the new columns must be
        // usable, which save_artifact (writing block + pre_image_sha256) proves.
        let store = Store::memory().unwrap();
        store.save_artifact(&pending_artifact("fresh", None)).unwrap();
        let got = store.artifact_by_id("fresh").unwrap().unwrap();
        assert_eq!(got.block, pending_artifact("fresh", None).block);
    }

    #[test]
    fn migrate_adds_artifact_columns_on_preexisting_db() {
        // Build a DB with the legacy 8-column `artifacts` table (no block /
        // pre_image_sha256), close it, then open via Store::open so migrate()
        // ALTERs in the two M4 columns idempotently. A round-trip then proves the
        // columns exist and old rows survive with safe defaults.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy.db");
        {
            let c = Connection::open(&db_path).unwrap();
            c.execute_batch(
                "CREATE TABLE artifacts(id TEXT PRIMARY KEY,finding_id TEXT,kind TEXT NOT NULL,target_path TEXT NOT NULL,diff TEXT NOT NULL,status TEXT NOT NULL,applied_at TEXT,backup_path TEXT,created_at TEXT);",
            )
            .unwrap();
            c.execute(
                "INSERT INTO artifacts(id,finding_id,kind,target_path,diff,status,created_at) VALUES('old',NULL,'claude_md_guardrail','/tmp/x','d','pending','2026-06-25T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        // Open through Store, which runs migrate() and must add the two columns.
        let store = Store::open(&db_path).unwrap();
        // Legacy row survives; block defaults to '' (NOT NULL DEFAULT '').
        let old = store.artifact_by_id("old").unwrap().expect("legacy row");
        assert_eq!(old.status, "pending");
        assert_eq!(old.block, "");
        assert!(old.pre_image_sha256.is_none());
        assert!(old.post_image_sha256.is_none());
        // New writes use the new columns.
        store.save_artifact(&pending_artifact("new", Some("f"))).unwrap();
        let new = store.artifact_by_id("new").unwrap().unwrap();
        assert!(!new.block.is_empty());
    }
}
