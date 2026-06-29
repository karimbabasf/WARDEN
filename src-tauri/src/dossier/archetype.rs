//! Heuristic project-archetype classifier (dimension 6 — the Phase-4 fallback).
//!
//! Dimension 6 asks "what does this operator build, and how does their behavior
//! shift per kind of project?". The *real* implementation (Phase 4) clusters
//! on-device embeddings of sessions; this module is the deterministic floor the
//! spec calls for when embeddings prove heavy or low-value — a keyword classifier
//! over the project name.
//!
//! Pipeline: group every [`FeatureVector`] by its `project` (a `None` project
//! becomes the literal `"unknown"`), assign each *project* exactly one archetype
//! by the first matching keyword set (priority order below), then roll the
//! projects up per archetype into [`ProjectArchetype`]s carrying the member
//! project names (deduped), the total session count (one feature row = one
//! session), and a short human note. Output is sorted by session count desc,
//! ties broken by archetype name, so it is fully deterministic.
//!
//! Keyword matching is a case-insensitive substring test, exactly as specified —
//! crude on purpose; this is a floor, not the embedding classifier. The priority
//! order disambiguates a name that matches several sets (e.g. it is checked
//! against `web_app` before `infra`).

use crate::dossier::types::ProjectArchetype;
use crate::ir::FeatureVector;
use std::collections::BTreeMap;

/// Archetype keyword table, in priority order. The first set with a matching
/// keyword wins for a given project name. `"unknown"` is the implicit fallback
/// (no keyword set) handled in [`classify_one`].
const ARCHETYPE_KEYWORDS: &[(&str, &[&str])] = &[
    ("web_app", &["web", "app", "ui", "frontend", "react", "next", "vue", "svelte", "site"]),
    ("cli", &["cli", "tool", "cmd", "bin"]),
    ("infra", &["infra", "deploy", "docker", "k8s", "terraform", "ops", "ci"]),
    ("data", &["data", "etl", "pipeline", "ml", "model", "analytics"]),
    ("viz", &["viz", "chart", "graph", "dashboard", "render"]),
    ("mobile", &["ios", "android", "mobile", "swift", "kotlin"]),
    ("docs", &["docs", "blog", "wiki"]),
];

/// Classify one project name into its archetype label. Case-insensitive
/// substring match against [`ARCHETYPE_KEYWORDS`] in priority order; no match →
/// `"unknown"`.
///
/// `pub(crate)` so the Phase-4 embedding clusterer ([`super::cluster`]) can reuse
/// the exact same keyword logic to *name* a cluster by its members' dominant
/// heuristic archetype — keeping one source of truth for archetype labels.
pub(crate) fn classify_one(project: &str) -> &'static str {
    let lower = project.to_lowercase();
    for (label, keywords) in ARCHETYPE_KEYWORDS {
        if keywords.iter().any(|kw| lower.contains(kw)) {
            return label;
        }
    }
    "unknown"
}

/// A running rollup for one archetype: distinct project names (insertion-stable
/// via a `BTreeMap`) and the total session count across them.
#[derive(Default)]
struct Bucket {
    projects: BTreeMap<String, ()>,
    session_count: u32,
}

