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

use chrono::{DateTime, Datelike, Utc};
use sha2::{Digest, Sha256};
use tauri::Emitter;

use crate::brain::Brain;
use crate::dossier::aggregate::aggregate_window;
use crate::dossier::archetype::classify_archetypes;
use crate::dossier::efficiency::window_efficiency;
use crate::dossier::proof::gate_trend;
use crate::dossier::scope::{scoped_sessions, Window};
use crate::dossier::summarize::{
    project_rollup, session_summary_via_brain, week_rollup, ProjectSummary, WeekSummary,
};
use crate::dossier::synthesize::{synthesize_profile_via_brain, SynthesisInputs};
use crate::dossier::trajectory::classify_trend;
use crate::dossier::types::{Profile, TrendPoint};
use crate::detectors::nominate_windowed;
use crate::ir::{FeatureVector, Session};
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
    let aggregate = aggregate_window(store, window, now)?;
    let efficiency = window_efficiency(store, window, now)?;
    let archetypes = classify_archetypes(&features);
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
    let findings = nominate_windowed(store, &store.profile()?, cutoff)?;

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
}
