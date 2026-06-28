//! DOSSIER Phase 4 — on-device embedding clustering + semantic dedup.
//!
//! This is the *real* implementation of dimension 6 (project archetypes) that
//! [`super::archetype`] is the deterministic floor for. It embeds each project's
//! descriptor with a small on-device ONNX model (BGE-small-en-v1.5 via
//! `fastembed`), clusters the vectors with a hand-rolled cosine k-means (no
//! `linfa`/`sqlite-vec` — the build stays light), and names each cluster by its
//! members' dominant *heuristic* archetype so the labels stay a single source of
//! truth with [`super::archetype::classify_one`].
//!
//! ## Safety net (the whole reason this module returns `Result`)
//! Embeddings are an *enhancement*, never a hard dependency. The model download
//! or load can fail (offline, no HF cache); when it does, every public function
//! degrades:
//!   * [`embed_texts`] / [`cluster_archetypes`] return `Err`, and the caller
//!     ([`super::build::build_profile`]) falls back to the keyword heuristic.
//!   * [`semantic_dedup`] is best-effort and *never* errors — on any embedding
//!     failure it returns each title as its own singleton group (i.e. "no two
//!     titles are near-duplicates", the safe identity for a dedup pass).
//!
//! ## What's unit-testable without the model
//! The vector math — [`cosine`], [`kmeans_cosine`], [`dedup_by_cosine`] — is
//! pure and tested with injected vectors (no network, no model). The two
//! model-touching tests are `#[ignore]`d so CI without a HF cache stays green;
//! run them manually with `cargo test -- --ignored`.

use std::sync::{Mutex, OnceLock};

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use sha2::{Digest, Sha256};

use crate::dossier::archetype::classify_one;
use crate::dossier::types::ProjectArchetype;

/// The embedding model id — also the `model` tag stored alongside every cached
/// vector (so a model swap is a clean cache miss in `dossier_embeddings`).
pub const MODEL_NAME: &str = "BGESmallENV15";

/// Cosine-similarity threshold above which two finding titles are considered
/// near-duplicates by [`semantic_dedup`]. 0.85 is conservative: paraphrases of
/// the same hole group, unrelated holes stay apart.
pub const DEDUP_THRESHOLD: f32 = 0.85;

/// Process-wide, lazily-initialized embedding model.
///
/// Built at most once. fastembed is synchronous and `embed` takes `&mut self`,
/// so the model lives behind a `Mutex`. We store the init *result* (not the bare
/// model) so a failed load is remembered as an `Err(String)` and every
/// subsequent call degrades immediately instead of re-attempting a doomed
/// download on every profile build.
static MODEL: OnceLock<Result<Mutex<TextEmbedding>, String>> = OnceLock::new();

/// Where the ONNX model weights are cached on disk.
///
/// fastembed's default is `./.fastembed_cache` (relative to the CWD) — which on
/// this app means littering the repo / working dir with ~130 MB of weights. We
/// pin it to a stable WARDEN-owned dir under the user's home instead, mirroring
/// the `~/.warden/…` convention used by `util::default_db_path` &c. An explicit
/// `FASTEMBED_CACHE_DIR` (set by a user or a test) always wins — we only fill in
/// the default when it is unset. Same env-helper shape as `util.rs`.
fn ensure_model_cache_dir() {
    if std::env::var_os("FASTEMBED_CACHE_DIR").is_none() {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".warden/fastembed_cache");
        std::env::set_var("FASTEMBED_CACHE_DIR", dir);
    }
}

/// Initialize (once) and borrow the shared model, or return the remembered
/// init error. Network/model-load failures surface here as `Err`.
fn model() -> Result<&'static Mutex<TextEmbedding>, anyhow::Error> {
    let slot = MODEL.get_or_init(|| {
        ensure_model_cache_dir();
        TextEmbedding::try_new(TextInitOptions::new(EmbeddingModel::BGESmallENV15))
            .map(Mutex::new)
            .map_err(|e| e.to_string())
    });
    match slot {
        Ok(m) => Ok(m),
        Err(e) => Err(anyhow::anyhow!("embedding model unavailable: {e}")),
    }
}

/// Stable content hash for a text — the `content_hash` PK in `dossier_embeddings`.
/// Public so the build/store layer keys the embedding cache identically.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Embed `texts` into dense vectors with the shared on-device model.
///
/// Lazy/cached model init via [`model`]; on a model-load failure returns `Err`
/// (the caller degrades). The returned `Vec` is 1:1 with `texts` in order. An
/// empty input is an empty output (no model touched).
pub fn embed_texts(texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let model = model()?;
    let mut guard = model
        .lock()
        .map_err(|_| anyhow::anyhow!("embedding model mutex poisoned"))?;
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let vectors = guard.embed(refs, None)?;
    Ok(vectors)
}

