//! DOSSIER Phase 3 — hierarchical summarization orchestration + caching.
//!
//! Implements the cheap tiers of the map-reduce in spec §9:
//!   - **Per-session micro-summary** ([`session_summary`]) — computed once per
//!     session and cached forever keyed by the session's content hash
//!     (`sessions.raw_hash`) + [`SUMMARIZER_VERSION`]. Reused across every window
//!     and every future profile, so the marginal cost of a new session is ≈ one
//!     small summary call. The actual LLM call is **injected** as a closure so
//!     the cache logic is unit-testable WITHOUT any network.
//!   - **Per-week / per-project rollup** ([`week_rollup`] / [`project_rollup`]) —
//!     fold the per-session micro-summaries into per-week / per-project digests.
//!     Pure + deterministic for v1 (concatenation/templating, length-capped); no
//!     LLM reduce yet.
//!
//! The expensive window-level synthesis (spec §11) is a later phase and lives
//! elsewhere. Privacy (spec §19) is enforced at the LLM boundary in
//! [`crate::brain::Brain::summarize_session`]: only distilled signals + scrubbed
//! snippets ever leave the device — never the raw transcript.

use crate::brain::Brain;
use crate::ir::{FeatureVector, Session};
use crate::store::Store;
use anyhow::Result;
use std::collections::BTreeMap;

/// Version tag for the per-session summarizer. Bump this whenever the summary
/// prompt or distillation changes so every cached summary is invalidated and
/// re-summarized under the new prompt (see [`Store::dossier_summary_get`]).
pub const SUMMARIZER_VERSION: &str = "dossier-sum-v1";

/// Max characters for a rolled-up digest (week or project). Keeps the reduce
/// tier bounded so a busy week of 50 sessions does not blow up the eventual
/// window-synthesis prompt.
const ROLLUP_CHAR_CAP: usize = 1_200;

/// A per-week digest of the session micro-summaries that fell in that ISO week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeekSummary {
    /// ISO-week label, e.g. `"2026-W24"` (same format as `aggregate::WeekBin`).
    pub iso_week: String,
    /// Number of session summaries folded into this week.
    pub session_count: usize,
    /// Concatenated, length-capped digest of the week's session summaries.
    pub digest: String,
}

/// A per-project digest of the session micro-summaries for that project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSummary {
    /// Project key (the `FeatureVector::project` string, typically the cwd).
    pub project: String,
    /// Number of session summaries folded into this project.
    pub session_count: usize,
    /// Concatenated, length-capped digest of the project's session summaries.
    pub digest: String,
}

/// Per-session micro-summary with caching, LLM source **injected**.
///
/// Checks [`Store::dossier_summary_get`] under `raw_hash` + [`SUMMARIZER_VERSION`]:
///   - **hit** ⇒ return the cached summary and NEVER call `summarize`.
///   - **miss** ⇒ `summarize().await`, persist via [`Store::dossier_summary_put`],
///     and return.
///
/// `raw_hash` is the session's content hash stringified (`session.raw_hash`),
/// the same key the cache invalidates on. Injecting the summary source as a
/// closure keeps the cache logic testable with no network.
pub async fn session_summary<F, Fut>(
    store: &Store,
    session: &Session,
    _features: &FeatureVector,
    raw_hash: &str,
    summarize: F,
) -> Result<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    if let Some(cached) = store.dossier_summary_get(&session.id, raw_hash, SUMMARIZER_VERSION)? {
        return Ok(cached);
    }
    let summary = summarize().await?;
    store.dossier_summary_put(&session.id, raw_hash, SUMMARIZER_VERSION, &summary)?;
    Ok(summary)
}

/// Thin real wrapper over [`session_summary`] that injects a closure calling the
/// brain's per-session summarizer. The cache short-circuits before the brain is
/// ever touched, so an unchanged session costs zero tokens.
pub async fn session_summary_via_brain(
    store: &Store,
    brain: &Brain,
    session: &Session,
    features: &FeatureVector,
    sample_prompts: &[String],
) -> Result<String> {
    let raw_hash = session.raw_hash.to_string();
    session_summary(store, session, features, &raw_hash, || async {
        brain.summarize_session(session, features, sample_prompts).await
    })
    .await
}

/// Fold per-session summaries into per-week digests. Pure + deterministic:
/// groups by `iso_week`, concatenates the member summaries in input order, caps
/// the digest length, and returns weeks sorted by label.
pub fn week_rollup(summaries: &[(String, String)]) -> Vec<WeekSummary> {
    group_and_fold(summaries)
        .into_iter()
        .map(|(iso_week, (session_count, digest))| WeekSummary {
            iso_week,
            session_count,
            digest,
        })
        .collect()
}

/// Fold per-session summaries into per-project digests. Pure + deterministic:
/// groups by project key, concatenates the member summaries in input order,
/// caps the digest length, and returns projects sorted by key.
pub fn project_rollup(summaries: &[(String, String)]) -> Vec<ProjectSummary> {
    group_and_fold(summaries)
        .into_iter()
        .map(|(project, (session_count, digest))| ProjectSummary {
            project,
            session_count,
            digest,
        })
        .collect()
}

