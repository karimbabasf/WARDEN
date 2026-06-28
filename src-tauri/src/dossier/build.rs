//! DOSSIER Phase 6 — build orchestration.
//!
//! [`build_profile`] is the single entry point that wires Phases 1–5 into one
//! [`Profile`]: scope → hash → cache check → aggregate → score → archetypes →
//! trajectory (by-proof gated) → windowed findings → bounded summaries →
//! synthesis (LLM-enriched or degraded) → persist. Every stage emits a
//! `dossier_progress` event so the UI can narrate the build; every store/brain
//! call goes through the seams Phases 1–5 already froze (no new signatures).
//!
//! Two correctness anchors:
//!   * **Cache key = `(window, data_hash)`.** [`data_hash`] fingerprints the
//!     scoped session set (sorted, content-hashed), so the moment a session is
//!     ingested or re-ingested the cached profile for that window goes stale and
//!     is rebuilt. Same sessions in a different read order ⇒ identical hash.
//!   * **Always returns a profile.** Synthesis degrades internally (brain
//!     unavailable or any LLM error ⇒ deterministic `detector_only` profile), so
//!     the product never has nothing to show.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use sha2::{Digest, Sha256};
use tauri::Emitter;

use crate::brain::Brain;
use crate::dossier::aggregate::aggregate_window;
use crate::dossier::archetype::classify_archetypes;
use crate::dossier::cluster;
use crate::dossier::efficiency::window_efficiency;
use crate::dossier::proof::gate_trend;
use crate::dossier::scope::{scoped_sessions, Window};
use crate::dossier::summarize::{
    project_rollup, session_summary_via_brain, week_rollup, ProjectSummary, WeekSummary,
};
use crate::dossier::synthesize::{synthesize_profile_via_brain, SynthesisInputs};
use crate::dossier::trajectory::classify_trend;
use crate::dossier::types::{Profile, ProjectArchetype, TrendPoint};
use crate::detectors::nominate_windowed;
use crate::ir::{FeatureVector, Finding, Session};
use crate::store::Store;

/// Cap on the number of sessions micro-summarized on a first (cache-cold) build.
/// Summaries are per-session LLM calls; without a cap an "all-time over 6 months"
/// build would fan out into thousands of requests. The most-recent
/// `SUMMARY_BUILD_CAP` sessions feed the week/project rollups; older sessions
/// still count in the deterministic aggregate, just not the narrative digests.
pub const SUMMARY_BUILD_CAP: usize = 40;

/// The single trait the v1 trajectory series tracks: mean per-week outcome.
const TRAJECTORY_TRAIT: &str = "outcome";