/// Cosine similarity of two equal-length vectors. Returns `0.0` for a
/// zero-magnitude vector (no direction ⇒ no similarity) rather than `NaN`.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Deterministic cosine k-means over `vectors` → a cluster id per vector.
///
/// Pure and model-free (vectors are injected), so it is unit-tested directly.
/// Determinism is load-bearing — the whole DOSSIER profile is cached by content
/// hash, so the same inputs must always yield the same clusters:
///   * centroids are seeded by an evenly-strided pick across the input order
///     (not random), and
///   * ties in nearest-centroid assignment break toward the lower cluster id.
/// `k` is clamped to `1..=vectors.len()`. Returns one id in `0..k` per vector.
pub fn kmeans_cosine(vectors: &[Vec<f32>], k: usize) -> Vec<usize> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.clamp(1, n);
    if k == 1 {
        return vec![0; n];
    }

    // Deterministic seeding: evenly-strided picks across the input order.
    let stride = (n as f32 / k as f32).max(1.0);
    let mut centroids: Vec<Vec<f32>> = (0..k)
        .map(|i| vectors[((i as f32 * stride) as usize).min(n - 1)].clone())
        .collect();

    let mut assignments = vec![0usize; n];
    // Bounded iterations: cosine k-means converges fast; the cap also guarantees
    // termination regardless of input pathologies.
    for _ in 0..50 {
        let mut changed = false;

        // Assignment step: nearest centroid by cosine (ties → lower id).
        for (vi, v) in vectors.iter().enumerate() {
            let mut best = 0usize;
            let mut best_sim = f32::NEG_INFINITY;
            for (ci, c) in centroids.iter().enumerate() {
                let sim = cosine(v, c);
                if sim > best_sim {
                    best_sim = sim;
                    best = ci;
                }
            }
            if assignments[vi] != best {
                assignments[vi] = best;
                changed = true;
            }
        }

        // Update step: each centroid = mean of its members (empty cluster keeps
        // its prior centroid so it stays a valid, reachable target).
        let dim = vectors[0].len();
        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        for (vi, v) in vectors.iter().enumerate() {
            let c = assignments[vi];
            counts[c] += 1;
            for (d, &val) in v.iter().enumerate() {
                sums[c][d] += val;
            }
        }
        for ci in 0..k {
            if counts[ci] > 0 {
                for d in 0..dim {
                    centroids[ci][d] = sums[ci][d] / counts[ci] as f32;
                }
            }
        }

        if !changed {
            break;
        }
    }

    assignments
}

/// Group indices of `vectors` whose pairwise cosine similarity is ≥ `threshold`.
///
/// Pure, model-free, deterministic. Single-link grouping by a stable scan: walk
/// indices in order; each unassigned index opens a new group and pulls in every
/// later unassigned index within `threshold`. Returns groups in first-seen
/// order; every index appears in exactly one group (a unique vector is its own
/// singleton group). Used by [`semantic_dedup`].
pub fn dedup_by_cosine(vectors: &[Vec<f32>], threshold: f32) -> Vec<Vec<usize>> {
    let n = vectors.len();
    let mut assigned = vec![false; n];
    let mut groups: Vec<Vec<usize>> = Vec::new();

    for i in 0..n {
        if assigned[i] {
            continue;
        }
        assigned[i] = true;
        let mut group = vec![i];
        for j in (i + 1)..n {
            if !assigned[j] && cosine(&vectors[i], &vectors[j]) >= threshold {
                assigned[j] = true;
                group.push(j);
            }
        }
        groups.push(group);
    }

    groups
}