/// Group `features` by project, assign each project an archetype, and roll up
/// into [`ProjectArchetype`]s sorted by session count desc (ties → archetype
/// name asc). A `None` project counts under `"unknown"`.
pub fn classify_archetypes(features: &[FeatureVector]) -> Vec<ProjectArchetype> {
    // First fold features → per-project session counts (one feature = one
    // session). BTreeMap keeps project iteration deterministic.
    let mut per_project: BTreeMap<String, u32> = BTreeMap::new();
    for fv in features {
        let project = fv.project.clone().unwrap_or_else(|| "unknown".to_string());
        *per_project.entry(project).or_insert(0) += 1;
    }

    // Then fold projects → per-archetype buckets.
    let mut buckets: BTreeMap<&'static str, Bucket> = BTreeMap::new();
    for (project, count) in per_project {
        let label = classify_one(&project);
        let bucket = buckets.entry(label).or_default();
        bucket.projects.insert(project, ());
        bucket.session_count += count;
    }

    let mut out: Vec<ProjectArchetype> = buckets
        .into_iter()
        .map(|(archetype, bucket)| {
            let projects: Vec<String> = bucket.projects.into_keys().collect();
            let note = format!(
                "{} project{}, {} session{}",
                projects.len(),
                if projects.len() == 1 { "" } else { "s" },
                bucket.session_count,
                if bucket.session_count == 1 { "" } else { "s" },
            );
            ProjectArchetype {
                archetype: archetype.to_string(),
                projects,
                session_count: bucket.session_count,
                note,
            }
        })
        .collect();

    // Sort by session count desc; deterministic tiebreak on archetype name asc.
    out.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| a.archetype.cmp(&b.archetype))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(project: Option<&str>) -> FeatureVector {
        FeatureVector {
            session_id: "s".into(),
            project: project.map(|p| p.to_string()),
            ..Default::default()
        }
    }

    /// The spec golden: 4 features across acme-web-app (x2), deploy-infra (x1),
    /// and a None project → web_app(2 sessions), infra(1), unknown(1), sorted by
    /// session count desc then archetype name.
    #[test]
    fn classifies_and_rolls_up_golden() {
        let features = vec![
            feat(Some("acme-web-app")),
            feat(Some("deploy-infra")),
            feat(Some("acme-web-app")),
            feat(None),
        ];
        let out = classify_archetypes(&features);

        assert_eq!(out.len(), 3);

        // web_app first (2 sessions), one distinct project.
        assert_eq!(out[0].archetype, "web_app");
        assert_eq!(out[0].session_count, 2);
        assert_eq!(out[0].projects, vec!["acme-web-app".to_string()]);
        assert_eq!(out[0].note, "1 project, 2 sessions");

        // infra and unknown both have 1 session; tiebreak puts infra before unknown.
        assert_eq!(out[1].archetype, "infra");
        assert_eq!(out[1].session_count, 1);
        assert_eq!(out[1].projects, vec!["deploy-infra".to_string()]);
        assert_eq!(out[1].note, "1 project, 1 session");

        assert_eq!(out[2].archetype, "unknown");
        assert_eq!(out[2].session_count, 1);
        assert_eq!(out[2].projects, vec!["unknown".to_string()]);
    }

    /// A None project lands under the literal "unknown" archetype.
    #[test]
    fn none_project_is_unknown() {
        let out = classify_archetypes(&[feat(None)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].archetype, "unknown");
        assert_eq!(out[0].projects, vec!["unknown".to_string()]);
    }

    /// Each keyword family resolves as documented (spot-check one keyword each).
    #[test]
    fn keyword_families_resolve() {
        assert_eq!(classify_one("my-react-frontend"), "web_app");
        assert_eq!(classify_one("rust-cli"), "cli");
        assert_eq!(classify_one("terraform-stack"), "infra");
        assert_eq!(classify_one("etl-pipeline"), "data");
        assert_eq!(classify_one("metrics-dashboard"), "viz");
        assert_eq!(classify_one("ios-keyboard"), "mobile");
        assert_eq!(classify_one("project-wiki"), "docs");
        assert_eq!(classify_one("zzz-mystery"), "unknown");
    }

    /// Priority order: a name matching several sets takes the highest-priority
    /// one. "web" (web_app) outranks "ops" (infra) here.
    #[test]
    fn priority_order_breaks_multi_matches() {
        // contains "web" (web_app, prio 0) AND "ops" (infra, prio 2) -> web_app.
        assert_eq!(classify_one("webops-console"), "web_app");
    }

    /// Distinct projects of the same archetype merge into one entry, sessions sum,
    /// project list deduped and sorted.
    #[test]
    fn distinct_projects_same_archetype_merge() {
        let out = classify_archetypes(&[
            feat(Some("alpha-app")),
            feat(Some("beta-ui")),
            feat(Some("alpha-app")),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].archetype, "web_app");
        assert_eq!(out[0].session_count, 3);
        assert_eq!(out[0].projects, vec!["alpha-app".to_string(), "beta-ui".to_string()]);
        assert_eq!(out[0].note, "2 projects, 3 sessions");
    }

    /// Empty input yields no archetypes (no panic).
    #[test]
    fn empty_features_yield_nothing() {
        assert!(classify_archetypes(&[]).is_empty());
    }
}