/// Deterministic fingerprint of a scoped session set.
///
/// Sorts the sessions by `id` (so read order never changes the result), then
/// folds each session's `id` + `raw_hash` into a SHA-256 digest, hex-encoded.
/// `raw_hash` is the per-file content hash, so a re-ingested (changed) session
/// flips the digest and invalidates the cached profile; a pure reordering does
/// not. An empty set hashes to the digest of no input (a stable constant).
pub fn data_hash(sessions: &[Session]) -> String {
    let mut sorted: Vec<&Session> = sessions.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut hasher = Sha256::new();
    for s in sorted {
        hasher.update(s.id.as_bytes());
        hasher.update([0u8]); // field separator so id|hash boundaries can't collide
        hasher.update(s.raw_hash.to_le_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

/// The window's frozen wire string (e.g. `"30d"`, `"all-time"`) — the cache key
/// and the value embedded in [`Profile::window`]. Derived from the serde rename
/// (single source of truth) rather than a hand-maintained match, so a `Window`
/// variant rename can never drift the cache key away from the JSON.
fn window_kebab(window: Window) -> String {
    serde_json::to_value(window)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        // `Window` always serializes to a bare string; this fallback is purely
        // defensive and is never reached.
        .unwrap_or_else(|| format!("{window:?}"))
}

/// Emit a `dossier_progress` stage event when running inside Tauri. A no-op when
/// `app` is `None` (unit tests, headless builds) — the build proceeds either way.
fn emit(app: Option<&tauri::AppHandle>, payload: serde_json::Value) {
    if let Some(app) = app {
        let _ = app.emit("dossier_progress", payload);
    }
}

/// Build (or serve from cache) the full operator [`Profile`] for one `window`.
///
/// `now` is injected (not read from the clock) so the result is deterministic
/// and unit-testable. `app` is optional: when present, each stage emits a
/// `dossier_progress` event; when absent the build runs silently. Never panics
/// and never requires the network — synthesis degrades to a deterministic
/// profile when the brain is unavailable or errors.
pub async fn build_profile(
    store: &Store,
    brain: &Brain,
    window: Window,
    now: DateTime<Utc>,
    app: Option<&tauri::AppHandle>,
) -> anyhow::Result<Profile> {
    // 1. Scope + fingerprint.
    let sessions = scoped_sessions(store, window, now)?;
    let hash = data_hash(&sessions);
    let kebab = window_kebab(window);
    let session_count = sessions.len() as u32;
    emit(
        app,
        serde_json::json!({
            "stage": "scope",
            "status": "done",
            "session_count": session_count,
        }),
    );

    // 2. Cache check — a fresh hit short-circuits the entire pipeline.
    if let Some(cached) = store.dossier_profile_get(&kebab, &hash)? {
        if let Ok(profile) = serde_json::from_str::<Profile>(&cached) {
            emit(app, serde_json::json!({"stage": "cache", "status": "hit"}));
            return Ok(profile);
        }
        // A corrupt/old-schema cache row is treated as a miss and rebuilt below.
    }

    // 3. Windowed feature slice (the same slice every aggregator/scorer reads).
    let cutoff = window.cutoff(now);
    let features = store.features_since(cutoff)?;

    // 4. Deterministic substrate: aggregate, efficiency, archetypes.
    //    Archetypes try the Phase-4 embedding clusterer and DEGRADE to the
    //    keyword heuristic on any embedding failure (see `clustered_archetypes`).
    let aggregate = aggregate_window(store, window, now)?;
    let efficiency = window_efficiency(store, window, now)?;
    let archetypes = clustered_archetypes(store, &features);
    emit(app, serde_json::json!({"stage": "aggregate", "status": "done"}));
    emit(app, serde_json::json!({"stage": "score", "status": "done"}));

    // 5. Trajectory: a per-week outcome series, by-proof gated by samples/bucket.
    let points: Vec<TrendPoint> = aggregate
        .by_week
        .iter()
        .map(|w| TrendPoint {
            bucket: w.iso_week.clone(),
            value: w.mean_outcome,
        })
        .collect();
    let samples_per_bucket: Vec<usize> =
        aggregate.by_week.iter().map(|w| w.sessions as usize).collect();
    let trend = classify_trend(TRAJECTORY_TRAIT, points);
    let trajectory = vec![gate_trend(trend, &samples_per_bucket)];

    // 6. Windowed detector findings — the evidence source for holes/leaks.
    //    Best-effort semantic dedup collapses near-duplicate finding titles into
    //    one representative each (keeping the strongest), so the synthesis layer
    //    isn't fed three phrasings of the same hole. Degrades to a no-op (every
    //    finding kept) when embeddings are unavailable.
    let findings = dedup_findings(store, nominate_windowed(store, &store.profile()?, cutoff)?);

    // 7. Bounded summaries — only when the brain is available; else empty vecs.
    //    Cache-cold cost is bounded to the most-recent SUMMARY_BUILD_CAP sessions
    //    (sessions are newest-first from the store); each call is itself cached.
    let (week_summaries, project_summaries) =
        build_summaries(store, brain, &sessions, &features, app).await;

    // 8. Synthesis — LLM-enriched, degrading to detector-only internally.
    let inputs = SynthesisInputs {
        window,
        data_hash: hash.clone(),
        session_count,
        aggregate,
        efficiency,
        archetypes,
        trajectory,
        findings,
        week_summaries,
        project_summaries,
    };
    let profile = synthesize_profile_via_brain(&inputs, now, brain).await;
    emit(
        app,
        serde_json::json!({
            "stage": "synthesize",
            "status": "done",
            "detector_only": profile.detector_only,
        }),
    );

    // 9. Persist under (window, hash) so an unchanged re-open is a cache hit.
    store.dossier_profile_put(&kebab, &hash, &serde_json::to_string(&profile)?)?;
    emit(app, serde_json::json!({"stage": "persist", "status": "done"}));

    Ok(profile)
}

/// Project archetypes via the Phase-4 embedding clusterer, **degrading to the
/// keyword heuristic on any failure**.
///
/// This is the safety net that keeps embeddings a pure enhancement: it tries
/// [`cluster::archetypes_from_vectors`] over cache-aware embeddings of the
/// distinct project names; if the model is unavailable (offline / no HF cache)
/// or there are no projects to embed, it returns
/// [`classify_archetypes`]`(&features)` — the exact deterministic result this
/// build used before Phase 4. The build can therefore NEVER fail because
/// embeddings failed.
///
/// On the clustered path, `session_count`/`note` are reconciled from the real
/// per-project session totals (one feature row = one session) that this layer
/// owns — the clusterer only knows project counts.
fn clustered_archetypes(store: &Store, features: &[FeatureVector]) -> Vec<ProjectArchetype> {
    // Per-project real session counts (mirrors classify_archetypes' folding: a
    // None project counts under the literal "unknown").
    let mut sessions_per_project: BTreeMap<String, u32> = BTreeMap::new();
    for fv in features {
        let project = fv.project.clone().unwrap_or_else(|| "unknown".to_string());
        *sessions_per_project.entry(project).or_insert(0) += 1;
    }

    // Distinct project names, deterministically ordered (BTreeMap keys).
    let names: Vec<String> = sessions_per_project.keys().cloned().collect();
    if names.is_empty() {
        return classify_archetypes(features);
    }

    // Cache-aware embeddings of the descriptors (descriptor = project name in v1;
    // embeddings generalize past the heuristic's literal substring match).
    let vectors = match embed_cached(store, &names) {
        Ok(v) if v.len() == names.len() => v,
        _ => return classify_archetypes(features), // model unavailable ⇒ heuristic
    };

    let mut clustered = cluster::archetypes_from_vectors(&names, &vectors);
    if clustered.is_empty() {
        return classify_archetypes(features);
    }

    // Reconcile real session totals: the clusterer set session_count to the
    // member-project count; overwrite it with the summed real session counts and
    // restate the note to match classify_archetypes' wording.
    for arche in clustered.iter_mut() {
        let total: u32 = arche
            .projects
            .iter()
            .map(|p| sessions_per_project.get(p).copied().unwrap_or(0))
            .sum();
        arche.session_count = total;
        let n_projects = arche.projects.len();
        arche.note = format!(
            "{} project{}, {} session{} (embedding-clustered)",
            n_projects,
            if n_projects == 1 { "" } else { "s" },
            total,
            if total == 1 { "" } else { "s" },
        );
    }

    // Re-sort after the count reconciliation so ordering still reflects real
    // session totals (count desc, archetype name asc).
    clustered.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| a.archetype.cmp(&b.archetype))
    });
    clustered
}