/// Embedding-clustered project archetypes — the Phase-4 upgrade of
/// [`super::archetype::classify_archetypes`], returning the *same* output type.
///
/// `project_texts` is one `(project, descriptor)` pair per distinct project. We
/// embed the descriptors, k-means them into `min(6, n)` clusters, and name each
/// cluster by the dominant heuristic archetype of its member project *names*
/// (reusing [`classify_one`], so labels never drift from the floor). Each
/// returned [`ProjectArchetype`] carries its member project names (deduped,
/// sorted) and a `session_count` of the number of member projects — the build
/// layer, which owns the real per-project session totals, reconciles
/// `session_count`/`note` after clustering. Output is sorted by `session_count`
/// desc, ties broken by archetype name asc — identical ordering to the heuristic.
///
/// Returns `Err` if the model is unavailable (caller falls back to the
/// heuristic). Empty input ⇒ empty output (no model touched).
pub fn cluster_archetypes(
    project_texts: &[(String, String)],
) -> anyhow::Result<Vec<ProjectArchetype>> {
    if project_texts.is_empty() {
        return Ok(Vec::new());
    }

    let descriptors: Vec<String> = project_texts.iter().map(|(_, d)| d.clone()).collect();
    let vectors = embed_texts(&descriptors)?;
    let k = project_texts.len().min(6);
    let assignments = kmeans_cosine(&vectors, k);

    // Fold members into clusters, then collapse clusters sharing the same
    // dominant heuristic label (so "two web_app clusters" present as one
    // archetype, matching the heuristic's per-archetype rollup shape).
    use std::collections::BTreeMap;
    let mut by_label: BTreeMap<&'static str, BTreeMap<String, ()>> = BTreeMap::new();
    for (idx, (project, _)) in project_texts.iter().enumerate() {
        let cluster = assignments[idx];
        // Dominant label for this vector's cluster = the most common
        // classify_one over the cluster's member project names.
        let label = dominant_label(project_texts, &assignments, cluster);
        by_label
            .entry(label)
            .or_default()
            .insert(project.clone(), ());
    }

    let mut out: Vec<ProjectArchetype> = by_label
        .into_iter()
        .map(|(archetype, projects)| {
            let projects: Vec<String> = projects.into_keys().collect();
            let session_count = projects.len() as u32;
            let note = format!(
                "{} project{} (embedding-clustered)",
                projects.len(),
                if projects.len() == 1 { "" } else { "s" },
            );
            ProjectArchetype {
                archetype: archetype.to_string(),
                projects,
                session_count,
                note,
            }
        })
        .collect();

    // Sort by session count desc; deterministic tiebreak on archetype name asc
    // (matches classify_archetypes' ordering contract).
    out.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| a.archetype.cmp(&b.archetype))
    });

    Ok(out)
}

/// The most common heuristic archetype label among the member project *names* of
/// one k-means `cluster`. Ties break toward the alphabetically-first label
/// (deterministic). A cluster always has ≥1 member (the vector that anchors it).
fn dominant_label(
    project_texts: &[(String, String)],
    assignments: &[usize],
    cluster: usize,
) -> &'static str {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    for (idx, (project, _)) in project_texts.iter().enumerate() {
        if assignments[idx] == cluster {
            *counts.entry(classify_one(project)).or_insert(0) += 1;
        }
    }
    // BTreeMap iterates label-name asc, so `max_by_key` with a stable scan yields
    // the alphabetically-first label among those tied for the highest count.
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(label, _)| label)
        .unwrap_or("unknown")
}

