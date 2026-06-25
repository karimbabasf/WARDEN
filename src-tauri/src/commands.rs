use crate::brain::Brain;
use crate::featurizer;
use crate::forge::{self, FixPreview};
use crate::harness_theme::harness_theme;
use crate::ingest::claude_code;
use crate::ir::*;
use crate::radar::{self, RadarAgent, RadarState};
use crate::scaffold::not_in_slice;
use crate::store::{ProfileBreakdown, Store};
use crate::util::{backup_dir, default_claude_sessions_dir, default_db_path};
use chrono::Utc;
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
    /// Serializes ALL forge target mutations (stage→apply→revert). Tauri dispatches
    /// commands on a worker-thread pool, so two `apply_artifact` invokes for two
    /// findings that resolve to the SAME target file (e.g. `~/.claude/CLAUDE.md`)
    /// could otherwise interleave their read→backup→write and lose one block (the
    /// second rename wins, the first guardrail vanishes). Holding this lock across
    /// the whole apply/revert body closes that lost-update window. Distinct from
    /// `run_lock` (which only guards `run_diagnosis`).
    pub forge_lock: Arc<Mutex<()>>,
    pub radar_state: crate::scheduler::RadarStateCache,
}
impl AppState {
    pub fn init() -> Result<Self> {
        let store = Store::open(default_db_path())?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: Store) -> Self {
        Self {
            store,
            run_lock: Arc::new(Mutex::new(())),
            forge_lock: Arc::new(Mutex::new(())),
            radar_state: crate::scheduler::new_radar_state_cache(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_store_for_test(store: Store) -> Self {
        Self::from_store(store)
    }

    pub fn cache_radar_state(&self, radar: RadarState) {
        crate::scheduler::cache_radar_state(&self.radar_state, radar);
    }

    pub fn cached_radar_state(&self) -> Option<RadarState> {
        crate::scheduler::latest_cached_radar_state(&self.radar_state)
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
/// Hide the overlay window. The daemon keeps running and the window is
/// re-summonable via ⌘⌥⌃M or the tray menu. Best-effort: a missing
/// window must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn hide_overlay(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
    Ok(())
}
/// Minimize the persistent overlay window to the Dock (macOS). Best-effort: a
/// missing window must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn minimize_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.minimize();
    }
    Ok(())
}
/// Hide the persistent overlay window. The daemon keeps running and the window
/// stays re-summonable via the tray menu or the ⌘⌥⌃M hotkey. Best-effort: a
/// missing window must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn hide_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
    Ok(())
}
/// M4 Forge: resolve the `Finding` a stage request refers to — either a saved
/// finding (by `finding_id`) or a live orb issue (by `issue_id`, mirroring
/// `get_orb_fix_preview`). Returns the finding plus the id we key the artifact on
/// (the saved finding id, or the orb issue id as the stable handle).
fn resolve_finding_for_stage(
    store: &Store,
    finding_id: Option<&str>,
    issue_id: Option<&str>,
) -> Result<(Finding, String)> {
    if let Some(fid) = finding_id {
        let finding = store
            .finding_by_id(fid)?
            .ok_or_else(|| anyhow::anyhow!("finding {fid} not found"))?;
        return Ok((finding, fid.to_string()));
    }
    if let Some(iid) = issue_id {
        let scene = build_orb_scene(store)?;
        let issue = scene
            .issues
            .into_iter()
            .find(|i| i.id == iid)
            .ok_or_else(|| anyhow::anyhow!("orb issue {iid} not found"))?;
        let finding = Finding {
            id: issue.id.clone(),
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
        return Ok((finding, issue.id));
    }
    Err(anyhow::anyhow!(
        "stage_artifact requires either findingId or issueId"
    ))
}

/// M4 Forge: stage a PENDING artifact from a fix preview. Resolves the finding
/// (saved or orb-issue), captures the target path + display diff + literal block,
/// persists one `artifacts` row, and returns it. Idempotent on `(finding_id,
/// target_path)`: re-staging while a non-applied artifact already exists returns
/// that existing row rather than creating a duplicate, so a double-click on APPLY
/// can't accumulate rows.
#[tauri::command]
pub async fn stage_artifact(
    state: tauri::State<'_, AppState>,
    finding_id: Option<String>,
    issue_id: Option<String>,
) -> Result<Artifact, String> {
    // Serialize with apply/revert: staging reads the target to build the diff, and a
    // concurrent apply must not interleave with a stage on the same file.
    let _guard = state.forge_lock.lock().await;
    stage_artifact_inner(&state.store, finding_id.as_deref(), issue_id.as_deref())
        .map_err(|e| e.to_string())
}

/// Pure core of `stage_artifact`, isolated from `tauri::State` so it is directly
/// unit-testable against a temp `Store`.
fn stage_artifact_inner(
    store: &Store,
    finding_id: Option<&str>,
    issue_id: Option<&str>,
) -> Result<Artifact> {
    let (finding, key_id) = resolve_finding_for_stage(store, finding_id, issue_id)?;
    let preview = forge::preview_for_finding(&finding);
    let block = forge::block_for_finding(&finding);

    // Re-staging idempotency: if a non-applied artifact already exists for this
    // key + target, return it instead of inserting a duplicate.
    if let Some(existing) = store
        .artifacts_for_finding(&key_id)?
        .into_iter()
        .find(|a| a.target_path == preview.target_path && a.status != "reverted")
    {
        return Ok(existing);
    }

    let nanos = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .to_string();
    let id = crate::util::stable_id(&[
        &key_id,
        "claude_md_guardrail",
        &preview.target_path,
        &nanos,
    ]);
    let artifact = Artifact {
        id,
        finding_id: Some(key_id),
        kind: "claude_md_guardrail".into(),
        target_path: preview.target_path,
        diff: preview.diff,
        block,
        status: "pending".into(),
        applied_at: None,
        backup_path: None,
        pre_image_sha256: None,
        post_image_sha256: None,
    };
    store.save_artifact(&artifact)?;
    Ok(artifact)
}

/// M4 Forge: apply a staged artifact — back up the target's pre-image and write
/// the guardrail block (idempotent no-op if already present). Returns the updated
/// APPLIED artifact so the FACE can flip the card from the returned status without
/// a second round-trip. Re-applying is harmless.
#[tauri::command]
pub async fn apply_artifact(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Artifact, String> {
    // Hold the forge lock across the ENTIRE read→backup→write so two applies to the
    // same target can't interleave and drop a block (lost update).
    let _guard = state.forge_lock.lock().await;
    apply_artifact_inner(&state.store, &id).map_err(|e| e.to_string())
}

fn apply_artifact_inner(store: &Store, id: &str) -> Result<Artifact> {
    let artifact = store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} not found"))?;
    let target = crate::util::expand_tilde(&artifact.target_path);
    let backup_path = backup_dir(&target).join(format!("{id}.bak"));

    let outcome = forge::apply_block(&target, &artifact.block, &backup_path)?;
    let applied_at = artifact.applied_at.clone().unwrap_or_else(|| Utc::now().to_rfc3339());
    // Decide which (backup_path, pre_image_sha256) to record:
    //  - A *changing* apply that wrote a NEW backup records that fresh backup +
    //    pre-image (the bytes that were on disk before this apply).
    //  - A *changing* apply that PRESERVED a pre-existing backup (re-apply after the
    //    user edited the file out-of-band) must keep the ORIGINAL pre-image sha — the
    //    backup on disk still holds the first pristine pre-image, not the current
    //    bytes — so revert restores the true baseline, not an intermediate edit.
    //  - A no-op apply (block already present) preserves whatever a prior changing
    //    apply recorded; clearing it would orphan the pre-image and break revert.
    //    When there was never a changing apply (out-of-band block), both stay null.
    let (backup_str, pre_sha) = if outcome.changed && outcome.backup_written {
        (
            outcome.backup_path.map(|p| p.to_string_lossy().into_owned()),
            Some(outcome.pre_image_sha256),
        )
    } else {
        (artifact.backup_path.clone(), artifact.pre_image_sha256.clone())
    };
    // Always record the post-image sha of what is now on disk so revert can detect
    // out-of-band drift since apply and refuse to clobber the user's later edits.
    let post_sha = Some(outcome.post_image_sha256);
    store.update_artifact_status(
        id,
        "applied",
        Some(&applied_at),
        backup_str.as_deref(),
        pre_sha.as_deref(),
        post_sha.as_deref(),
    )?;
    store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after apply"))
}

/// M4 Forge: revert an applied artifact — verify the backup's SHA against the
/// recorded pre-image and restore it (refuses on mismatch). A no-op Ok when the
/// artifact was never applied (nothing to restore), keeping the UI forgiving.
#[tauri::command]
pub async fn revert_artifact(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Artifact, String> {
    // Same serialization as apply: a revert racing an apply on the same target has
    // the same lost-update shape, so it shares the lock.
    let _guard = state.forge_lock.lock().await;
    revert_artifact_inner(&state.store, &id).map_err(|e| e.to_string())
}

fn revert_artifact_inner(store: &Store, id: &str) -> Result<Artifact> {
    let artifact = store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} not found"))?;

    // TERMINAL-STATE GUARD (data-loss fix): revert is only meaningful on an APPLIED
    // artifact. On a PENDING artifact there is nothing to restore, and on an already
    // REVERTED artifact the backup is INVALIDATED (deleted + backup_path/pre_image_sha256
    // nulled) at the end of the prior revert — so even if this guard were absent the
    // restore branch would now find no backup. Belt-and-suspenders: a second revert
    // must never re-enter the restore branch and atomic-write a stale pre-image over
    // whatever the user has since written. Spec §4: "revert on a PENDING/REVERTED
    // artifact → returns Ok no-op". Make it a true no-op: return the row untouched
    // without writing the target.
    if artifact.status != "applied" {
        return Ok(artifact);
    }

    // Never applied with a real write (a no-op apply with no backup) → nothing to
    // restore. Flip the status to reverted (re-stageable) and return — forgiving.
    let (Some(backup_path), Some(expected_sha)) =
        (&artifact.backup_path, &artifact.pre_image_sha256)
    else {
        store.update_artifact_status(
            id,
            "reverted",
            artifact.applied_at.as_deref(),
            None,
            None,
            artifact.post_image_sha256.as_deref(),
        )?;
        return store
            .artifact_by_id(id)?
            .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after revert"));
    };

    let target = crate::util::expand_tilde(&artifact.target_path);

    // OUT-OF-BAND DRIFT GUARD (data-loss fix): before restoring, prove the target
    // still holds exactly what WARDEN wrote at apply time. If the user (or another
    // tool) edited the file since apply, its current sha won't match the recorded
    // post-image — blindly restoring the pre-image would clobber those edits. Refuse
    // with a typed error and leave the file untouched (integrity over convenience).
    // Older rows applied before this column existed have no post-image sha; skip the
    // check for them (best-effort) rather than refusing every legacy revert.
    if let Some(expected_post) = artifact.post_image_sha256.as_deref() {
        // SAFE-REFUSAL (data-loss fix): read RAW BYTES, not
        // `read_to_string(...).unwrap_or_default()`. If the target now holds invalid
        // UTF-8 (or is unreadable), do NOT collapse it to "" — that would compute the
        // sha of empty against the expected post-image, almost certainly mismatch, and
        // (correctly, here) refuse; but on the off chance the recorded post-image were
        // empty it would FALSE-MATCH and let the restore clobber binary content. Treat
        // any non-UTF8 / unreadable current as an explicit out-of-band refusal. A
        // missing file hashes as "" (sha of empty), preserving prior behavior.
        let current = match std::fs::read(&target) {
            Ok(bytes) => match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "target {} is not valid UTF-8; refusing to revert to avoid \
                         clobbering non-text content",
                        target.display()
                    ))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(anyhow::anyhow!("read target {}: {e}", target.display()))
            }
        };
        let current_sha = crate::util::sha256_hex(current.as_bytes());
        if current_sha != expected_post {
            return Err(anyhow::anyhow!(
                "target {} changed out-of-band since apply (expected post-image sha \
                 {expected_post}, found {current_sha}); refusing to revert and \
                 overwrite those edits",
                target.display()
            ));
        }
    }

    let backup_file = crate::util::expand_tilde(&backup_path.clone());
    forge::revert_block(&target, &backup_file, expected_sha)?;

    // BACKUP INVALIDATION ON SUCCESSFUL REVERT (residual data-loss fix): a successful
    // revert ENDS this apply cycle, so the backup it just consumed is no longer the
    // pristine pre-image of any future apply. If we retained `{id}.bak` +
    // `backup_path`, a later RE-APPLY of this same id would see `backup_path.exists()`
    // and (via apply_block's backup_preexisted guard) PRESERVE the stale pre-FIRST-apply
    // backup instead of capturing the user's current (post-revert, possibly edited)
    // content as the new pre-image. A subsequent revert would then restore that stale
    // ORIGINAL and clobber the user's edits. So: delete the .bak (best-effort; tolerate
    // an already-absent file) and NULL backup_path + pre_image_sha256 on the row. The
    // next apply is then a FRESH cycle that captures a new pristine backup of CURRENT
    // content. The terminal-state guard (status != "applied" → no-op) still blocks a
    // double revert, and within an APPLIED cycle the pristine backup is still preserved
    // (this branch only fires once per applied→reverted transition).
    let _ = std::fs::remove_file(&backup_file);
    store.update_artifact_status(
        id,
        "reverted",
        artifact.applied_at.as_deref(),
        None,
        None,
        artifact.post_image_sha256.as_deref(),
    )?;
    store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after revert"))
}