/// Embed `texts` through the `dossier_embeddings` cache: read every hit, embed
/// only the misses (one batched model call), persist the new vectors, and return
/// the vectors 1:1 with `texts`.
///
/// Keyed by `(content_hash(text), MODEL_NAME)` so an unchanged descriptor is
/// embedded at most once ever, across windows and rebuilds. Returns `Err` only
/// when the misses need the model and it is unavailable — the caller then
/// degrades to the heuristic.
fn embed_cached(store: &Store, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    let hashes: Vec<String> = texts.iter().map(|t| cluster::content_hash(t)).collect();

    // 1. Resolve cache hits; collect the indices/texts that miss.
    let mut cached: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
    let mut miss_idx: Vec<usize> = Vec::new();
    let mut miss_text: Vec<String> = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        match store.dossier_embedding_get(&hashes[i], cluster::MODEL_NAME)? {
            Some(v) => cached.push(Some(v)),
            None => {
                cached.push(None);
                miss_idx.push(i);
                miss_text.push(text.clone());
            }
        }
    }

    // 2. Embed only the misses (one batched call), and persist them.
    if !miss_text.is_empty() {
        let fresh = cluster::embed_texts(&miss_text)?; // Err ⇒ caller degrades
        for (k, vec) in fresh.into_iter().enumerate() {
            let i = miss_idx[k];
            store.dossier_embedding_put(&hashes[i], cluster::MODEL_NAME, &vec)?;
            cached[i] = Some(vec);
        }
    }

    // 3. Unwrap in order (every slot is filled by now).
    Ok(cached.into_iter().map(|o| o.unwrap_or_default()).collect())
}

