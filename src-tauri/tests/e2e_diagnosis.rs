//! End-to-end diagnosis test (Task 10, spec §12 DoD-4).
//!
//! Seeds an in-memory `Store` with BOTH a Claude Code session and a Codex
//! session, then drives the REAL detector-only diagnosis path (no API key) all
//! the way to a persisted, ranked `Diagnosis`. This is the offline production
//! path: `Brain::run_pipeline` computes features, nominates deterministic
//! candidates, and — finding no brain API credentials — assembles + saves the
//! detector-only diagnosis without any network call.
//!
//! Assertions (the spec's "evidence-cited, harness-aware diagnosis" guarantees):
//!   1. the diagnosis is non-empty and ranked (severity-ordered);
//!   2. every finding carries `evidence`, and each `EvidenceRef` resolves to a
//!      REAL stored event/quote — either an inline quote or, when the quote is
//!      null, a ground-truth excerpt recoverable via `store.event_text`;
//!   3. the findings' evidence resolves to a harness via `store.session_harness`,
//!      and BOTH Claude and Codex are represented across the diagnosis;
//!   4. `candidates_nominated`-shaped data (pattern + session + harness +
//!      severity per candidate) is producible from the nominated findings.
//!
//! Runs as its own integration binary, so the env removal below cannot race the
//! in-crate `#[cfg(test)]` unit tests (separate process).

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;
use warden_lib::brain::Brain;
use warden_lib::detectors;
use warden_lib::featurizer;
use warden_lib::ingest::codex::CodexAdapter;
use warden_lib::ingest::Adapter;
use warden_lib::ir::*;
use warden_lib::store::Store;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Seed one search-heavy, unverified session of `harness` through the REAL
/// `upsert_session_batch` path. `n_search` `Read` ToolCalls in the main context
/// with zero subagent spawns and no verification command trip the deterministic
/// CONTEXT_BLOAT / NO_DELEGATION / UNVERIFIED_COMPLETION detectors, and each
/// ToolCall is a real stored event the evidence drill-down can resolve.
fn seed_search_heavy(store: &Store, id: &str, harness: Harness, source: &str, n_search: usize) {
    let now = Utc::now();
    let source_path = PathBuf::from(source);
    let session = Session {
        id: id.into(),
        harness,
        external_id: id.into(),
        project: Some(ProjectRef {
            cwd: PathBuf::from(format!("/work/{id}")),
            repo_root: None,
            git_branch: None,
        }),
        model_ids: vec![],
        started_at: now,
        ended_at: None,
        source_path: source_path.clone(),
        raw_hash: 0,
        ingested_at: now,
        meta: json!({}),
    };
    let turn = Turn {
        id: format!("{id}-t0"),
        session_id: id.into(),
        parent_id: None,
        role: Role::Assistant,
        index: 0,
        started_at: now,
        duration_ms: None,
        is_sidechain: false,
    };
    let mut events = Vec::new();
    // A real user prompt (gives VAGUE/REPEATED detectors and evidence a quote).
    events.push(EventRecord {
        id: format!("{id}-prompt"),
        turn_id: turn.id.clone(),
        session_id: id.into(),
        ts: now,
        event: Event::UserPrompt {
            text: format!("look around the {id} repo and tell me what is wrong"),
            attachments: vec![],
            is_meta: false,
        },
        raw_ref: RawRef {
            source_path: source_path.clone(),
            offset: 0,
            line: 1,
        },
    });
    for i in 0..n_search {
        events.push(EventRecord {
            id: format!("{id}-read{i}"),
            turn_id: turn.id.clone(),
            session_id: id.into(),
            ts: now,
            event: Event::ToolCall {
                tool: "Read".into(),
                input: json!({ "file_path": format!("/work/{id}/src/file{i}.rs") }),
                call_id: format!("call-{i}"),
                kind: ToolKind::Builtin,
            },
            raw_ref: RawRef {
                source_path: source_path.clone(),
                offset: (i as u64 + 1) * 100,
                line: i as u32 + 2,
            },
        });
    }
    store
        .upsert_session_batch(&session, &[turn], &events, 0)
        .unwrap();
}