/// Group near-duplicate finding titles by cosine similarity ≥ `threshold`.
///
/// Returns index groups into `finding_titles` (each title in exactly one group).
/// **Best-effort and never errors**: if the model is unavailable, returns each
/// title as its own singleton group — the safe identity for a dedup pass (no two
/// titles collapse), so a degraded environment simply shows every finding.
pub fn semantic_dedup(finding_titles: &[String], threshold: f32) -> Vec<Vec<usize>> {
    if finding_titles.is_empty() {
        return Vec::new();
    }
    match embed_texts(finding_titles) {
        Ok(vectors) => dedup_by_cosine(&vectors, threshold),
        Err(err) => {
            tracing::warn!("semantic_dedup degraded (embedding unavailable: {err}); no titles merged");
            (0..finding_titles.len()).map(|i| vec![i]).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cosine: identical direction ⇒ 1.0; orthogonal ⇒ 0.0; opposite ⇒ -1.0;
    /// a zero vector ⇒ 0.0 (not NaN).
    #[test]
    fn cosine_basic_cases() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    /// k-means over two tight, well-separated bundles puts each bundle in its own
    /// cluster, and the result is deterministic across runs (same inputs ⇒ same
    /// assignment vector).
    #[test]
    fn kmeans_separates_two_bundles_deterministically() {
        let vectors = vec![
            vec![1.0, 0.0],
            vec![0.98, 0.02],
            vec![0.0, 1.0],
            vec![0.02, 0.98],
        ];
        let a = kmeans_cosine(&vectors, 2);
        let b = kmeans_cosine(&vectors, 2);
        assert_eq!(a, b, "k-means must be deterministic for cache stability");
        // The two members of each bundle share a cluster; the bundles differ.
        assert_eq!(a[0], a[1], "first bundle co-clustered");
        assert_eq!(a[2], a[3], "second bundle co-clustered");
        assert_ne!(a[0], a[2], "the two bundles are in different clusters");
    }

    /// k is clamped: k=1 (or k>n) never panics and yields a single valid cluster.
    #[test]
    fn kmeans_clamps_k() {
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        assert_eq!(kmeans_cosine(&vectors, 1), vec![0, 0]);
        // k > n is clamped to n; every id stays in 0..n.
        let a = kmeans_cosine(&vectors, 10);
        assert!(a.iter().all(|&c| c < 2));
        assert!(kmeans_cosine(&[], 3).is_empty());
    }

    /// dedup groups near-duplicates (cosine ≥ threshold) and keeps distinct
    /// vectors apart — tested with injected vectors, no model.
    #[test]
    fn dedup_groups_near_duplicates() {
        let vectors = vec![
            vec![1.0, 0.0],   // 0
            vec![0.99, 0.01], // 1 — near-dup of 0
            vec![0.0, 1.0],   // 2 — distinct
        ];
        let groups = dedup_by_cosine(&vectors, 0.85);
        assert_eq!(groups, vec![vec![0, 1], vec![2]]);
    }

    /// Every index lands in exactly one group, and a fully-distinct set yields all
    /// singletons (the identity a degraded model also produces).
    #[test]
    fn dedup_partitions_all_indices() {
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![-1.0, 0.0]];
        let groups = dedup_by_cosine(&vectors, 0.95);
        assert_eq!(groups, vec![vec![0], vec![1], vec![2]]);
        let total: usize = groups.iter().map(|g| g.len()).sum();
        assert_eq!(total, 3, "partition covers every index once");
    }

    /// semantic_dedup on empty input is empty (no model touched) — proves the
    /// best-effort wrapper short-circuits before any embedding attempt.
    #[test]
    fn semantic_dedup_empty_is_empty() {
        assert!(semantic_dedup(&[], DEDUP_THRESHOLD).is_empty());
    }

    /// content_hash is stable and content-sensitive.
    #[test]
    fn content_hash_is_stable_and_sensitive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
    }

    /// dominant_label picks the most common heuristic label in a cluster, ties to
    /// the alphabetically-first label. Cluster 0 here is all web_app names.
    #[test]
    fn dominant_label_picks_majority() {
        let pt = vec![
            ("acme-web-app".to_string(), "d".to_string()),
            ("beta-ui".to_string(), "d".to_string()),
            ("deploy-infra".to_string(), "d".to_string()),
        ];
        let assignments = vec![0, 0, 1];
        assert_eq!(dominant_label(&pt, &assignments, 0), "web_app");
        assert_eq!(dominant_label(&pt, &assignments, 1), "infra");
    }

    // ---- model-dependent (network/HF cache); #[ignore]d so CI stays green ----

    /// Smoke: the model loads and embeds two strings into equal-dim vectors.
    /// Run manually: `cargo test -- --ignored embed_texts_smoke`.
    #[test]
    #[ignore = "requires the BGE-small ONNX model (network/HF cache)"]
    fn embed_texts_smoke() {
        let out = embed_texts(&["hello world".to_string(), "goodbye world".to_string()]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), out[1].len());
        assert!(out[0].len() > 100, "BGE-small is 384-dim");
    }

    /// End-to-end: clustering real project descriptors separates a web bundle
    /// from an infra bundle and labels them via the heuristic.
    /// Run manually: `cargo test -- --ignored cluster_archetypes_smoke`.
    #[test]
    #[ignore = "requires the BGE-small ONNX model (network/HF cache)"]
    fn cluster_archetypes_smoke() {
        let pt = vec![
            ("acme-web".to_string(), "a react frontend single page web app".to_string()),
            ("shop-ui".to_string(), "a vue storefront user interface".to_string()),
            ("deploy-infra".to_string(), "kubernetes terraform deployment infrastructure".to_string()),
            ("ops-ci".to_string(), "docker ci pipeline ops automation".to_string()),
        ];
        let out = cluster_archetypes(&pt).unwrap();
        // Both archetypes present; every project accounted for exactly once.
        let total_projects: usize = out.iter().map(|a| a.projects.len()).sum();
        assert_eq!(total_projects, 4);
        let labels: Vec<&str> = out.iter().map(|a| a.archetype.as_str()).collect();
        assert!(labels.contains(&"web_app"), "got labels: {labels:?}");
        assert!(labels.contains(&"infra"), "got labels: {labels:?}");
    }
}