/// Collapse near-duplicate findings by title, keeping one representative per
/// group — **best-effort, never fails**.
///
/// Uses [`cluster::semantic_dedup`] (which itself degrades to all-singletons when
/// embeddings are unavailable, so this becomes a no-op that keeps every finding).
/// Within each near-duplicate group the representative is the highest-severity,
/// then highest-confidence finding; output preserves the first-seen order of the
/// kept representatives so the result stays deterministic.
fn dedup_findings(store: &Store, findings: Vec<Finding>) -> Vec<Finding> {
    if findings.len() < 2 {
        return findings;
    }

    // Warm the embedding cache for these titles (so repeated builds are cheap),
    // then group by cosine. We ignore embed_cached's result here — semantic_dedup
    // recomputes via its own (cache-backed) path and is the source of truth for
    // grouping; the warm-up is purely an optimization and must never fail the
    // build, hence the discard.
    let titles: Vec<String> = findings.iter().map(|f| f.title.clone()).collect();
    let _ = embed_cached(store, &titles);

    let groups = cluster::semantic_dedup(&titles, cluster::DEDUP_THRESHOLD);

    // For each group, pick the strongest representative; remember its original
    // index so we can emit kept findings in first-seen order.
    let mut keep: Vec<usize> = groups
        .iter()
        .map(|group| {
            *group
                .iter()
                .max_by(|&&a, &&b| {
                    findings[a]
                        .severity
                        .cmp(&findings[b].severity)
                        .then_with(|| {
                            findings[a]
                                .confidence
                                .partial_cmp(&findings[b].confidence)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                })
                .expect("a dedup group is never empty")
        })
        .collect();
    keep.sort_unstable();

    keep.into_iter().map(|i| findings[i].clone()).collect()
}

/// Build the week/project narrative rollups, bounded + brain-gated.
///
/// Returns `(vec![], vec![])` when the brain is unavailable (the degradation
/// path — no network, zero cost). Otherwise micro-summarizes the most-recent
/// `SUMMARY_BUILD_CAP` sessions (each call cached by `session_summary_via_brain`,
/// so a per-session summary is computed at most once), keying each summary by
/// its ISO week and project for the deterministic folds. A session with no
/// matching feature row is skipped (no project/week to bin it under).
async fn build_summaries(
    store: &Store,
    brain: &Brain,
    sessions: &[Session],
    features: &[FeatureVector],
    app: Option<&tauri::AppHandle>,
) -> (Vec<WeekSummary>, Vec<ProjectSummary>) {
    if !brain.available() {
        emit(
            app,
            serde_json::json!({"stage": "summarize", "status": "skipped", "n": 0}),
        );
        return (Vec::new(), Vec::new());
    }

    let by_id: std::collections::HashMap<&str, &FeatureVector> =
        features.iter().map(|f| (f.session_id.as_str(), f)).collect();

    let mut week_pairs: Vec<(String, String)> = Vec::new();
    let mut project_pairs: Vec<(String, String)> = Vec::new();
    let mut n = 0usize;

    for session in sessions.iter().take(SUMMARY_BUILD_CAP) {
        let Some(fv) = by_id.get(session.id.as_str()) else {
            continue; // no feature row ⇒ nothing to bin this summary under
        };
        let summary = match session_summary_via_brain(store, brain, session, fv, &[]).await {
            Ok(s) => s,
            Err(err) => {
                // One bad session must not sink the whole build — skip it.
                tracing::warn!(
                    "dossier summary failed for session {} ({err}); skipping",
                    session.id
                );
                continue;
            }
        };
        let project = fv.project.clone().unwrap_or_else(|| "unknown".to_string());
        week_pairs.push((iso_week_label(session.started_at), summary.clone()));
        project_pairs.push((project, summary));
        n += 1;
    }

    emit(
        app,
        serde_json::json!({"stage": "summarize", "status": "done", "n": n}),
    );
    (week_rollup(&week_pairs), project_rollup(&project_pairs))
}

/// Format a UTC instant's ISO week as `YYYY-Www` — the SAME label
/// `aggregate::WeekBin` uses, so week digests align with the week rollups.
fn iso_week_label(ts: DateTime<Utc>) -> String {
    let iso = ts.iso_week();
    format!("{}-W{:02}", iso.year(), iso.week())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Harness, Session};
    use crate::store::Store;
    use chrono::{Duration, TimeZone};
    use std::path::PathBuf;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap()
    }

    fn session(id: &str, raw_hash: u64, started_at: DateTime<Utc>) -> Session {
        Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash,
            ingested_at: started_at,
            meta: serde_json::json!({}),
        }
    }

    /// Seed a session + its feature row via the real upsert/save paths, exactly
    /// as the Phase 1/2 aggregate tests do.
    fn seed(store: &Store, id: &str, raw_hash: u64, started_at: DateTime<Utc>, clean: bool) {
        store
            .upsert_session_batch(&session(id, raw_hash, started_at), &[], &[], 0)
            .unwrap();
        store
            .save_feature(
                &FeatureVector {
                    session_id: id.into(),
                    started_at: Some(started_at),
                    project: Some("alpha".into()),
                    token_burn_total: 1000,
                    tool_call_count: 6,
                    verification_present: clean,
                    cache_read_ratio: if clean { 0.8 } else { 0.0 },
                    ..Default::default()
                },
                "test",
            )
            .unwrap();
    }

    /// The hash is order-independent (sort by id) and content-sensitive (a
    /// changed `raw_hash` flips it).
    #[test]
    fn data_hash_is_deterministic_and_order_independent() {
        let now = fixed_now();
        let a = session("a", 11, now);
        let b = session("b", 22, now);

        let forward = data_hash(&[a.clone(), b.clone()]);
        let reversed = data_hash(&[b.clone(), a.clone()]);
        assert_eq!(forward, reversed, "read order must not change the hash");

        // A changed content hash on one session changes the digest.
        let b_changed = session("b", 99, now);
        let changed = data_hash(&[a, b_changed]);
        assert_ne!(forward, changed, "a changed raw_hash must change the digest");
    }

    /// `window_kebab` mirrors the serde wire string (the cache key contract).
    #[test]
    fn window_kebab_matches_wire_string() {
        assert_eq!(window_kebab(Window::ThirtyDays), "30d");
        assert_eq!(window_kebab(Window::AllTime), "all-time");
        assert_eq!(window_kebab(Window::SixMonths), "6mo");
    }

    /// With NO brain key (`available() == false`), a build still produces a valid
    /// `detector_only` profile with the seven canonical dimensions — no panic, no
    /// network — and a SECOND build of the unchanged window serves the cached
    /// profile (proven byte-identical and via the cache row being present).
    #[tokio::test]
    async fn build_profile_degrades_without_brain() {
        // Ensure no engine is configured so `Brain::available()` is false.
        std::env::remove_var("WARDEN_BRAIN_API_KEY");
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENAI_BASE_URL");

        let store = Store::memory().unwrap();
        let now = fixed_now();
        seed(&store, "s1", 1, now - Duration::days(3), true);
        seed(&store, "s2", 2, now - Duration::days(9), false);
        seed(&store, "s3", 3, now - Duration::days(20), true);

        let brain = Brain::new(store.clone());
        assert!(!brain.available(), "test must run with no engine configured");

        // First build: degraded synthesis, fully deterministic.
        let p1 = build_profile(&store, &brain, Window::ThirtyDays, now, None)
            .await
            .unwrap();
        assert!(p1.detector_only, "no brain ⇒ detector-only profile");
        assert_eq!(p1.dimensions.len(), 7, "all seven dimensions present");
        assert_eq!(p1.session_count, 3);
        assert_eq!(p1.window, Window::ThirtyDays);
        // The seven dimensions are exactly the canonical keys, in order — the
        // degraded profile is still structurally complete, not a stub.
        let keys: Vec<&str> = p1.dimensions.iter().map(|d| d.key.as_str()).collect();
        let expected: Vec<&str> = crate::dossier::types::DIMENSION_KEYS
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(keys, expected, "dimensions must be the 7 canonical keys in order");

        // The cache row now exists under the window's kebab + current hash.
        let kebab = window_kebab(Window::ThirtyDays);
        let hash = data_hash(&scoped_sessions(&store, Window::ThirtyDays, now).unwrap());
        assert!(
            store.dossier_profile_get(&kebab, &hash).unwrap().is_some(),
            "first build must have persisted the profile (observable cache hit)"
        );

        // Second build of the unchanged window ⇒ byte-identical cached profile.
        let p2 = build_profile(&store, &brain, Window::ThirtyDays, now, None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_string(&p1).unwrap(),
            serde_json::to_string(&p2).unwrap(),
            "second build of an unchanged window must serve the cached profile"
        );
        assert_eq!(
            p1.generated_at, p2.generated_at,
            "cache hit preserves the original generated_at (not a fresh build)"
        );
    }

    /// Build a feature row for an explicit project (so a test can mix archetypes).
    fn feat_for(project: &str, started_at: DateTime<Utc>) -> FeatureVector {
        FeatureVector {
            session_id: format!("{project}-{}", started_at.timestamp()),
            started_at: Some(started_at),
            project: Some(project.into()),
            ..Default::default()
        }
    }

    /// SAFETY NET — the whole point of Phase 4. When embeddings are unavailable
    /// (the default in tests; in production the kill switch or a model-load
    /// failure), `clustered_archetypes` must return EXACTLY what the keyword
    /// heuristic returns: same archetypes, same projects, same session counts.
    #[test]
    fn clustered_archetypes_degrades_to_heuristic_when_embeddings_disabled() {
        let store = Store::memory().unwrap();
        let now = fixed_now();
        // Two web projects (2 + 1 sessions) and one infra project (1 session) —
        // exercises rollup + session-count summation.
        let features = vec![
            feat_for("acme-web-app", now),
            feat_for("acme-web-app", now - Duration::days(1)),
            feat_for("beta-ui", now - Duration::days(2)),
            feat_for("deploy-infra", now - Duration::days(3)),
        ];

        let degraded = clustered_archetypes(&store, &features);
        let heuristic = classify_archetypes(&features);

        // Identical to the heuristic, field for field (the fallback returns the
        // heuristic value verbatim).
        assert_eq!(degraded.len(), heuristic.len());
        for (d, h) in degraded.iter().zip(heuristic.iter()) {
            assert_eq!(d.archetype, h.archetype, "same archetype label");
            assert_eq!(d.projects, h.projects, "same member projects");
            assert_eq!(d.session_count, h.session_count, "same session totals");
            assert_eq!(d.note, h.note, "same note (heuristic wording)");
        }
        // Sanity: the substrate is what we expect (web_app 3 sessions, infra 1).
        assert_eq!(degraded[0].archetype, "web_app");
        assert_eq!(degraded[0].session_count, 3);
    }

    /// End-to-end: a full `build_profile` with embeddings unavailable still
    /// succeeds and its archetypes equal the heuristic's — embeddings failing can
    /// never break the build (it just degrades).
    #[tokio::test]
    async fn build_profile_succeeds_with_embeddings_disabled() {
        std::env::remove_var("WARDEN_BRAIN_API_KEY");
        std::env::remove_var("WARDEN_BRAIN_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENAI_BASE_URL");

        let store = Store::memory().unwrap();
        let now = fixed_now();
        seed(&store, "s1", 1, now - Duration::days(3), true);
        seed(&store, "s2", 2, now - Duration::days(9), false);

        let brain = Brain::new(store.clone());
        let profile = build_profile(&store, &brain, Window::ThirtyDays, now, None)
            .await
            .unwrap();

        // Build succeeded with a complete profile despite embeddings being off.
        assert_eq!(profile.dimensions.len(), 7);
        assert_eq!(profile.session_count, 2);
        // Archetypes equal the heuristic over the same window's features.
        let features = store.features_since(Window::ThirtyDays.cutoff(now)).unwrap();
        let heuristic = classify_archetypes(&features);
        assert_eq!(
            serde_json::to_string(&profile.archetypes).unwrap(),
            serde_json::to_string(&heuristic).unwrap(),
            "embeddings-disabled build must carry the heuristic archetypes"
        );
    }

    /// `dedup_findings` is a no-op safety-wise when embeddings are disabled (the
    /// test default): every finding is kept (semantic_dedup degrades to
    /// all-singletons), and order is preserved.
    #[test]
    fn dedup_findings_keeps_all_when_embeddings_disabled() {
        let store = Store::memory().unwrap();

        let mk = |id: &str, title: &str, sev: u8| Finding {
            id: id.into(),
            pattern_id: "p".into(),
            title: title.into(),
            severity: sev,
            frequency: 1.0,
            est_cost_tokens: 0,
            est_cost_minutes: 0,
            confidence: 0.5,
            rationale: String::new(),
            evidence: vec![],
            status: "open".into(),
            verifier_verdict: None,
        };
        let findings = vec![
            mk("f1", "Re-reading files already in context", 3),
            mk("f2", "Re-reads files that are already loaded", 2), // a paraphrase
            mk("f3", "No verification after edits", 4),
        ];

        let out = dedup_findings(&store, findings.clone());

        // Degraded ⇒ nothing collapsed, all three kept in original order.
        assert_eq!(out.len(), 3);
        let ids: Vec<&str> = out.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["f1", "f2", "f3"]);
    }
}