/// Shared deterministic fold for both rollups: group `(key, summary)` pairs by
/// `key`, preserving input order within each group, then concatenate the member
/// summaries (newline-joined) and cap the digest at [`ROLLUP_CHAR_CAP`] chars.
/// Returns `(key -> (count, digest))` sorted by key via [`BTreeMap`].
fn group_and_fold(summaries: &[(String, String)]) -> BTreeMap<String, (usize, String)> {
    let mut grouped: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (key, summary) in summaries {
        grouped.entry(key.clone()).or_default().push(summary.as_str());
    }
    grouped
        .into_iter()
        .map(|(key, parts)| {
            let count = parts.len();
            let mut digest = parts.join("\n");
            if digest.chars().count() > ROLLUP_CHAR_CAP {
                digest = digest
                    .chars()
                    .take(ROLLUP_CHAR_CAP.saturating_sub(1))
                    .collect::<String>();
                digest.push('…');
            }
            (key, (count, digest))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FeatureVector, Harness, Session};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn seed_session(store: &Store, id: &str, raw_hash: u64) {
        let now = Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap();
        let session = Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&session, &[], &[], 0).unwrap();
    }

    fn session(id: &str, raw_hash: u64) -> Session {
        let now = Utc.with_ymd_and_hms(2026, 6, 27, 12, 0, 0).unwrap();
        Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash,
            ingested_at: now,
            meta: serde_json::json!({}),
        }
    }

    fn fv(id: &str) -> FeatureVector {
        FeatureVector {
            session_id: id.into(),
            ..FeatureVector::default()
        }
    }

    /// A pre-seeded cache MUST short-circuit before the injected source runs:
    /// the closure panics if called, and the call still returns the cached value.
    #[tokio::test]
    async fn session_summary_cache_hit_skips_call() {
        let store = Store::memory().unwrap();
        let s = session("sess-hit", 0xABCD);
        store
            .dossier_summary_put("sess-hit", "43981", SUMMARIZER_VERSION, "cached micro-summary")
            .unwrap();

        let out = session_summary(&store, &s, &fv("sess-hit"), "43981", || async {
            panic!("summarize source must NOT be called on a cache hit");
        })
        .await
        .unwrap();

        assert_eq!(out, "cached micro-summary");
    }

    /// On a cache miss the injected source runs once, its result is returned, and
    /// it is now persisted (a second lookup hits).
    #[tokio::test]
    async fn session_summary_miss_calls_and_stores() {
        let store = Store::memory().unwrap();
        seed_session(&store, "sess-miss", 0x1234);
        let s = session("sess-miss", 0x1234);
        let raw_hash = s.raw_hash.to_string(); // "4660"

        let out = session_summary(&store, &s, &fv("sess-miss"), &raw_hash, || async {
            Ok("fixed summary".to_string())
        })
        .await
        .unwrap();
        assert_eq!(out, "fixed summary");

        // Now cached under the same (raw_hash, version).
        assert_eq!(
            store
                .dossier_summary_get("sess-miss", &raw_hash, SUMMARIZER_VERSION)
                .unwrap(),
            Some("fixed summary".to_string()),
            "a miss persists the freshly-computed summary"
        );
    }

    /// Week rollup groups by ISO week, counts members, concatenates digests, and
    /// returns weeks sorted by label (deterministic).
    #[test]
    fn week_rollup_folds_by_iso_week() {
        let input = vec![
            ("2026-W25".to_string(), "did B".to_string()),
            ("2026-W24".to_string(), "did A1".to_string()),
            ("2026-W24".to_string(), "did A2".to_string()),
        ];
        let weeks = week_rollup(&input);
        assert_eq!(weeks.len(), 2);
        // Sorted by label: W24 first.
        assert_eq!(weeks[0].iso_week, "2026-W24");
        assert_eq!(weeks[0].session_count, 2);
        assert!(weeks[0].digest.contains("did A1"));
        assert!(weeks[0].digest.contains("did A2"));
        assert_eq!(weeks[1].iso_week, "2026-W25");
        assert_eq!(weeks[1].session_count, 1);
        assert_eq!(weeks[1].digest, "did B");
    }

    /// Project rollup groups by project key, counts members, concatenates, and
    /// returns projects sorted by key (deterministic).
    #[test]
    fn project_rollup_folds_by_project() {
        let input = vec![
            ("warden".to_string(), "ingest work".to_string()),
            ("aleph".to_string(), "auth work".to_string()),
            ("warden".to_string(), "radar work".to_string()),
        ];
        let projects = project_rollup(&input);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].project, "aleph");
        assert_eq!(projects[0].session_count, 1);
        assert_eq!(projects[1].project, "warden");
        assert_eq!(projects[1].session_count, 2);
        assert!(projects[1].digest.contains("ingest work"));
        assert!(projects[1].digest.contains("radar work"));
    }
}
