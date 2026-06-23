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
        CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY,finding_id TEXT,kind TEXT NOT NULL,target_path TEXT NOT NULL,diff TEXT NOT NULL,status TEXT NOT NULL,applied_at TEXT,backup_path TEXT);
        CREATE TABLE IF NOT EXISTS fugu_runs(id TEXT PRIMARY KEY,stage TEXT NOT NULL,model TEXT NOT NULL,effort TEXT NOT NULL,req_hash TEXT NOT NULL,input_tokens INTEGER NOT NULL,output_tokens INTEGER NOT NULL,orchestration_input_tokens INTEGER NOT NULL,orchestration_output_tokens INTEGER NOT NULL,latency_ms INTEGER NOT NULL,cost_usd REAL NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS interjections(id TEXT PRIMARY KEY,ts TEXT NOT NULL,pattern_id TEXT NOT NULL,session_id TEXT,shown INTEGER NOT NULL,dismissed INTEGER NOT NULL,muted INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_events_session_kind ON events(session_id, kind);
        CREATE INDEX IF NOT EXISTS idx_turns_session_idx ON turns(session_id, idx);
        INSERT OR REPLACE INTO schema_meta(key,value) VALUES('schema_version','1');
        "#)?;
        Ok(())
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
        tx.execute(
            "INSERT INTO sessions(id,harness,external_id,project_json,model_ids_json,started_at,ended_at,source_path,raw_hash,ingested_at,meta_json) VALUES(?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET \
               project_json=CASE WHEN excluded.project_json IN ('null','') THEN sessions.project_json ELSE excluded.project_json END, \
               model_ids_json=CASE WHEN excluded.model_ids_json IN ('[]','') THEN sessions.model_ids_json ELSE excluded.model_ids_json END, \
               started_at=MIN(sessions.started_at, excluded.started_at), \
               ended_at=MAX(COALESCE(sessions.ended_at, excluded.ended_at), COALESCE(excluded.ended_at, sessions.ended_at)), \
               raw_hash=excluded.raw_hash, \
               ingested_at=excluded.ingested_at, \
               meta_json=CASE WHEN excluded.meta_json IN ('{\"ignored_record_types\":{}}','{}','') THEN sessions.meta_json ELSE excluded.meta_json END",
            params![session.id, session.harness.as_str(), session.external_id, serde_json::to_string(&session.project)?, serde_json::to_string(&session.model_ids)?, session.started_at.to_rfc3339(), session.ended_at.map(|d| d.to_rfc3339()), session.source_path.to_string_lossy(), session.raw_hash as i64, session.ingested_at.to_rfc3339(), serde_json::to_string(&session.meta)?],
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
        self.conn()
            .query_row(
                "SELECT raw_hash FROM sessions WHERE source_path=? LIMIT 1",
                [path.to_string_lossy().to_string()],
                |r| Ok(r.get::<_, i64>(0)? as u64),
            )
            .optional()
            .map_err(Into::into)
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
}