/// Ingest the committed Codex rollout fixture through the REAL Codex adapter, so
/// the store holds a genuine Codex session (not just a synthetic one).
fn ingest_codex_fixture(store: &Store) -> String {
    let path = fixture("codex_rollout_sample.jsonl");
    let bytes = std::fs::read(&path).expect("read codex fixture");
    let adapter = CodexAdapter::with_root(
        path.parent().unwrap().to_path_buf(),
        path.parent().unwrap().to_path_buf(),
        store.clone(),
    );
    let batches = adapter
        .parse_range(&path, &bytes, 0, warden_lib::util::hash64(&bytes))
        .expect("parse codex fixture");
    let b = &batches[0];
    assert!(matches!(b.session.harness, Harness::Codex));
    store
        .upsert_session_batch(&b.session, &b.turns, &b.events, b.offset)
        .unwrap();
    b.session.id.clone()
}

/// An `EvidenceRef` is considered resolvable when it either carries an inline
/// quote OR its `(session_id, event_id)` recovers ground-truth text from the
/// store (the exact contract the Task 9 drill-down relies on).
fn evidence_resolves(store: &Store, e: &EvidenceRef) -> bool {
    if e.quote
        .as_deref()
        .map(|q| !q.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if let Some(eid) = &e.event_id {
        if let Ok(Some((text, _))) = store.event_text(&e.session_id, eid) {
            return !text.trim().is_empty();
        }
    }
    false
}

#[tokio::test]
async fn detector_only_diagnosis_is_ranked_evidence_cited_and_harness_aware() {
    // Guarantee the offline detector-only path: Brain snapshots credentials at
    // construction, so removing every supported fallback BEFORE `Brain::new`
    // forces api_key/api_url to stay absent even on developer machines with
    // OpenAI-compatible env vars exported.
    for key in [
        "WARDEN_BRAIN_API_KEY",
        "WARDEN_BRAIN_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "SAKANA_API_KEY",
    ] {
        std::env::remove_var(key);
    }

    let store = Store::memory().unwrap();

    // Two search-heavy/unverified sessions — one per harness — so a finding's
    // evidence spans BOTH harnesses, plus a real Codex session from the fixture.
    seed_search_heavy(
        &store,
        "claude-1",
        Harness::ClaudeCode,
        "/logs/claude-1.jsonl",
        12,
    );
    seed_search_heavy(&store, "codex-1", Harness::Codex, "/logs/codex-1.jsonl", 12);
    let fixture_codex_id = ingest_codex_fixture(&store);

    // Both harnesses are present and individually resolvable in the store.
    assert_eq!(
        store.session_harness("claude-1").unwrap().as_deref(),
        Some("claude_code")
    );
    assert_eq!(
        store.session_harness("codex-1").unwrap().as_deref(),
        Some("codex")
    );
    assert_eq!(
        store.session_harness(&fixture_codex_id).unwrap().as_deref(),
        Some("codex")
    );

    // ── lowest-level Diagnosis path: run the real pipeline with no key. ──
    let brain = Brain::new(store.clone());
    assert!(
        !brain.available(),
        "test must run the offline detector-only path (no API key)"
    );
    let diagnosis = brain
        .run_pipeline(RunScope {
            harness: None,
            query: Some("what's wrong with how I use my agents?".into()),
            force: None,
            max_files: None,
        })
        .await
        .expect("run_pipeline (detector-only) succeeds offline");

    // 1 — non-empty + detector-only + ranked (non-increasing severity).
    assert!(
        diagnosis.detector_only,
        "no-key pipeline must yield a detector-only diagnosis"
    );
    assert!(
        !diagnosis.ranked_findings.is_empty(),
        "detector-only diagnosis must be non-empty for search-heavy sessions"
    );
    for w in diagnosis.ranked_findings.windows(2) {
        assert!(
            w[0].severity >= w[1].severity,
            "ranked findings must be severity-ordered: {} then {}",
            w[0].severity,
            w[1].severity
        );
    }

    // 2 — every finding carries evidence, and every EvidenceRef resolves to a
    //     real stored event/quote.
    for f in &diagnosis.ranked_findings {
        assert!(
            !f.evidence.is_empty(),
            "finding {} must carry evidence",
            f.pattern_id
        );
        for e in &f.evidence {
            assert!(
                evidence_resolves(&store, e),
                "evidence for {} (session {}, event {:?}) must resolve to a real quote/event",
                f.pattern_id,
                e.session_id,
                e.event_id
            );
        }
    }

    // 3 — the findings' evidence resolves to harnesses, and BOTH Claude AND Codex
    //     are represented across the diagnosis (harness-differentiation guarantee).
    let mut harnesses = std::collections::BTreeSet::new();
    for f in &diagnosis.ranked_findings {
        for e in &f.evidence {
            if let Some(h) = store.session_harness(&e.session_id).unwrap() {
                harnesses.insert(h);
            }
        }
    }
    assert!(
        harnesses.contains("claude_code"),
        "diagnosis evidence must reference at least one Claude session; got {harnesses:?}"
    );
    assert!(
        harnesses.contains("codex"),
        "diagnosis evidence must reference at least one Codex session; got {harnesses:?}"
    );

    // 4 — `candidates_nominated`-shaped data is producible: re-nominate from the
    //     same profile and confirm each candidate yields (pattern, session,
    //     resolvable harness, severity) — the exact tuple the war-room consumes.
    let profile = featurizer::compute_all(&store).unwrap();
    let candidates = detectors::nominate(&store, &profile).unwrap();
    assert!(
        !candidates.is_empty(),
        "detectors must nominate at least one candidate"
    );
    let mut candidate_harnesses = std::collections::BTreeSet::new();
    for c in &candidates {
        // Every candidate yields the `candidates_nominated` tuple the war-room
        // consumes: a pattern, a severity in 1..=5, and a session whose harness
        // resolves (the payload keys are pattern_id / session_id / harness /
        // severity_hint — see brain::candidates_payload).
        let first = c
            .evidence
            .first()
            .expect("each candidate finding carries at least one evidence ref");
        assert!(!c.pattern_id.is_empty());
        assert!((1..=5).contains(&c.severity));
        assert!(
            store.session_harness(&first.session_id).unwrap().is_some(),
            "candidate {}'s lead session {} must resolve to a harness",
            c.pattern_id,
            first.session_id
        );
        // Tally harnesses across ALL of a candidate's evidence sessions (the
        // detectors cite up to 12 affected sessions per pattern; the war-room
        // spawns a node per candidate-session, so both harnesses surface here).
        for e in &c.evidence {
            if let Some(h) = store.session_harness(&e.session_id).unwrap() {
                candidate_harnesses.insert(h);
            }
        }
    }
    assert!(
        candidate_harnesses.contains("claude_code") && candidate_harnesses.contains("codex"),
        "nominated candidates must span both harnesses; got {candidate_harnesses:?}"
    );

    // The diagnosis round-trips through the store (run_pipeline persisted it).
    let reloaded = store
        .latest_diagnosis()
        .unwrap()
        .expect("run_pipeline persisted the diagnosis");
    assert_eq!(reloaded.id, diagnosis.id);
    assert_eq!(
        reloaded.ranked_findings.len(),
        diagnosis.ranked_findings.len()
    );
}

/// Guards the helper contract used above: `seed_search_heavy` writes events whose
/// `(session_id, event_id)` the store can resolve to ground truth — so a
/// null-quote EvidenceRef pointing at one of these events still resolves.
#[test]
fn seeded_events_are_resolvable_ground_truth() {
    let store = Store::memory().unwrap();
    seed_search_heavy(&store, "s", Harness::ClaudeCode, "/logs/s.jsonl", 3);
    let (text, source) = store
        .event_text("s", "s-read1")
        .unwrap()
        .expect("seeded ToolCall event is stored");
    assert!(
        text.contains("Read"),
        "tool-call text is searchable: {text}"
    );
    assert_eq!(source.as_deref(), Some(Path::new("/logs/s.jsonl")));
}