/// M4 Forge: single-artifact read for state reconciliation on panel open.
#[tauri::command]
pub async fn get_artifact(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<Artifact>, String> {
    state.store.artifact_by_id(&id).map_err(|e| e.to_string())
}

/// M4 Forge: artifact history, optionally filtered by finding/issue id.
#[tauri::command]
pub async fn list_artifacts(
    state: tauri::State<'_, AppState>,
    finding_id: Option<String>,
) -> Result<Vec<Artifact>, String> {
    let result = match finding_id {
        Some(fid) => state.store.artifacts_for_finding(&fid),
        None => state.store.all_artifacts(),
    };
    result.map_err(|e| e.to_string())
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
/// RADAR (Task 9): the live agent forest, contract-shaped (`radar_state`). Reads
/// live transcript tails before returning so the frontend's visible-RADAR polling
/// closes any missed filesystem event gap, then caches the fresh forest for push
/// consumers.
#[tauri::command]
pub async fn get_radar_state(state: tauri::State<'_, AppState>) -> Result<RadarState, String> {
    Ok(fresh_radar_state_for_read(&state))
}

/// RADAR: the flat list of live agents (the forest's `agents`). Shares the
/// `assemble` path with [`get_radar_state`]; kept for the fleet roster surface.
#[tauri::command]
pub async fn list_fleet(state: tauri::State<'_, AppState>) -> Result<Vec<RadarAgent>, String> {
    Ok(fresh_radar_state_for_read(&state).agents)
}

fn fresh_radar_state_for_read(state: &AppState) -> RadarState {
    let sessions_dir = default_claude_sessions_dir();
    radar::refresh_live_context(&state.store, &sessions_dir);
    let radar = radar::recompute_radar_state(&state.store, &sessions_dir);
    state.cache_radar_state(radar.clone());
    radar
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

    #[test]
    fn app_state_caches_latest_radar_state_for_read_commands() {
        let store = Store::memory().unwrap();
        let state = AppState::from_store_for_test(store);
        let radar = RadarState {
            generated_at: "2026-06-25T07:50:00Z".into(),
            agents: Vec::new(),
        };

        assert_eq!(state.cached_radar_state(), None);
        state.cache_radar_state(radar.clone());

        assert_eq!(
            state.cached_radar_state(),
            Some(radar),
            "RADAR read commands should return the last emitted forest without recomputing"
        );
    }

    #[test]
    fn radar_read_refreshes_live_codex_even_when_cache_is_stale() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_codex_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_codex_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");
        let old_claude_sessions = std::env::var_os("WARDEN_CLAUDE_SESSIONS");
        let old_claude_projects = std::env::var_os("WARDEN_CLAUDE_PROJECTS");

        let codex_sessions = tempfile::tempdir().unwrap();
        let codex_archived = tempfile::tempdir().unwrap();
        let claude_sessions = tempfile::tempdir().unwrap();
        let claude_projects = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", codex_sessions.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", codex_archived.path());
        std::env::set_var("WARDEN_CLAUDE_SESSIONS", claude_sessions.path());
        std::env::set_var("WARDEN_CLAUDE_PROJECTS", claude_projects.path());

        let live_dir = codex_sessions.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let live =
            live_dir.join("rollout-2026-06-25T12-00-00-019f0040-0000-7000-8000-000000000001.jsonl");
        std::fs::write(
            &live,
            "{\"timestamp\":\"2026-06-25T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019f0040-0000-7000-8000-000000000001\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\",\"thread_source\":\"user\"}}\n\
             {\"timestamp\":\"2026-06-25T19:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"watch this live Codex agent\"}}\n",
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let state = AppState::from_store_for_test(store);
        state.cache_radar_state(RadarState {
            generated_at: "2026-06-25T07:50:00Z".into(),
            agents: Vec::new(),
        });

        let radar = fresh_radar_state_for_read(&state);

        assert!(
            radar
                .agents
                .iter()
                .any(|a| a.harness == "codex" && a.label == "LiveCodex"),
            "a RADAR read must not let a stale cache hide a live Codex rollout"
        );
        assert_eq!(
            state.store.watermark_offset(&live).unwrap(),
            std::fs::metadata(&live).unwrap().len(),
            "the read path must ingest the live Codex tail before returning"
        );

        match old_codex_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_codex_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }
        match old_claude_sessions {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_SESSIONS"),
        }
        match old_claude_projects {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_PROJECTS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_PROJECTS"),
        }
    }

    /// Smoke test for the RADAR core shape: with a seeded session, `assemble`
    /// returns a contract-shaped forest (one agent, serialized camelCase). Exercising
    /// the shared core avoids constructing a Tauri `State`.
    #[test]
    fn radar_command_core_returns_contract_shaped_forest() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 1);

        // Exercise the shared core (`assemble`) directly so no Tauri `State` is built.
        // A live Claude registry entry makes the seeded root OPEN under the membership
        // filter; is_alive=true and the codex predicate is unused for a Claude root.
        let reg = tempfile::tempdir().unwrap();
        std::fs::write(
            reg.path().join("100.json"),
            serde_json::json!({ "pid": 100, "sessionId": "c1", "cwd": "/work" }).to_string(),
        )
        .unwrap();
        let state = crate::radar::assemble(
            &store,
            reg.path(),
            &|_| true,
            &|_| false,
            chrono::Utc::now(),
        );
        assert_eq!(state.agents.len(), 1, "one seeded OPEN session → one agent");
        let agent = &state.agents[0];
        assert_eq!(agent.id, "c1");
        assert_eq!(agent.harness, "claude_code");
        assert_eq!(agent.depth, 0);
        assert_eq!(agent.parent_id, None);

        let json = serde_json::to_string(&state).unwrap();
        for key in [
            "\"generatedAt\"",
            "\"fillPct\"",
            "\"contextTokens\"",
            "\"childCount\"",
        ] {
            assert!(json.contains(key), "contract camelCase key {key} present");
        }
    }

    // ── M4 Forge command-core happy path (temp WARDEN_CLAUDE_MD — never real) ──

    /// A persisted finding the staging path can resolve. Carries one evidence ref
    /// so `save_findings` records a session id.
    fn saved_finding(store: &Store, id: &str, pattern: &str) {
        let finding = Finding {
            id: id.into(),
            pattern_id: pattern.into(),
            title: pattern.into(),
            severity: 4,
            frequency: 0.5,
            est_cost_tokens: 1,
            est_cost_minutes: 1,
            confidence: 0.7,
            rationale: "r".into(),
            evidence: vec![EvidenceRef {
                session_id: "s1".into(),
                turn_id: None,
                event_id: None,
                quote: None,
                source_path: None,
            }],
            status: "candidate".into(),
            verifier_verdict: None,
        };
        store.save_findings(&[finding]).unwrap();
    }

    #[test]
    fn forge_stage_apply_revert_happy_path_against_temp_target() {
        // Serialize env mutation so WARDEN_CLAUDE_MD can't race other env tests.
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("WARDEN_CLAUDE_MD");

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "# Project\n\nexisting line\n").unwrap();
        std::env::set_var("WARDEN_CLAUDE_MD", &target);

        let store = Store::memory().unwrap();
        saved_finding(&store, "find-1", "UNVERIFIED_COMPLETION");

        // STAGE → PENDING row pointing at the temp target.
        let staged = stage_artifact_inner(&store, Some("find-1"), None).unwrap();
        assert_eq!(staged.status, "pending");
        assert_eq!(staged.finding_id.as_deref(), Some("find-1"));
        assert_eq!(staged.target_path, target.to_string_lossy());
        assert!(!staged.diff.is_empty());
        assert!(staged.block.contains("UNVERIFIED_COMPLETION"));

        // Re-stage returns the SAME row (idempotent, no duplicate).
        let restaged = stage_artifact_inner(&store, Some("find-1"), None).unwrap();
        assert_eq!(restaged.id, staged.id);
        assert_eq!(store.all_artifacts().unwrap().len(), 1);

        // APPLY → file gets exactly one block, status applied, backup + sha recorded.
        let applied = apply_artifact_inner(&store, &staged.id).unwrap();
        assert_eq!(applied.status, "applied");
        assert!(applied.applied_at.is_some());
        assert!(applied.backup_path.is_some());
        assert!(applied.pre_image_sha256.is_some());
        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            on_disk
                .matches("## WARDEN guardrail — UNVERIFIED_COMPLETION")
                .count(),
            1,
            "exactly one guardrail block on disk"
        );

        // Double-apply is a no-op: status stays applied, file unchanged.
        let reapplied = apply_artifact_inner(&store, &staged.id).unwrap();
        assert_eq!(reapplied.status, "applied");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), on_disk);

        // REVERT → file restored to the pre-image, status reverted.
        let reverted = revert_artifact_inner(&store, &staged.id).unwrap();
        assert_eq!(reverted.status, "reverted");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "# Project\n\nexisting line\n",
            "revert restores the exact pre-image"
        );

        match prev {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_MD", v),
            None => std::env::remove_var("WARDEN_CLAUDE_MD"),
        }
    }

    #[test]
    fn forge_apply_unknown_artifact_errors() {
        let store = Store::memory().unwrap();
        assert!(apply_artifact_inner(&store, "ghost").is_err());
        assert!(revert_artifact_inner(&store, "ghost").is_err());
    }

    #[test]
    fn forge_stage_requires_finding_or_issue() {
        let store = Store::memory().unwrap();
        assert!(stage_artifact_inner(&store, None, None).is_err());
        assert!(stage_artifact_inner(&store, Some("nope"), None).is_err());
    }

    /// Set WARDEN_CLAUDE_MD to `target`, run `body`, then restore the prior value —
    /// holding the shared env lock so parallel env tests don't race. Mirrors the
    /// happy-path test's manual guard, hoisted so the regression tests below stay
    /// focused on the behavior they prove.
    fn with_claude_md_env<T>(target: &std::path::Path, body: impl FnOnce() -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("WARDEN_CLAUDE_MD");
        std::env::set_var("WARDEN_CLAUDE_MD", target);
        let out = body();
        match prev {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_MD", v),
            None => std::env::remove_var("WARDEN_CLAUDE_MD"),
        }
        out
    }

    /// REGRESSION (critical data-loss): apply → revert (file back to ORIGINAL) →
    /// user rewrites the file by hand → a SECOND revert MUST be a true no-op and must
    /// NOT clobber the user's new content with the stale pre-image. Before the fix,
    /// the second revert re-entered the restore branch (backup + sha still recorded),
    /// the integrity check passed, and the original was atomic-written over the user's
    /// edits. The terminal-state guard (`status != "applied"`) makes the second revert
    /// inert.
    #[test]
    fn forge_double_revert_does_not_clobber_user_edits() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "ORIGINAL\n").unwrap();

        with_claude_md_env(&target, || {
            let store = Store::memory().unwrap();
            saved_finding(&store, "find-dr", "UNVERIFIED_COMPLETION");

            let staged = stage_artifact_inner(&store, Some("find-dr"), None).unwrap();
            apply_artifact_inner(&store, &staged.id).unwrap();

            // First revert restores ORIGINAL.
            let reverted = revert_artifact_inner(&store, &staged.id).unwrap();
            assert_eq!(reverted.status, "reverted");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "ORIGINAL\n");

            // The user now rewrites the file entirely.
            std::fs::write(&target, "USER REWROTE EVERYTHING\n").unwrap();

            // A second revert must be a no-op and leave the user's content intact.
            let second = revert_artifact_inner(&store, &staged.id).unwrap();
            assert_eq!(second.status, "reverted");
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                "USER REWROTE EVERYTHING\n",
                "DOUBLE REVERT MUST NOT CLOBBER USER EDITS"
            );
        });
    }

    /// REGRESSION (critical data-loss): apply → user edits the applied file
    /// out-of-band (so it no longer matches what WARDEN wrote) → revert MUST REFUSE
    /// rather than blindly restoring the pre-image over the user's newer edits. The
    /// post-image drift guard catches the mismatch.
    #[test]
    fn forge_revert_refuses_when_target_drifted_out_of_band() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "ORIGINAL\n").unwrap();

        with_claude_md_env(&target, || {
            let store = Store::memory().unwrap();
            saved_finding(&store, "find-drift", "UNVERIFIED_COMPLETION");
            let staged = stage_artifact_inner(&store, Some("find-drift"), None).unwrap();
            apply_artifact_inner(&store, &staged.id).unwrap();

            // User edits the applied file out-of-band (keeps the block, adds a line).
            let applied = std::fs::read_to_string(&target).unwrap();
            let user_edited = format!("{applied}\nUSER ADDED A CRITICAL RULE\n");
            std::fs::write(&target, &user_edited).unwrap();

            let err = revert_artifact_inner(&store, &staged.id).unwrap_err();
            assert!(
                err.to_string().contains("changed out-of-band"),
                "expected an out-of-band drift refusal, got: {err}"
            );
            // The user's out-of-band edits must survive — revert wrote nothing.
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                user_edited,
                "refused revert must leave the drifted file untouched"
            );
        });
    }

    /// REGRESSION (data-loss): a changing re-apply on the SAME artifact id must NOT
    /// overwrite the pristine backup with intermediate (user-edited) content. Apply
    /// #1 backs up the original. The user then removes the block + edits other lines
    /// out-of-band. Apply #2 is changing (block re-added) but must PRESERVE the
    /// original backup and keep the original pre_image_sha256, so a later revert
    /// restores the true pre-WARDEN baseline.
    #[test]
    fn forge_reapply_preserves_original_backup() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "GEN-1 ORIGINAL\n").unwrap();

        with_claude_md_env(&target, || {
            let store = Store::memory().unwrap();
            saved_finding(&store, "find-re", "UNVERIFIED_COMPLETION");
            let staged = stage_artifact_inner(&store, Some("find-re"), None).unwrap();

            let applied1 = apply_artifact_inner(&store, &staged.id).unwrap();
            let backup_path = crate::util::expand_tilde(applied1.backup_path.as_ref().unwrap());
            assert_eq!(std::fs::read_to_string(&backup_path).unwrap(), "GEN-1 ORIGINAL\n");
            let pre_sha1 = applied1.pre_image_sha256.clone().unwrap();

            // User removes the guardrail block AND rewrites the rest out-of-band.
            std::fs::write(&target, "GEN-2 USER EDIT\n").unwrap();

            // Re-apply: changing (block re-added) but the original backup must stand.
            let applied2 = apply_artifact_inner(&store, &staged.id).unwrap();
            assert_eq!(
                std::fs::read_to_string(&backup_path).unwrap(),
                "GEN-1 ORIGINAL\n",
                "re-apply must NOT overwrite the pristine backup with GEN-2"
            );
            assert_eq!(
                applied2.pre_image_sha256.as_deref(),
                Some(pre_sha1.as_str()),
                "re-apply must keep the ORIGINAL pre_image_sha256"
            );
        });
    }
}
