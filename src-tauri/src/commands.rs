use crate::brain::Brain;
use crate::featurizer;
use crate::forge::{self, FixPreview};
use crate::harness_theme::harness_theme;
use crate::ingest::claude_code;
use crate::ir::*;
use crate::radar::{self, RadarAgent, RadarState};
use crate::scaffold::not_in_slice;
use crate::store::{ProfileBreakdown, Store};
use crate::util::{default_claude_sessions_dir, default_db_path};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tauri::Emitter;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub run_lock: Arc<Mutex<()>>,
}
impl AppState {
    pub fn init() -> Result<Self> {
        let store = Store::open(default_db_path())?;
        Ok(Self {
            store,
            run_lock: Arc::new(Mutex::new(())),
        })
    }
}

/// TEMP diagnostic sink: the packaged overlay is an accessory window with no
/// devtools, so live-overlay JS reports where the R3F island breaks via this →
/// stderr. Remove once the war-room render path is fixed.
#[tauri::command]
pub fn diag(msg: String) {
    eprintln!("[WARDEN-DIAG] {msg}");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrbAgent {
    pub id: String,
    pub harness: String,
    pub label: String,
    pub glyph: String,
    pub color: String,
    pub sessions: u32,
    pub event_count: u64,
    pub total_load: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrbIssue {
    pub id: String,
    pub agent_id: String,
    pub harness: String,
    pub pattern_id: String,
    pub title: String,
    pub count: u32,
    pub severity: u8,
    pub rationale: String,
    pub est_cost_tokens: u64,
    pub est_cost_minutes: u64,
    pub frequency: f64,
    pub confidence: f64,
    pub session_ids: Vec<String>,
    pub evidence: Vec<EvidenceRef>,
    pub finding_id: Option<String>,
    pub verifier_verdict: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrbLink {
    pub source: String,
    pub target: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrbGuidance {
    pub do_items: Vec<String>,
    pub stop_items: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrbScenePayload {
    pub agents: Vec<OrbAgent>,
    pub issues: Vec<OrbIssue>,
    pub links: Vec<OrbLink>,
    pub guidance: OrbGuidance,
}

fn harness_from_str(h: &str) -> Harness {
    match h {
        "claude_code" => Harness::ClaudeCode,
        "codex" => Harness::Codex,
        "cursor" => Harness::Cursor,
        "hermes" => Harness::Hermes,
        other => Harness::Generic(other.to_string()),
    }
}

fn agent_for_harness(harness: &str, sessions: u32, event_count: u64) -> OrbAgent {
    if harness == "unknown" {
        return OrbAgent {
            id: "unknown".into(),
            harness: "unknown".into(),
            label: "Unknown".into(),
            glyph: "●".into(),
            color: "#76ff9d".into(),
            sessions,
            event_count,
            total_load: 0,
        };
    }
    let h = harness_from_str(harness);
    let t = harness_theme(&h);
    OrbAgent {
        id: harness.into(),
        harness: harness.into(),
        label: t.label.into(),
        glyph: t.glyph.into(),
        color: t.color.into(),
        sessions,
        event_count,
        total_load: 0,
    }
}

fn session_ids_for(features: &[FeatureVector]) -> Vec<String> {
    let mut out = Vec::new();
    for f in features {
        if !out.contains(&f.session_id) {
            out.push(f.session_id.clone());
        }
    }
    out
}

fn finding_harness(f: &Finding, sessions_by_id: &HashMap<String, Session>) -> String {
    f.evidence
        .first()
        .and_then(|e| sessions_by_id.get(&e.session_id))
        .map(|s| s.harness.as_str().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub(crate) fn build_orb_scene(store: &Store) -> Result<OrbScenePayload> {
    let profile = store.profile()?;
    let features = store.all_features()?;
    let sessions = store.sessions()?;
    if features.is_empty() && sessions.is_empty() {
        return Ok(OrbScenePayload::default());
    }

    let sessions_by_id = sessions
        .iter()
        .cloned()
        .map(|s| (s.id.clone(), s))
        .collect::<HashMap<_, _>>();
    let mut agents = store
        .profile_with_harness_breakdown()?
        .by_harness
        .into_iter()
        .map(|r| {
            (
                r.harness.clone(),
                agent_for_harness(&r.harness, r.sessions, r.events),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let persisted = store.all_findings()?;
    let mut persisted_by_key = HashMap::new();
    for f in persisted {
        persisted_by_key
            .entry((finding_harness(&f, &sessions_by_id), f.pattern_id.clone()))
            .or_insert(f);
    }

    let mut issues = Vec::new();
    let mut links = Vec::new();

    for hit in crate::detectors::detect(&profile, &features) {
        let mut by_harness: BTreeMap<String, Vec<FeatureVector>> = BTreeMap::new();
        for feature in &hit.affected {
            let harness = sessions_by_id
                .get(&feature.session_id)
                .map(|s| s.harness.as_str().to_string())
                .unwrap_or_else(|| "unknown".into());
            by_harness.entry(harness).or_default().push(feature.clone());
        }

        for (harness, affected) in by_harness {
            let session_ids = session_ids_for(&affected);
            let issue_id = format!("{harness}:{}", hit.pattern_id);
            let finding = crate::detectors::finding_from_hit(
                store,
                &sessions_by_id,
                &hit,
                &affected,
                features.len().max(1),
            );
            let persisted = persisted_by_key.get(&(harness.clone(), hit.pattern_id.to_string()));
            let count = affected.len() as u32;
            let agent = agents
                .entry(harness.clone())
                .or_insert_with(|| agent_for_harness(&harness, session_ids.len() as u32, 0));
            agent.sessions = agent.sessions.max(session_ids.len() as u32);
            agent.total_load = agent.total_load.saturating_add(count);

            links.push(OrbLink {
                source: harness.clone(),
                target: issue_id.clone(),
                kind: "agent_issue".into(),
            });
            issues.push(OrbIssue {
                id: issue_id,
                agent_id: harness.clone(),
                harness,
                pattern_id: hit.pattern_id.into(),
                title: hit.title.into(),
                count,
                severity: hit.severity,
                rationale: finding.rationale,
                est_cost_tokens: finding.est_cost_tokens,
                est_cost_minutes: finding.est_cost_minutes,
                frequency: finding.frequency,
                confidence: finding.confidence,
                session_ids,
                evidence: finding.evidence,
                finding_id: persisted.map(|f| f.id.clone()),
                verifier_verdict: persisted.and_then(|f| f.verifier_verdict.clone()),
                status: persisted.map(|f| f.status.clone()),
            });
        }
    }

    let guidance = store
        .latest_diagnosis()?
        .map_or_else(OrbGuidance::default, |d| OrbGuidance {
            do_items: d.do_items,
            stop_items: d.stop_items,
        });

    Ok(OrbScenePayload {
        agents: agents.into_values().collect(),
        issues,
        links,
        guidance,
    })
}

#[tauri::command]
pub async fn query_profile(state: tauri::State<'_, AppState>) -> Result<ProfileBreakdown, String> {
    state
        .store
        .profile_with_harness_breakdown()
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn get_diagnosis(state: tauri::State<'_, AppState>) -> Result<Option<Diagnosis>, String> {
    state.store.latest_diagnosis().map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn get_findings(state: tauri::State<'_, AppState>) -> Result<Vec<Finding>, String> {
    let p = state.store.profile().map_err(|e| e.to_string())?;
    crate::detectors::nominate(&state.store, &p).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn get_orb_scene(state: tauri::State<'_, AppState>) -> Result<OrbScenePayload, String> {
    build_orb_scene(&state.store).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn run_diagnosis(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: RunScope,
) -> Result<Diagnosis, String> {
    let _guard = state.run_lock.lock().await;
    app.emit(
        "ingest_progress",
        serde_json::json!({"phase":"ingest","status":"started"}),
    )
    .ok();
    let (ingested_sessions, ingested_events) =
        claude_code::ingest_all(&state.store, None, scope.max_files).map_err(|e| e.to_string())?;
    let (total_sessions, total_events, finding_count) =
        state.store.counts().map_err(|e| e.to_string())?;
    app.emit(
        "ingest_progress",
        serde_json::json!({
            "phase":"ingest",
            "status":"complete",
            "ingested_sessions":ingested_sessions,
            "ingested_events":ingested_events,
            "total_sessions":total_sessions,
            "total_events":total_events,
            "finding_count":finding_count
        }),
    )
    .ok();
    app.emit(
        "ingest_progress",
        serde_json::json!({"phase":"featurize","status":"started"}),
    )
    .ok();
    let profile = featurizer::compute_all(&state.store).map_err(|e| e.to_string())?;
    app.emit(
        "ingest_progress",
        serde_json::json!({
            "phase":"featurize",
            "status":"complete",
            "session_count":profile.session_count,
            "event_count":profile.event_count
        }),
    )
    .ok();
    app.emit(
        "diagnosis_status",
        serde_json::json!({"phase":"brain","status":"started"}),
    )
    .ok();
    let diagnosis = Brain::new(state.store.clone())
        .with_app(app.clone())
        .run_pipeline(scope)
        .await
        .map_err(|e| e.to_string())?;
    app.emit(
        "diagnosis_ready",
        serde_json::json!({"id":diagnosis.id,"finding_count":diagnosis.ranked_findings.len()}),
    )
    .ok();
    Ok(diagnosis)
}
#[tauri::command]
pub async fn ask(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    query: String,
    mode: Option<String>,
) -> Result<Diagnosis, String> {
    run_diagnosis(
        app,
        state,
        RunScope {
            harness: Some("claude_code".into()),
            query: Some(query),
            force: Some(mode.as_deref() == Some("force")),
            max_files: None,
        },
    )
    .await
}
/// Dismiss the overlay (frontend Esc handler). Hides the pre-warmed `overlay`
/// window and restores click-through so the desktop is interactive while idle —
/// the same end state as the `tauri://blur` handler. Best-effort: a missing
/// window or a platform that rejects ignore-cursor-events must not error the
/// frontend, so failures are swallowed.
#[tauri::command]
pub async fn hide_overlay(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
        let _ = w.set_ignore_cursor_events(true);
    }
    Ok(())
}
#[tauri::command]
pub async fn apply_artifact(_id: String) -> Result<(), String> {
    Err(not_in_slice("Forge artifact apply"))
}
#[tauri::command]
pub async fn revert_artifact(_id: String) -> Result<(), String> {
    Err(not_in_slice("Forge artifact revert"))
}
#[tauri::command]
pub async fn start_voice() -> Result<(), String> {
    Err(not_in_slice("Voice STT"))
}
#[tauri::command]
pub async fn stop_voice() -> Result<(), String> {
    Err(not_in_slice("Voice STT"))
}
#[tauri::command]
pub async fn capture_screen() -> Result<(), String> {
    Err(not_in_slice("Screen Q&A"))
}
/// Persist a partial config patch to `~/.warden/config.toml`. Forward-only and
/// merge-based: only keys present in `value` are written (see `config::save`).
#[tauri::command]
pub async fn set_config(value: serde_json::Value) -> Result<(), String> {
    crate::config::save(value).map_err(|e| e.to_string())
}

/// Read-only fix preview for a saved finding: returns a unified diff against the
/// real target file with `applied: false`. Never writes (apply is the M4 slice).
#[tauri::command]
pub async fn get_fix_preview(
    state: tauri::State<'_, AppState>,
    finding_id: String,
) -> Result<FixPreview, String> {
    forge::fix_preview(&state.store, &finding_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_orb_fix_preview(
    state: tauri::State<'_, AppState>,
    issue_id: String,
) -> Result<FixPreview, String> {
    let scene = build_orb_scene(&state.store).map_err(|e| e.to_string())?;
    let issue = scene
        .issues
        .into_iter()
        .find(|i| i.id == issue_id)
        .ok_or_else(|| format!("orb issue {issue_id} not found"))?;
    let finding = Finding {
        id: issue.id,
        pattern_id: issue.pattern_id,
        title: issue.title,
        severity: issue.severity,
        frequency: issue.frequency,
        est_cost_tokens: issue.est_cost_tokens,
        est_cost_minutes: issue.est_cost_minutes,
        confidence: issue.confidence,
        rationale: issue.rationale,
        evidence: issue.evidence,
        status: issue.status.unwrap_or_else(|| "candidate".into()),
        verifier_verdict: issue.verifier_verdict,
    };
    Ok(forge::preview_for_finding(&finding))
}

/// Recovered ground truth for an `EvidenceRef` whose `quote` the Fugu pipeline
/// left null (`brain.rs` maps an `Option<String>` the model may omit). Both
/// fields are optional: the drill-down only swaps in `quote` when it is present,
/// otherwise it keeps the honest "no excerpt stored" placeholder.
#[derive(serde::Serialize)]
pub struct ResolvedEvidence {
    pub quote: Option<String>,
    pub source_path: Option<String>,
}

/// READ-ONLY evidence resolver (Task 9 fallback). When a finding's `EvidenceRef`
/// carries an `event_id` but no stored `quote`, the diagnosis screen calls this
/// on expand to recover the excerpt from the underlying event — preserving the
/// spec's "every claim traceable to ground truth" guarantee without ever
/// fabricating text. Reads one `events` row (`store::event_text`), truncates the
/// text to ~220 chars through the same redacting `excerpt` the detectors use, and
/// returns it with the raw source path. Never writes. `quote` is `None` when the
/// event is missing or its text is empty, so the UI degrades gracefully.
#[tauri::command]
pub async fn resolve_evidence(
    state: tauri::State<'_, AppState>,
    session_id: String,
    event_id: String,
) -> Result<ResolvedEvidence, String> {
    let resolved = state
        .store
        .event_text(&session_id, &event_id)
        .map_err(|e| e.to_string())?;
    let (quote, source_path) = match resolved {
        Some((text, source_path)) => {
            let trimmed = text.trim();
            let quote = if trimmed.is_empty() {
                None
            } else {
                Some(crate::redaction::excerpt(trimmed, 220))
            };
            (quote, source_path.map(|p| p.to_string_lossy().into_owned()))
        }
        None => (None, None),
    };
    Ok(ResolvedEvidence { quote, source_path })
}
#[tauri::command]
pub async fn mute_pattern(_id: String) -> Result<(), String> {
    Err(not_in_slice("Live interjection muting"))
}
/// RADAR (Task 9): the live agent forest, contract-shaped (`radar_state`). Pure
/// read — recomputes from the store + Claude liveness registry on demand. The
/// watcher also pushes this as an event; this command serves the initial pull.
#[tauri::command]
pub async fn get_radar_state(state: tauri::State<'_, AppState>) -> Result<RadarState, String> {
    Ok(radar::recompute_radar_state(
        &state.store,
        &default_claude_sessions_dir(),
    ))
}

/// RADAR: the flat list of live agents (the forest's `agents`). Shares the
/// `assemble` path with [`get_radar_state`]; kept for the fleet roster surface.
#[tauri::command]
pub async fn list_fleet(state: tauri::State<'_, AppState>) -> Result<Vec<RadarAgent>, String> {
    Ok(radar::recompute_radar_state(&state.store, &default_claude_sessions_dir()).agents)
}
#[tauri::command]
pub async fn locate_agent(_id: String) -> Result<(), String> {
    Err(not_in_slice("RADAR locate"))
}
#[tauri::command]
pub async fn warp_to_agent(_id: String) -> Result<(), String> {
    Err(not_in_slice("RADAR warp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;

    fn seed_session(store: &Store, id: &str, harness: Harness, events: usize) {
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
            meta: json!({}),
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
        let records = (0..events)
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
            .upsert_session_batch(&session, &[turn], &records, 0)
            .unwrap();
    }

    fn context_feature(session_id: &str) -> FeatureVector {
        FeatureVector {
            session_id: session_id.into(),
            search_in_main_context: 8,
            context_saturation_peak: 0.5,
            tool_call_count: 3,
            verification_present: true,
            token_burn_total: 1_000,
            ..FeatureVector::default()
        }
    }

    #[test]
    fn orb_scene_groups_same_pattern_by_harness_without_merging() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 2);
        seed_session(&store, "c2", Harness::ClaudeCode, 1);
        seed_session(&store, "x1", Harness::Codex, 3);
        store.save_feature(&context_feature("c1"), "test").unwrap();
        store.save_feature(&context_feature("c2"), "test").unwrap();
        store.save_feature(&context_feature("x1"), "test").unwrap();
        store
            .save_profile(&CompetenceProfile {
                session_count: 3,
                event_count: 6,
                finding_count: 0,
                ..CompetenceProfile::default()
            })
            .unwrap();

        let scene = build_orb_scene(&store).unwrap();
        let claude = scene
            .issues
            .iter()
            .find(|i| i.id == "claude_code:CONTEXT_BLOAT")
            .expect("claude context issue");
        let codex = scene
            .issues
            .iter()
            .find(|i| i.id == "codex:CONTEXT_BLOAT")
            .expect("codex context issue");

        assert_eq!(claude.count, 2);
        assert_eq!(codex.count, 1);
        assert_eq!(claude.session_ids, vec!["c1", "c2"]);
        assert_eq!(codex.session_ids, vec!["x1"]);
        assert!(scene
            .links
            .iter()
            .all(|link| link.source == link.target.split(':').next().unwrap()));
    }

    #[test]
    fn orb_scene_hub_load_is_sum_of_issue_counts_and_clean_agents_stay_clean() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 1);
        seed_session(&store, "x1", Harness::Codex, 1);
        store.save_feature(&context_feature("c1"), "test").unwrap();
        store
            .save_profile(&CompetenceProfile {
                session_count: 2,
                event_count: 2,
                finding_count: 0,
                ..CompetenceProfile::default()
            })
            .unwrap();

        let scene = build_orb_scene(&store).unwrap();
        let claude = scene.agents.iter().find(|a| a.id == "claude_code").unwrap();
        let codex = scene.agents.iter().find(|a| a.id == "codex").unwrap();

        assert_eq!(claude.total_load, 1);
        assert_eq!(codex.total_load, 0);
        assert!(scene.issues.iter().all(|i| i.agent_id != "codex"));
    }

    #[test]
    fn orb_scene_missing_session_harness_degrades_to_unknown() {
        let store = Store::memory().unwrap();
        seed_session(&store, "u1", Harness::Generic("unknown".into()), 0);
        store.save_feature(&context_feature("u1"), "test").unwrap();
        store
            .save_profile(&CompetenceProfile {
                session_count: 1,
                event_count: 0,
                finding_count: 0,
                ..CompetenceProfile::default()
            })
            .unwrap();

        let scene = build_orb_scene(&store).unwrap();
        let agent = scene.agents.iter().find(|a| a.id == "unknown").unwrap();
        let issue = scene
            .issues
            .iter()
            .find(|i| i.id == "unknown:CONTEXT_BLOAT")
            .unwrap();

        assert_eq!(agent.total_load, 1);
        assert_eq!(issue.harness, "unknown");
        assert_eq!(issue.count, 1);
    }

    #[test]
    fn orb_scene_empty_profile_returns_empty_map() {
        let store = Store::memory().unwrap();
        let scene = build_orb_scene(&store).unwrap();
        assert!(scene.agents.is_empty());
        assert!(scene.issues.is_empty());
        assert!(scene.links.is_empty());
    }

    /// Smoke test for the RADAR command path: the `get_radar_state`/`list_fleet`
    /// commands both delegate to `radar::recompute_radar_state`; with a seeded
    /// session that path returns a contract-shaped forest (one agent, serialized
    /// camelCase). Exercising the shared core avoids constructing a Tauri `State`.
    #[test]
    fn radar_command_core_returns_contract_shaped_forest() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 1);

        // No registry dir present in tests → mtime/idle fallback; uses real clock.
        let state = crate::radar::recompute_radar_state(
            &store,
            std::path::Path::new("/no/such/registry"),
        );
        assert_eq!(state.agents.len(), 1, "one seeded session → one agent");
        let agent = &state.agents[0];
        assert_eq!(agent.id, "c1");
        assert_eq!(agent.harness, "claude_code");
        assert_eq!(agent.depth, 0);
        assert_eq!(agent.parent_id, None);

        let json = serde_json::to_string(&state).unwrap();
        for key in ["\"generatedAt\"", "\"fillPct\"", "\"contextTokens\"", "\"childCount\""] {
            assert!(json.contains(key), "contract camelCase key {key} present");
        }
    }
}
