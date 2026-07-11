//! Top-level forest assembly: the pure, deterministic join of store sessions +
//! liveness + composition + identity into the final [`RadarState`]. Membership and
//! the clock are injected so this is unit-testable without real PIDs or a real clock;
//! the live filesystem orchestration lives in [`super::live`].

use super::agent::build_agent;
use super::identity::{display_label, subagent_terminated_at};
use super::liveness::{partition_claude, read_claude_registry, AgentStatus};
use super::model::RadarState;
use super::status::{agent_status, claude_conversation_status, transcript_mtime_secs_ago};
use crate::ir::{Harness, Session};
use crate::store::Store;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Assemble the live agent forest from the store + the Claude liveness registry.
///
/// `is_alive`/`now` are injected so the join is deterministic and unit-testable
/// without real PIDs or a real clock. The forest is the set of store sessions;
/// each becomes a [`RadarAgent`] with:
/// * `parentId`/`depth`/`childCount` from `Store::parent_of`;
/// * size + exact composition from the session's last `TokenUsage` event
///   (Tasks 7), and an estimated composition from its turn-1 baseline (Task 8);
/// * `status` from the liveness partition (Claude registry match by external id,
///   else idle) — Task 6.
///
/// Honest viz: labels/origin/nickname come straight from the session metadata; no
/// children are fabricated (a child only exists when linkage was persisted).
pub fn assemble(
    store: &Store,
    sessions_dir: &Path,
    is_alive: &dyn Fn(u32) -> bool,
    is_codex_open: &dyn Fn(&Session) -> bool,
    now: DateTime<Utc>,
) -> RadarState {
    let sessions = store.sessions().unwrap_or_default();

    // Liveness: map a Claude external session id → status from the registry.
    let registry = read_claude_registry(sessions_dir);
    let mtime_secs_ago = |sid: &str| transcript_mtime_secs_ago(&sessions, sid, now);
    // FAULT B: when the registry carries no authoritative `status`, decide working/idle
    // from the session's CONVERSATION STATE (its last ingested event), not file mtime —
    // deterministic across reads, so the working↔idle flicker is gone. The closure
    // bridges the registry's external `sessionId` → the store row → its events.
    let stale_secs = crate::util::radar_working_stale_secs();
    let fallback_status = |ext: &str| {
        claude_conversation_status(store, &sessions, ext, now, stale_secs, &mtime_secs_ago)
    };
    let live = partition_claude(&registry, is_alive, &fallback_status);
    let claude_status: HashMap<String, AgentStatus> =
        live.into_iter().map(|(s, st)| (s.session_id, st)).collect();

    // parent link per session id (None = root). Built for every stored session so
    // we can resolve a subagent's root before deciding membership.
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for s in &sessions {
        let parent = store.parent_of(&s.id).ok().flatten();
        parent_of.insert(s.id.clone(), parent);
    }

    // The OPEN FOREST: include a session ONLY if it is currently open (spec §3 "the
    // set of agent trees currently open", §5 "the live forest"). A root is directly
    // open when — Claude: its `external_id` is in the live registry partition (a dead
    // PID was already dropped by `partition_claude`); Codex: its rollout currently
    // lives under `~/.codex/sessions/` and has NOT been archived (`is_codex_open`).
    // A subagent is open iff the ROOT of its parent-chain is directly open — close
    // the root and the whole tree implodes; this also guarantees no kept subagent
    // ever dangles (its parent shares the same open root, so it is kept too).
    let directly_open = |s: &Session| -> bool {
        match s.harness {
            Harness::Codex => is_codex_open(s),
            _ => claude_status.contains_key(&s.external_id),
        }
    };
    let by_id: HashMap<&str, &Session> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let open: HashMap<String, bool> = sessions
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                root_is_open(&s.id, &parent_of, &by_id, &directly_open),
            )
        })
        .collect();
    let is_open = |id: &str| open.get(id).copied().unwrap_or(false);

    // ── dedupe: one live session = one globe ────────────────────────────────────
    // A long-running Claude session is re-ingested as SEVERAL store rows that share a
    // single `external_id` (one row per compaction/segment). They are the same live
    // agent, so the forest must show ONE globe — not one per row. Collapse each
    // external_id group of OPEN sessions to a canonical row (the freshest: latest
    // started_at, then ingested_at, then id for determinism) and remember the mapping
    // so a child whose parent link points at a dropped row is re-pointed onto it.
    let mut by_ext: HashMap<&str, Vec<&Session>> = HashMap::new();
    for s in sessions.iter().filter(|s| is_open(&s.id)) {
        by_ext.entry(s.external_id.as_str()).or_default().push(s);
    }
    let mut keep: HashSet<String> = HashSet::new();
    let mut canonical: HashMap<String, String> = HashMap::new();
    for group in by_ext.values() {
        let chosen = group
            .iter()
            .copied()
            .max_by(|a, b| {
                (!is_subagent_transcript_path(&a.source_path))
                    .cmp(&(!is_subagent_transcript_path(&b.source_path)))
                    .then(
                        a.started_at
                            .cmp(&b.started_at)
                            .then(a.ingested_at.cmp(&b.ingested_at))
                            .then(a.id.cmp(&b.id)),
                    )
            })
            .expect("group is non-empty");
        keep.insert(chosen.id.clone());
        for s in group {
            canonical.insert(s.id.clone(), chosen.id.clone());
        }
    }

    // Parent map over the KEPT set, with dropped-duplicate parents remapped onto their
    // canonical row (so a subagent linked to a collapsed root still nests under it).
    let mut kept_parent: HashMap<String, Option<String>> = HashMap::new();
    for id in &keep {
        let rp = parent_of
            .get(id)
            .cloned()
            .flatten()
            .map(|p| canonical.get(&p).cloned().unwrap_or(p))
            .filter(|p| p != id);
        kept_parent.insert(id.clone(), rp);
    }

    // ── subagent termination ─────────────────────────────────────────────────────
    // A subagent has no PID, so liveness can't see it finish. Derive it: the parent
    // logged a tool-result for the subagent's call (permanent ⇒ idempotent), or the
    // subagent fell silent past the backstop. Within a grace window we EMIT it as
    // `terminated` (the FACE implodes it); past the window we DROP it from the forest
    // so it never lingers as idle and never resurrects.
    let terminate_ms = crate::util::radar_subagent_terminate_ms();
    let grace_ms = crate::util::radar_terminate_grace_ms();
    let mut terminated_now: HashSet<String> = HashSet::new();
    let mut terminated_drop: HashSet<String> = HashSet::new();
    for id in &keep {
        let Some(Some(parent)) = kept_parent.get(id) else {
            continue; // roots are never "terminated" (they Close instead)
        };
        let Some(child) = by_id.get(id.as_str()) else {
            continue;
        };
        let tid = child.meta.get("toolUseId").and_then(|v| v.as_str());
        let parent_events = store.session_events(parent).unwrap_or_default();
        let last = mtime_secs_ago(&child.external_id)
            .map(|secs| now - chrono::Duration::seconds(secs as i64));
        if let Some(ts) = subagent_terminated_at(tid, &parent_events, last, now, terminate_ms) {
            let age_ms = now.signed_duration_since(ts).num_milliseconds().max(0) as u64;
            if age_ms <= grace_ms {
                terminated_now.insert(id.clone());
            } else {
                terminated_drop.insert(id.clone());
            }
        }
    }
    // Drop past-grace terminated subagents from the forest entirely (BEFORE counts).
    if !terminated_drop.is_empty() {
        keep.retain(|id| !terminated_drop.contains(id));
        kept_parent.retain(|id, _| !terminated_drop.contains(id));
    }

    // childCount over the kept set only (a closed/duplicate child never inflates it).
    let mut child_count: HashMap<String, u32> = HashMap::new();
    for id in &keep {
        if let Some(Some(p)) = kept_parent.get(id) {
            *child_count.entry(p.clone()).or_insert(0) += 1;
        }
    }

    // Per-parent subagent ordinals (1-based by spawn order) and per-folder root
    // disambiguators (1-based by spawn order) — both over the KEPT set so the
    // numbering is stable and never counts a dropped/closed sibling.
    let started = |id: &str| by_id.get(id).map(|s| (s.started_at, s.id.clone()));
    let mut subagent_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            if let Some(Some(p)) = kept_parent.get(id) {
                by_parent.entry(p.clone()).or_default().push(id.clone());
            }
        }
        for sibs in by_parent.values_mut() {
            sibs.sort_by_key(|id| started(id));
            for (i, id) in sibs.iter().enumerate() {
                subagent_ordinal.insert(id.clone(), (i as u32) + 1);
            }
        }
    }
    let mut root_dup_ordinal: HashMap<String, u32> = HashMap::new();
    {
        let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
        for id in &keep {
            let is_root = kept_parent.get(id).map(|p| p.is_none()).unwrap_or(true);
            if !is_root {
                continue;
            }
            let folder = by_id
                .get(id.as_str())
                .and_then(|s| s.project.as_ref())
                .and_then(|p| p.cwd.file_name())
                .map(|n| n.to_string_lossy().to_string());
            if let Some(folder) = folder {
                by_folder.entry(folder).or_default().push(id.clone());
            }
        }
        for roots in by_folder.values_mut() {
            if roots.len() < 2 {
                continue; // a lone root keeps its bare folder name
            }
            roots.sort_by_key(|id| started(id));
            for (i, id) in roots.iter().enumerate() {
                root_dup_ordinal.insert(id.clone(), (i as u32) + 1);
            }
        }
    }

    // Build one agent per kept session. Depth is the parent-chain length within the
    // (kept) tree (root = 0). Iterate `sessions` for a stable, source-ordered forest.
    let mut agents = Vec::with_capacity(keep.len());
    for s in &sessions {
        if !keep.contains(&s.id) {
            continue;
        }
        let parent_id = kept_parent.get(&s.id).cloned().flatten();
        let depth = depth_of(&s.id, &kept_parent);
        let status = if terminated_now.contains(&s.id) {
            AgentStatus::Terminated
        } else {
            agent_status(store, s, &claude_status, &mtime_secs_ago, now)
        };
        let mut agent = build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            status,
        );
        agent.label = display_label(
            depth,
            agent.cwd.as_deref(),
            subagent_ordinal.get(&s.id).copied(),
            root_dup_ordinal.get(&s.id).copied(),
            &agent.label,
        );
        agents.push(agent);
    }

    RadarState {
        generated_at: now.to_rfc3339(),
        agents,
    }
}

fn is_subagent_transcript_path(path: &Path) -> bool {
    crate::ingest::claude_code::is_subagent_session_path(path)
}

/// Walk a session's parent-chain to its root and report whether that root is
/// directly open. A session is a member of the live forest iff its root agent is
/// open (a subagent rides on its open root; an orphan under a closed root is
/// excluded). Bounded to avoid looping on a malformed cycle; a chain whose parent
/// id is absent from the store is treated as ending at the current node (root).
fn root_is_open(
    id: &str,
    parent_of: &HashMap<String, Option<String>>,
    by_id: &HashMap<&str, &Session>,
    directly_open: &dyn Fn(&Session) -> bool,
) -> bool {
    let mut cur = id.to_string();
    for _ in 0..64 {
        match parent_of.get(&cur).and_then(|p| p.clone()) {
            // A parent that is not itself a stored session can't anchor a tree —
            // stop here and judge the current node as the effective root.
            Some(p) if by_id.contains_key(p.as_str()) => cur = p,
            _ => break,
        }
    }
    by_id
        .get(cur.as_str())
        .is_some_and(|root| directly_open(root))
}

/// Depth = number of ancestors via the persisted parent links (root = 0). Bounded
/// to avoid looping on a malformed cycle.
fn depth_of(id: &str, parent_of: &HashMap<String, Option<String>>) -> u32 {
    let mut depth = 0;
    let mut cur = id.to_string();
    for _ in 0..64 {
        match parent_of.get(&cur).and_then(|p| p.clone()) {
            Some(p) => {
                depth += 1;
                cur = p;
            }
            None => break,
        }
    }
    depth
}

#[cfg(test)]
fn relink_store_subagents(store: &Store) {
    let _ = crate::ingest::codex::link_codex_subagents_in_store(store);
    let _ = crate::ingest::claude_code::link_claude_subagents_in_store(store);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::radar::agent::recent_activity;
    use crate::radar::composition;
    use crate::radar::context::est_cost_usd;
    use crate::radar::live::{recompute_radar_state, refresh_live_context};
    use chrono::Utc;
    use std::path::PathBuf;

    /// Seed a session with the given id/external/harness and an optional last
    /// `TokenUsage` event (so size/composition populate).
    fn seed(
        store: &Store,
        id: &str,
        external: &str,
        harness: Harness,
        cwd: Option<&str>,
        usage: Option<(u32, u32, u32, u32, &str)>,
    ) {
        let now = Utc::now();
        let session = Session {
            id: id.into(),
            harness,
            external_id: external.into(),
            project: cwd.map(|c| ProjectRef {
                cwd: PathBuf::from(c),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let tid = format!("{id}-t0");
        let mut events = vec![EventRecord {
            id: format!("{id}-p"),
            turn_id: tid.clone(),
            session_id: id.into(),
            ts: now,
            event: Event::UserPrompt {
                text: "do the thing".into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        }];
        if let Some((input, cc, cr, output, model)) = usage {
            // A turn that produced usage also produced its assistant response: end on a
            // completed `AssistantText` (a text-only `end_turn` turn) so the semantic
            // liveness rule reads this seeded session as SETTLED/idle — the realistic
            // shape of a finished turn. (A still-working turn is `seed(.., None)`, which
            // leaves the trailing UserPrompt → working.) Distinct increasing timestamps
            // keep the stored order UserPrompt → AssistantText → TokenUsage.
            events.push(EventRecord {
                id: format!("{id}-a"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now + chrono::Duration::milliseconds(10),
                event: Event::AssistantText {
                    text: "here you go".into(),
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: 1,
                    line: 2,
                },
            });
            events.push(EventRecord {
                id: format!("{id}-u"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now + chrono::Duration::milliseconds(20),
                event: Event::TokenUsage {
                    input,
                    output,
                    cache_creation: cc,
                    cache_read: cr,
                    model: model.into(),
                    orchestration: None,
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: 2,
                    line: 3,
                },
            });
        }
        let turn = Turn {
            id: tid,
            session_id: id.into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        store
            .upsert_session_batch(&session, &[turn], &events, 0)
            .unwrap();
    }

    /// Build a temp Claude liveness registry dir holding a `<pid>.json` per
    /// `(pid, external_id)`, so the named root sessions count as currently OPEN under
    /// the membership filter. Returns the tempdir guard (drop = cleanup) — keep it
    /// alive for the duration of the test.
    fn claude_registry(entries: &[(u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (pid, sid) in entries {
            std::fs::write(
                dir.path().join(format!("{pid}.json")),
                serde_json::json!({ "pid": pid, "sessionId": sid, "cwd": "/work" }).to_string(),
            )
            .unwrap();
        }
        dir
    }

    /// A Codex-open predicate that treats every Codex session as open — used by
    /// tests whose subject is composition/labels/links, not membership.
    fn codex_all_open(_: &Session) -> bool {
        true
    }

    /// One live Claude session re-ingested as SEVERAL store rows (same external id,
    /// distinct store ids — the real shape of a long session crossing compaction
    /// segments) must collapse to a SINGLE globe, not one per row. Regression for the
    /// "14 globes for 6 live sessions" duplication.
    #[test]
    fn assemble_collapses_duplicate_external_id_rows_to_one_agent() {
        let store = Store::memory().unwrap();
        for i in 0..5 {
            seed(
                &store,
                &format!("dup-row-{i}"),
                "live-sid", // shared external id across all five rows
                Harness::ClaudeCode,
                Some("/Users/k/Developer/MyRepo"),
                Some((2, 100, 1000, 50, "claude-opus-4-8")),
            );
        }
        let reg = claude_registry(&[(4242, "live-sid")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        assert_eq!(
            state.agents.len(),
            1,
            "five store rows of one live session must render as ONE globe"
        );
        assert_eq!(state.agents[0].depth, 0);
        assert_eq!(state.agents[0].child_count, 0);
    }

    #[test]
    fn assemble_prefers_root_transcript_over_subagent_duplicate_external_id() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = |id: &str, external: &str, source: &str, started_at: DateTime<Utc>| Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: external.into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at,
            ended_at: None,
            source_path: PathBuf::from(source),
            raw_hash: 0,
            ingested_at: started_at,
            meta: serde_json::json!({}),
        };
        let root = session("root", "root-ext", "/tmp/proj/root-ext.jsonl", now);
        let duplicate = session(
            "duplicate-subagent-row",
            "root-ext",
            "/tmp/proj/root-ext/subagents/agent-child.jsonl",
            now + chrono::Duration::seconds(10),
        );
        let child = session(
            "child",
            "agent-child",
            "/tmp/proj/root-ext/subagents/agent-child.jsonl",
            now + chrono::Duration::seconds(20),
        );
        store.upsert_session_batch(&root, &[], &[], 0).unwrap();
        store.upsert_session_batch(&duplicate, &[], &[], 0).unwrap();
        store.upsert_session_batch(&child, &[], &[], 0).unwrap();
        store.link_child_session("child", "root").unwrap();

        let reg = claude_registry(&[(100, "root-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);

        assert!(
            state.agents.iter().any(|a| a.id == "root"),
            "the real root transcript must remain the canonical root"
        );
        assert!(
            state
                .agents
                .iter()
                .all(|a| a.id != "duplicate-subagent-row"),
            "a stale subagent-path duplicate must not replace the root"
        );
        let child = state.agents.iter().find(|a| a.id == "child").unwrap();
        assert_eq!(child.parent_id.as_deref(), Some("root"));
    }

    #[test]
    fn codex_tail_usage_with_empty_model_uses_session_window_metadata() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = Session {
            id: "codex-tail".into(),
            harness: Harness::Codex,
            external_id: "codex-tail-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/codex-tail.jsonl"),
            raw_hash: 1,
            ingested_at: now,
            meta: serde_json::json!({ "model_context_window": 258400 }),
        };
        let turn = Turn {
            id: "codex-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let usage = EventRecord {
            id: "codex-tail-usage".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: now,
            event: Event::TokenUsage {
                input: 94_263,
                output: 153,
                cache_creation: 0,
                cache_read: 93_056,
                model: "".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[usage], 100)
            .unwrap();

        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            now,
        );
        let agent = state.agents.iter().find(|a| a.id == "codex-tail").unwrap();
        assert_eq!(agent.context_tokens, 94_263);
        assert_eq!(
            agent.max_tokens, 258_400,
            "an incremental Codex token_count has no session_meta in the tail, so an empty event model must fall back to session/window metadata"
        );
        assert!(agent.fill_pct > 0.36 && agent.fill_pct < 0.37);
    }

    #[test]
    fn context_tokens_include_estimated_tail_after_latest_usage() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "tail-growth",
            "tail-growth-ext",
            Harness::ClaudeCode,
            Some("/tmp/WARDEN"),
            Some((100, 50, 1_000, 25, "claude-sonnet-4-5")),
        );
        let mut session = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == "tail-growth")
            .unwrap();
        session.raw_hash = 2;
        let now = Utc::now();
        let turn = Turn {
            id: "tail-growth-t1".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Tool,
            index: 2,
            started_at: now + chrono::Duration::milliseconds(30),
            duration_ms: None,
            is_sidechain: false,
        };
        let tail = EventRecord {
            id: "tail-growth-tool-result".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: now + chrono::Duration::milliseconds(30),
            event: Event::ToolResult {
                call_id: "c1".into(),
                status: ToolStatus::Ok,
                bytes: 400,
                summary: Some("fresh tool output".into()),
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 3,
                line: 4,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[tail], 400)
            .unwrap();

        let reg = claude_registry(&[(4242, "tail-growth-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        let agent = state.agents.iter().find(|a| a.id == "tail-growth").unwrap();
        assert_eq!(
            agent.context_tokens, 1_250,
            "last API usage is 1150 resident tokens; the 400-byte tail should add an estimated 100 tokens"
        );
        assert!(
            agent
                .context_breakdown
                .rows
                .iter()
                .any(|row| row.key == "pending_tail" && row.tokens == 100),
            "the context window must disclose post-usage tail bytes as an estimated row"
        );
    }

    #[test]
    fn assemble_chooses_newer_tail_usage_even_when_tail_turn_index_restarts() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let session = Session {
            id: "tail-order".into(),
            harness: Harness::Codex,
            external_id: "tail-order-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WARDEN"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/tail-order.jsonl"),
            raw_hash: 1,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let old_turn = Turn {
            id: "tail-order-old-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 2,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let old_usage = EventRecord {
            id: "tail-order-old-usage".into(),
            turn_id: old_turn.id.clone(),
            session_id: session.id.clone(),
            ts: now,
            event: Event::TokenUsage {
                input: 10_000,
                output: 10,
                cache_creation: 0,
                cache_read: 0,
                model: "openai".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 100,
                line: 10,
            },
        };
        store
            .upsert_session_batch(&session, &[old_turn], &[old_usage], 100)
            .unwrap();

        let mut tail_session = session.clone();
        tail_session.raw_hash = 2;
        let tail_turn = Turn {
            id: "tail-order-new-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now + chrono::Duration::seconds(1),
            duration_ms: None,
            is_sidechain: false,
        };
        let new_usage = EventRecord {
            id: "tail-order-new-usage".into(),
            turn_id: tail_turn.id.clone(),
            session_id: session.id.clone(),
            ts: now + chrono::Duration::seconds(1),
            event: Event::TokenUsage {
                input: 42_000,
                output: 10,
                cache_creation: 0,
                cache_read: 0,
                model: "".into(),
                orchestration: None,
            },
            raw_ref: RawRef {
                source_path: session.source_path.clone(),
                offset: 200,
                line: 20,
            },
        };
        store
            .upsert_session_batch(&tail_session, &[tail_turn], &[new_usage], 200)
            .unwrap();

        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            now + chrono::Duration::seconds(1),
        );
        let agent = state.agents.iter().find(|a| a.id == "tail-order").unwrap();
        assert_eq!(
            agent.context_tokens, 42_000,
            "newer live-tail usage must win even when the tail parser restarts local turn indexes"
        );
    }

    /// Fix #3 — INCREMENTAL token cache: re-assembling an UNCHANGED store must NOT
    /// re-tokenize. The first assemble tokenizes (cache miss) and persists the raw
    /// sums keyed by the session's content hash; the second assemble hits the cache
    /// and performs ZERO additional `tokenize_len` calls, while producing a
    /// byte-identical estimated composition. This is what drops a steady-state
    /// recompute (only the one written session changes) to ~ms.
    #[test]
    fn assemble_uses_token_cache_on_unchanged_session() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "live-sid",
            "live-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            Some((2, 13761, 331244, 2620, "claude-opus-4-8")),
        );
        let reg = claude_registry(&[(100, "live-ext")]);

        // First assemble: a cache miss → it tokenizes the transcript.
        let before1 = composition::tokenize_call_count();
        let state1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let tokenized_run1 = composition::tokenize_call_count() - before1;
        assert!(
            tokenized_run1 > 0,
            "the first assemble must tokenize (cache miss), did {tokenized_run1} calls"
        );
        let est1 = state1
            .agents
            .iter()
            .find(|a| a.id == "live-sid")
            .and_then(|a| a.composition.estimated.clone())
            .expect("estimated composition present");

        // Second assemble: the store is unchanged → cache hit → ZERO tokenization.
        let before2 = composition::tokenize_call_count();
        let state2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let tokenized_run2 = composition::tokenize_call_count() - before2;
        assert_eq!(
            tokenized_run2, 0,
            "an unchanged session must NOT re-tokenize on the second assemble, did {tokenized_run2}"
        );
        let est2 = state2
            .agents
            .iter()
            .find(|a| a.id == "live-sid")
            .and_then(|a| a.composition.estimated.clone())
            .expect("estimated composition present");

        assert_eq!(
            est1, est2,
            "the cached estimated composition must be byte-identical to the freshly-tokenized one"
        );
    }

    /// `assemble` builds a forest: the root (depth 0, parentId null, childCount 1,
    /// populated occupancy + exact composition) and a linked child (depth 1,
    /// parentId == root). JSON serializes with camelCase keys.
    #[test]
    fn assemble_builds_root_and_child_with_size_and_links() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "root-sid",
            "root-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            Some((2, 13761, 331244, 2620, "claude-opus-4-8")),
        );
        seed(
            &store,
            "child-sid",
            "child-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("child-sid", "root-sid").unwrap();

        // The root is registered as open (its external id is in the live registry);
        // the child rides on its open root. is_alive=true; codex predicate unused.
        let reg = claude_registry(&[(100, "root-ext")]);
        let now = Utc::now();
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);

        assert_eq!(state.agents.len(), 2);
        let root = state
            .agents
            .iter()
            .find(|a| a.id == "root-sid")
            .expect("root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(root.child_count, 1, "root has one linked child");
        assert_eq!(
            root.label, "MyRepo",
            "a Claude root is labeled by its project folder (B1)"
        );
        assert_eq!(
            root.cwd.as_deref(),
            Some("MyRepo"),
            "the cwd basename is still exposed for the folder subtitle"
        );
        assert_eq!(root.context_tokens, 345_007, "2+13761+331244");
        assert!(
            (root.fill_pct - 0.345_007).abs() < 1e-6,
            "345007 / 1M Opus window ≈ 0.345 (not clamped against the old 200k)"
        );
        assert_eq!(root.composition.exact.cache_read, 331_244);
        assert_eq!(root.composition.exact.fresh, 2 + 13_761);
        assert_eq!(root.composition.exact.output, 2_620);
        assert!(
            root.composition.estimated.is_some(),
            "a turn-1 baseline yields an estimated composition"
        );
        assert_eq!(root.context_breakdown.used_tokens, 345_007);
        assert_eq!(root.context_breakdown.max_tokens, 1_000_000);
        assert!(
            root.context_breakdown
                .rows
                .iter()
                .any(|r| r.key == "messages" && r.tokens > 0),
            "context window rows must include live message occupancy"
        );
        assert!(
            root.context_breakdown
                .rows
                .iter()
                .any(|r| r.key == "free_space" && r.tokens == 1_000_000 - 345_007),
            "context window rows must include free space against the real max window"
        );
        assert!(root.est_cost_usd.is_some(), "opus model → a cost estimate");

        let child = state
            .agents
            .iter()
            .find(|a| a.id == "child-sid")
            .expect("child present");
        assert_eq!(child.depth, 1);
        assert_eq!(child.parent_id.as_deref(), Some("root-sid"));
        assert_eq!(child.child_count, 0);

        // Contract: camelCase keys present in the serialized payload.
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"fillPct\""), "camelCase fillPct");
        assert!(
            json.contains("\"contextTokens\""),
            "camelCase contextTokens"
        );
        assert!(
            json.contains("\"contextBreakdown\""),
            "camelCase contextBreakdown"
        );
        assert!(json.contains("\"parentId\""), "camelCase parentId");
        assert!(json.contains("\"childCount\""), "camelCase childCount");
        assert!(json.contains("\"cacheRead\""), "camelCase nested cacheRead");
        assert!(json.contains("\"generatedAt\""), "camelCase generatedAt");
    }

    /// A Codex Desktop subagent inserted into the store WITHOUT any pre-run linkage
    /// pass is linked by the explicit relink boundary (startup/live ingest), then
    /// appears as a child in the forest. Steady recomputes are read-only.
    #[test]
    fn explicit_relink_links_codex_subagent_without_pre_pass() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let mk = |id: &str, ext: &str, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::Codex,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        // Parent Codex Desktop session.
        store
            .upsert_session_batch(
                &mk(
                    "cx-parent",
                    "thread-parent",
                    serde_json::json!({ "originator": "Codex Desktop" }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();
        // Subagent: thread_source=subagent + parent_thread_id pointing at the parent.
        store
            .upsert_session_batch(
                &mk(
                    "cx-child",
                    "thread-child",
                    serde_json::json!({
                        "thread_source": "subagent",
                        "parent_thread_id": "thread-parent",
                        "agent_role": "explorer",
                        "agent_nickname": "Hilbert",
                        "originator": "Codex Desktop",
                    }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();

        // No pre-run linkage pass: parent is NULL right now.
        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            None,
            "precondition: child is unlinked before recompute"
        );

        // Re-derive linkage as `recompute_radar_state` does, then assemble with the
        // two Codex sessions injected as open (membership decided by the closure, so
        // the test does not depend on real ~/.codex rollouts on disk).
        relink_store_subagents(&store);
        let open_ids = ["thread-parent", "thread-child"];
        let is_codex_open = |s: &Session| open_ids.contains(&s.external_id.as_str());
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            Utc::now(),
        );

        // Recompute re-derived the link and persisted it.
        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            Some("cx-parent".to_string()),
            "relink must persist the newly-resolvable parent"
        );
        let child = state
            .agents
            .iter()
            .find(|a| a.id == "cx-child")
            .expect("child present");
        assert_eq!(child.parent_id.as_deref(), Some("cx-parent"));
        assert_eq!(child.depth, 1, "child renders nested, not flat");
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 1, "parent shows one child");
    }

    #[test]
    fn steady_recompute_does_not_relink_without_new_ingest() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let mk = |id: &str, ext: &str, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::Codex,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        store
            .upsert_session_batch(
                &mk(
                    "cx-parent",
                    "thread-parent",
                    serde_json::json!({ "originator": "Codex Desktop" }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();
        store
            .upsert_session_batch(
                &mk(
                    "cx-child",
                    "thread-child",
                    serde_json::json!({
                        "thread_source": "subagent",
                        "parent_thread_id": "thread-parent",
                        "originator": "Codex Desktop",
                    }),
                ),
                &[],
                &[],
                0,
            )
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let _ = recompute_radar_state(&store, registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert_eq!(
            store.parent_of("cx-child").unwrap(),
            None,
            "heartbeat/read recomputes must not run the whole-store relinker when no new bytes were ingested"
        );
    }

    #[test]
    fn steady_recompute_does_not_ingest_live_codex_rollout_without_explicit_refresh() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

        let live_dir = sessions_root.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let path =
            live_dir.join("rollout-2026-06-25T00-00-00-019efd6c-8f60-7f42-8da1-3977122aa6be.jsonl");
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        let t1 = (now + chrono::Duration::milliseconds(100)).to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{t0}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"019efd6c-8f60-7f42-8da1-3977122aa6be\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\"}}}}\n\
                 {{\"timestamp\":\"{t1}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"do not ingest me on heartbeat\"}}}}\n",
            ),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let claude_registry = tempfile::tempdir().unwrap();
        let state = recompute_radar_state(&store, claude_registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            store.sessions().unwrap().is_empty(),
            "steady heartbeat/read recompute must not ingest transcript bytes"
        );
        assert!(
            state.agents.is_empty(),
            "without an explicit live refresh or backfill, recompute should only assemble the persisted store"
        );
    }

    /// Regression: a Codex rollout that was already open before WARDEN started must
    /// appear after the explicit startup/cold-read refresh even when the store is
    /// empty/stale. The refresh path pulls live Codex tails before assembly; ordinary
    /// heartbeat recompute remains read-only.
    #[test]
    fn explicit_refresh_ingests_live_codex_rollout_before_assembling() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let sessions_root = tempfile::tempdir().unwrap();
        let archived_root = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", sessions_root.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", archived_root.path());

        let live_dir = sessions_root.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let path =
            live_dir.join("rollout-2026-06-25T00-00-00-019efd6c-8f60-7f42-8da1-3977122aa6be.jsonl");
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        let t1 = (now + chrono::Duration::milliseconds(100)).to_rfc3339();
        let t2 = (now + chrono::Duration::milliseconds(200)).to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{t0}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"019efd6c-8f60-7f42-8da1-3977122aa6be\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\"}}}}\n\
                 {{\"timestamp\":\"{t1}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n\
                 {{\"timestamp\":\"{t2}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"keep tracking this live codex context\"}}}}\n",
            ),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let claude_registry = tempfile::tempdir().unwrap();
        let refreshed = refresh_live_context(&store, claude_registry.path());
        let state = recompute_radar_state(&store, claude_registry.path());

        match old_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            refreshed > 0,
            "explicit live refresh should ingest the live Codex rollout before assembly"
        );
        let codex = state
            .agents
            .iter()
            .find(|a| a.harness == "codex")
            .expect("live Codex rollout should render on first recompute");
        assert_eq!(codex.harness, "codex");
        assert_eq!(codex.status, "working");
        assert!(
            codex
                .recent_activity
                .iter()
                .any(|a| a.label.contains("keep tracking this live codex context")),
            "freshly ingested Codex activity should drive the live log: {:?}",
            codex.recent_activity
        );
    }

    /// Regression: a Claude Code session that was already running before WARDEN
    /// started must have its transcript tail pulled by the explicit startup/cold-read
    /// refresh before the live forest is assembled. The liveness registry alone can
    /// say the PID/session is open, but without a fresh store row RADAR has no
    /// context/logs to render and the globe is absent or stale until a later
    /// watcher/backfill catches up.
    #[test]
    fn explicit_refresh_ingests_live_claude_transcript_before_assembling() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_claude_projects = std::env::var_os("WARDEN_CLAUDE_PROJECTS");
        let old_codex_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_codex_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");

        let claude_projects = tempfile::tempdir().unwrap();
        let codex_sessions = tempfile::tempdir().unwrap();
        let codex_archived = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CLAUDE_PROJECTS", claude_projects.path());
        std::env::set_var("WARDEN_CODEX_SESSIONS", codex_sessions.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", codex_archived.path());

        let session_id = "live-claude-session";
        let project_dir = claude_projects.path().join("-tmp-LiveClaude");
        std::fs::create_dir_all(&project_dir).unwrap();
        let transcript = project_dir.join(format!("{session_id}.jsonl"));
        let now = Utc::now();
        let t0 = now.to_rfc3339();
        std::fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{session_id}\",\"timestamp\":\"{t0}\",\"cwd\":\"/tmp/LiveClaude\",\"message\":{{\"role\":\"user\",\"content\":\"track this live claude context before startup backfill\"}}}}\n",
            ),
        )
        .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        std::fs::write(
            registry.path().join(format!("{pid}.json")),
            serde_json::json!({
                "pid": pid,
                "sessionId": session_id,
                "cwd": "/tmp/LiveClaude",
                "entrypoint": "claude-desktop"
            })
            .to_string(),
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let refreshed = refresh_live_context(&store, registry.path());
        let state = recompute_radar_state(&store, registry.path());

        match old_claude_projects {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_PROJECTS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_PROJECTS"),
        }
        match old_codex_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_codex_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }

        assert!(
            refreshed > 0,
            "explicit live refresh should ingest the live Claude transcript before assembly"
        );
        let claude = state
            .agents
            .iter()
            .find(|a| a.harness == "claude_code")
            .expect("live Claude transcript should render on first recompute");
        assert_eq!(claude.status, "working");
        assert_eq!(claude.cwd.as_deref(), Some("LiveClaude"));
        assert!(
            claude
                .recent_activity
                .iter()
                .any(|a| a.label.contains("track this live claude context")),
            "freshly ingested Claude activity should drive the live log: {:?}",
            claude.recent_activity
        );
    }

    /// The "what is it doing" signal: a tool call's recent-activity label names its
    /// TARGET (file path basename / command), not just the bare tool name, and the
    /// opaque `result <call_id>` rows are dropped (they were pure noise). Built from
    /// the real `Event::ToolCall.input` shapes for Claude (`file_path`/`command`) and
    /// Codex (`cmd`).
    #[test]
    fn recent_activity_names_tool_targets_and_drops_result_rows() {
        let now = Utc::now();
        let turn = Turn {
            id: "t".into(),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let mk = |i: u64, event: Event| {
            (
                turn.clone(),
                EventRecord {
                    id: format!("e{i}"),
                    turn_id: "t".into(),
                    session_id: "s".into(),
                    ts: now,
                    event,
                    raw_ref: RawRef {
                        source_path: PathBuf::from("/x.jsonl"),
                        offset: i,
                        line: i as u32,
                    },
                },
            )
        };
        let events = vec![
            mk(
                1,
                Event::ToolCall {
                    tool: "Read".into(),
                    input: serde_json::json!({"file_path":"/Users/k/WARDEN/src/viz/orbLayout.ts"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            mk(
                2,
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: ToolStatus::Ok,
                    bytes: 10,
                    summary: None,
                },
            ),
            mk(
                3,
                Event::ToolCall {
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"cargo test radar"}),
                    call_id: "c2".into(),
                    kind: ToolKind::Builtin,
                },
            ),
            mk(
                4,
                Event::ToolCall {
                    tool: "exec_command".into(),
                    input: serde_json::json!({"cmd":"cargo build","workdir":"/Users/k/WARDEN"}),
                    call_id: "c3".into(),
                    kind: ToolKind::Builtin,
                },
            ),
        ];
        let acts = recent_activity(&events);
        assert!(
            acts.iter().all(|a| !a.label.starts_with("result ")),
            "opaque `result <id>` rows must be dropped, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("orbLayout.ts")),
            "a Read must name the file it touches, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("cargo test radar")),
            "a Bash must name the command it runs, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "tool" && a.label.contains("cargo build")),
            "a Codex exec_command must name the command it runs, got {acts:?}"
        );
    }

    #[test]
    fn recent_activity_orders_by_timestamp_not_storage_order() {
        let now = Utc::now();
        let turn = Turn {
            id: "t".into(),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let mk = |id: &str, ts: DateTime<Utc>, event: Event| {
            (
                turn.clone(),
                EventRecord {
                    id: id.into(),
                    turn_id: "t".into(),
                    session_id: "s".into(),
                    ts,
                    event,
                    raw_ref: RawRef {
                        source_path: PathBuf::from("/x.jsonl"),
                        offset: 0,
                        line: 1,
                    },
                },
            )
        };
        let events = vec![
            mk(
                "new",
                now,
                Event::AssistantText {
                    text: "newest final answer".into(),
                },
            ),
            mk(
                "old",
                now - chrono::Duration::seconds(10),
                Event::ToolCall {
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"old command"}),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ),
        ];

        let acts = recent_activity(&events);
        assert_eq!(
            acts.first().map(|a| a.label.as_str()),
            Some("newest final answer")
        );
    }

    /// Naming: a Claude ROOT agent is named by its originating task (its first
    /// non-meta user prompt), not merely the cwd basename — so several live sessions
    /// in the same repo are differentiated by what each is doing. The folder basename
    /// is still exposed (as `cwd`) for the secondary "folder · model" subtitle.
    #[test]
    fn claude_root_label_is_its_folder_with_cwd_exposed() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "r",
            "r-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/WARDEN"),
            Some((2, 100, 1000, 50, "claude-opus-4-8")),
        );
        let reg = claude_registry(&[(100, "r-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let a = state
            .agents
            .iter()
            .find(|a| a.id == "r")
            .expect("root present");
        assert_eq!(
            a.label, "WARDEN",
            "a Claude root is named by its project folder (B1), not its originating task"
        );
        assert_eq!(
            a.cwd.as_deref(),
            Some("WARDEN"),
            "the folder basename is exposed as `cwd` for the subtitle"
        );
    }

    /// Finding 1: a linked Claude subagent surfaces its sidecar `description` as the
    /// `label` and its `agentType` as the `role` (the frozen `radar_state` contract),
    /// instead of falling back to the external id with a null role.
    #[test]
    fn claude_subagent_uses_description_and_agent_type() {
        let store = Store::memory().unwrap();
        // Parent root (Claude, has a cwd → label = basename).
        seed(
            &store,
            "p-sid",
            "p-ext",
            Harness::ClaudeCode,
            Some("/Users/k/Developer/MyRepo"),
            None,
        );
        // Child subagent: meta carries the description + agentType the ingest path
        // persists from the sidecar `agent-<id>.meta.json`.
        let now = Utc::now();
        let child = Session {
            id: "c-sid".into(),
            harness: Harness::ClaudeCode,
            external_id: "c-ext".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from("/tmp/c.jsonl"),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({
                "description": "hunt for dead code in the radar module",
                "agentType": "Explore",
            }),
        };
        store.upsert_session_batch(&child, &[], &[], 0).unwrap();
        store.link_child_session("c-sid", "p-sid").unwrap();

        let reg = claude_registry(&[(100, "p-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());
        let c = state
            .agents
            .iter()
            .find(|a| a.id == "c-sid")
            .expect("child present");
        assert_eq!(
            c.label, "subagent 1",
            "Claude subagent label is its per-parent ordinal (B1), not its description"
        );
        assert_eq!(
            c.role.as_deref(),
            Some("Explore"),
            "Claude subagent role is its agentType"
        );

        // Root is labeled by its project folder; its folder is still exposed as `cwd`.
        let p = state.agents.iter().find(|a| a.id == "p-sid").unwrap();
        assert_eq!(p.label, "MyRepo", "root label is its project folder (B1)");
        assert_eq!(
            p.cwd.as_deref(),
            Some("MyRepo"),
            "root still exposes its cwd"
        );
    }

    /// `est_cost_usd` bills cache reads ~10× cheaper than fresh input: for opus
    /// (input $15/1M, cache-read $1.50/1M), 1M cache-read tokens cost ≈ $1.50, NOT
    /// the $15.00 the old "full input rate on the whole sum" path produced. Fresh
    /// input still bills at the full input rate; output at the output rate.
    #[test]
    fn est_cost_bills_cache_read_cheaper_than_fresh() {
        let model = Some("claude-opus-4-8".to_string());

        // Pure cache-read: 1M tokens at the cache-read rate (~0.1× input).
        let cache_only = composition::ExactComposition {
            cache_read: 1_000_000,
            fresh: 0,
            output: 0,
        };
        let cost = est_cost_usd(&model, &cache_only).expect("opus → a cost");
        assert!(
            (cost - 1.50).abs() < 1e-6,
            "1M cache-read tokens bill at the cache-read rate (~$1.50), got {cost}"
        );

        // Pure fresh input: 1M tokens at the full input rate.
        let fresh_only = composition::ExactComposition {
            cache_read: 0,
            fresh: 1_000_000,
            output: 0,
        };
        let fresh_cost = est_cost_usd(&model, &fresh_only).expect("opus → a cost");
        assert!(
            (fresh_cost - 15.0).abs() < 1e-6,
            "1M fresh tokens bill at the input rate ($15.00), got {fresh_cost}"
        );

        // Cache reads are strictly cheaper than the same volume of fresh input.
        assert!(
            cost < fresh_cost,
            "cache reads must be cheaper than fresh input"
        );

        // Unknown model stays nullable.
        assert_eq!(est_cost_usd(&Some("mystery".into()), &cache_only), None);
    }

    /// A session with no `TokenUsage` reports zero occupancy and a `null` estimated
    /// composition (no turn-1 baseline) — honest, never fabricated.
    #[test]
    fn assemble_session_without_usage_is_zeroed_and_unestimated() {
        let store = Store::memory().unwrap();
        seed(&store, "s", "e", Harness::Codex, Some("/tmp/proj"), None);
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &codex_all_open,
            Utc::now(),
        );
        let a = &state.agents[0];
        assert_eq!(a.context_tokens, 0);
        assert_eq!(a.fill_pct, 0.0);
        assert!(
            a.composition.estimated.is_none(),
            "no baseline → null estimate"
        );
        assert_eq!(a.est_cost_usd, None, "no model → no cost");
    }

    /// THE FIX (spec §3/§5: the forest is the OPEN set). A backfilled Claude session
    /// whose external id is NOT in the live registry is EXCLUDED — the archive of
    /// every transcript ever ingested must not render. A Claude session that IS in the
    /// registry is included; its status now comes from CONVERSATION STATE (Fault B):
    /// `seed` leaves a fresh, unanswered `UserPrompt` as the last event, so the honest
    /// verdict is `working` (the operator just asked) — not the old mtime "idle".
    #[test]
    fn claude_forest_includes_only_registry_open_sessions() {
        let store = Store::memory().unwrap();
        // Historical/backfill session: ingested long ago, no live registry entry.
        seed(
            &store,
            "hist",
            "hist-ext",
            Harness::ClaudeCode,
            Some("/tmp/old"),
            None,
        );
        // Currently-open session: a live `<pid>.json` references its session id.
        seed(
            &store,
            "live",
            "live-ext",
            Harness::ClaudeCode,
            Some("/tmp/now"),
            None,
        );

        let reg = claude_registry(&[(100, "live-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        assert_eq!(
            state.agents.len(),
            1,
            "only the registry-open session is in the forest"
        );
        let a = &state.agents[0];
        assert_eq!(
            a.id, "live",
            "the open session is the live one, not the backfill"
        );
        assert_eq!(
            a.status, "working",
            "last event is a fresh unanswered UserPrompt → working (Fault B: conversation-state, not mtime)"
        );
        assert!(
            !state.agents.iter().any(|a| a.id == "hist"),
            "the historical/backfill session must be excluded"
        );
    }

    /// FAULT B end-to-end via `assemble`: a registry-open Claude session's working/idle
    /// verdict comes from its LAST ingested event, and is DETERMINISTIC across reads
    /// (the property that kills the flicker). A session whose last event is a completed
    /// `TokenUsage` turn is idle; a session whose last event is an unanswered
    /// `UserPrompt` is working; two assembles on the unchanged store at the same instant
    /// return byte-identical statuses. (The OLD mtime path, keyed on FSEvents-coalesced
    /// file writes, could flip these between reads — this test pins the fix.)
    #[test]
    fn assemble_status_from_conversation_state_is_deterministic() {
        let store = Store::memory().unwrap();
        // `seed` writes a UserPrompt then optional bookkeeping TokenUsage. Add a real
        // trailing AssistantText for idle-sess so the semantic tail is a completed turn.
        seed(
            &store,
            "idle-sess",
            "idle-ext",
            Harness::ClaudeCode,
            Some("/tmp/a"),
            Some((2, 100, 1000, 50, "claude-opus-4-8")),
        );
        let done_ts = Utc::now() + chrono::Duration::milliseconds(10);
        let done_session = Session {
            id: "idle-sess".into(),
            harness: Harness::ClaudeCode,
            external_id: "idle-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/a"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: done_ts,
            ended_at: None,
            source_path: PathBuf::from("/tmp/idle-sess.jsonl"),
            raw_hash: 1,
            ingested_at: done_ts,
            meta: serde_json::json!({}),
        };
        let done_turn = Turn {
            id: "idle-sess-done-turn".into(),
            session_id: "idle-sess".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 99,
            started_at: done_ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let done_event = EventRecord {
            id: "idle-sess-done-text".into(),
            turn_id: done_turn.id.clone(),
            session_id: "idle-sess".into(),
            ts: done_ts,
            event: Event::AssistantText {
                text: "done".into(),
            },
            raw_ref: RawRef {
                source_path: done_session.source_path.clone(),
                offset: 2,
                line: 3,
            },
        };
        store
            .upsert_session_batch(&done_session, &[done_turn], &[done_event], 0)
            .unwrap();
        // working-sess: last event is an unanswered UserPrompt (a strong working signal).
        seed(
            &store,
            "working-sess",
            "working-ext",
            Harness::ClaudeCode,
            Some("/tmp/b"),
            None,
        );

        // Both are registry-open WITHOUT an authoritative `status` field, so the
        // conversation-state fallback decides. Evaluate 60s in the FUTURE relative to the
        // seeded events: the completed AssistantText is idle while the unanswered
        // UserPrompt is still within the 180s stale backstop and remains working. A
        // FIXED clock makes the verdict exact and deterministic.
        let reg = claude_registry(&[(101, "idle-ext"), (102, "working-ext")]);
        let now = Utc::now() + chrono::Duration::seconds(60);

        let st = |state: &RadarState, id: &str| {
            state
                .agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.status.clone())
                .unwrap_or_default()
        };

        let s1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        assert_eq!(
            st(&s1, "idle-sess"),
            "idle",
            "last real action is a completed AssistantText turn → idle"
        );
        assert_eq!(
            st(&s1, "working-sess"),
            "working",
            "last event is an unanswered UserPrompt → working"
        );

        // Determinism: a second assemble on the UNCHANGED store at the SAME instant
        // yields identical statuses (no mtime, no flicker).
        let s2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, now);
        assert_eq!(
            st(&s2, "idle-sess"),
            st(&s1, "idle-sess"),
            "idle status stable across reads"
        );
        assert_eq!(
            st(&s2, "working-sess"),
            st(&s1, "working-sess"),
            "working status stable across reads"
        );
    }

    #[test]
    fn codex_stale_uningested_tail_does_not_stay_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f1111-1111-7111-8111-111111111111.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n\
             {\"timestamp\":\"2026-06-25T00:00:20Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\",\"phase\":\"final_answer\"}}\n",
        )
        .unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-stale-tail".into(),
            harness: Harness::Codex,
            external_id: "019f1111-1111-7111-8111-111111111111".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/StaleTail"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-stale-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-stale-tail-user".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::UserPrompt {
                text: "go".into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], 10)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(240),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-stale-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "idle",
            "a stale store row whose source file grew past its watermark must settle after the semantic backstop"
        );
    }

    #[test]
    fn codex_inflight_file_write_stays_working_with_uningested_tail() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f2222-2222-7222-8222-222222222222.jsonl");
        let complete_tool_call = "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"apply_patch src/app.ts\\\"}\",\"call_id\":\"call_write\"}}\n";
        let partial_tool_result =
            "{\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\"";
        std::fs::write(&path, format!("{complete_tool_call}{partial_tool_result}")).unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-write-tail".into(),
            harness: Harness::Codex,
            external_id: "019f2222-2222-7222-8222-222222222222".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/WritingFiles"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-write-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-write-tail-tool-call".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::ToolCall {
                tool: "exec_command".into(),
                input: serde_json::json!({ "cmd": "apply_patch src/app.ts" }),
                call_id: "call_write".into(),
                kind: ToolKind::Unknown,
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], complete_tool_call.len() as u64)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(60),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-write-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "an in-flight Codex file-write ToolCall must stay working while its result line is still incomplete"
        );
    }

    #[test]
    fn codex_incomplete_patch_tail_after_assistant_stays_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f3333-3333-7333-8333-333333333333.jsonl");
        let assistant_line = "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"I will update the files now.\"}}\n";
        let partial_patch =
            "{\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\"";
        std::fs::write(&path, format!("{assistant_line}{partial_patch}")).unwrap();

        let base = Utc::now();
        let session = Session {
            id: "codex-patch-tail".into(),
            harness: Harness::Codex,
            external_id: "019f3333-3333-7333-8333-333333333333".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/PatchTail"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-patch-tail-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let event = EventRecord {
            id: "codex-patch-tail-assistant".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::AssistantText {
                text: "I will update the files now.".into(),
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&session, &[turn], &[event], assistant_line.len() as u64)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            base + chrono::Duration::seconds(60),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-patch-tail")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "a partial Codex patch record means file writing is in progress even when the last complete event was assistant text"
        );
    }

    #[test]
    fn codex_patch_snapshot_after_assistant_stays_working() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019f4444-4444-7444-8444-444444444444.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"I will update the files now.\"}}\n\
             {\"timestamp\":\"2026-06-25T00:00:40Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"changes\":{\"/tmp/PatchDone/src/app.ts\":{\"type\":\"update\"}}}}\n",
        )
        .unwrap();

        let base = Utc::now();
        let patch_ts = base + chrono::Duration::seconds(40);
        let session = Session {
            id: "codex-patch-done".into(),
            harness: Harness::Codex,
            external_id: "019f4444-4444-7444-8444-444444444444".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/tmp/PatchDone"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec!["openai".into()],
            started_at: base,
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 1,
            ingested_at: base,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let turn = Turn {
            id: "codex-patch-done-turn".into(),
            session_id: session.id.clone(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: base,
            duration_ms: None,
            is_sidechain: false,
        };
        let assistant = EventRecord {
            id: "codex-patch-done-assistant".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: base,
            event: Event::AssistantText {
                text: "I will update the files now.".into(),
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 0,
                line: 1,
            },
        };
        let files = EventRecord {
            id: "codex-patch-done-files".into(),
            turn_id: turn.id.clone(),
            session_id: session.id.clone(),
            ts: patch_ts,
            event: Event::FileSnapshot {
                files: vec![FileEdit {
                    path: "/tmp/PatchDone/src/app.ts".into(),
                    ..Default::default()
                }],
            },
            raw_ref: RawRef {
                source_path: path.clone(),
                offset: 140,
                line: 2,
            },
        };
        let watermark = std::fs::metadata(&path).unwrap().len();
        store
            .upsert_session_batch(&session, &[turn], &[assistant, files], watermark)
            .unwrap();

        let registry = tempfile::tempdir().unwrap();
        let state = assemble(
            &store,
            registry.path(),
            &|_| true,
            &codex_all_open,
            patch_ts + chrono::Duration::seconds(20),
        );

        let codex = state
            .agents
            .iter()
            .find(|a| a.id == "codex-patch-done")
            .expect("open Codex session is rendered");
        assert_eq!(
            codex.status, "working",
            "a fresh Codex FileSnapshot is a real file-write action, not idle bookkeeping"
        );
    }

    /// THE FIX (spec §4.3: the archive move is the Codex 'done' signal). A Codex
    /// session whose rollout is archived (closed) is EXCLUDED; a non-archived rollout
    /// is included. Membership rides on the injected `is_codex_open` closure — the
    /// real collector resolves it from the on-disk location, never the stale
    /// `source_path`.
    #[test]
    fn codex_forest_excludes_archived_sessions() {
        let store = Store::memory().unwrap();
        seed(
            &store,
            "open-cx",
            "open-uuid",
            Harness::Codex,
            Some("/tmp/p1"),
            None,
        );
        seed(
            &store,
            "done-cx",
            "done-uuid",
            Harness::Codex,
            Some("/tmp/p2"),
            None,
        );

        // Only `open-uuid` currently lives under sessions/ (done-uuid was archived).
        let is_codex_open = |s: &Session| s.external_id == "open-uuid";
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            Utc::now(),
        );

        assert_eq!(
            state.agents.len(),
            1,
            "only the non-archived Codex session is open"
        );
        assert_eq!(state.agents[0].id, "open-cx");
        assert!(
            !state.agents.iter().any(|a| a.id == "done-cx"),
            "an archived (closed) Codex session must be excluded"
        );
    }

    /// THE FIX (subagent rule): an OPEN root with an open subagent still links
    /// (depth/childCount intact). A root EXCLUDED for being closed takes its
    /// now-orphaned subagent out too — assert the subagent is gone AND no surviving
    /// agent dangles a `parentId` pointing at a non-present parent.
    #[test]
    fn closed_root_drops_orphaned_subagents_no_dangling_parent() {
        let store = Store::memory().unwrap();
        // Open tree: root `op-root` (in registry) + Claude subagent `op-sub`.
        seed(
            &store,
            "op-root",
            "op-root-ext",
            Harness::ClaudeCode,
            Some("/tmp/a"),
            None,
        );
        seed(
            &store,
            "op-sub",
            "op-sub-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("op-sub", "op-root").unwrap();
        // Closed tree: root `cl-root` (NOT in registry) + subagent `cl-sub`.
        seed(
            &store,
            "cl-root",
            "cl-root-ext",
            Harness::ClaudeCode,
            Some("/tmp/b"),
            None,
        );
        seed(
            &store,
            "cl-sub",
            "cl-sub-ext",
            Harness::ClaudeCode,
            None,
            None,
        );
        store.link_child_session("cl-sub", "cl-root").unwrap();

        // Only the open root is registered alive.
        let reg = claude_registry(&[(100, "op-root-ext")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, Utc::now());

        // The open tree survives, nested and counted.
        let root = state
            .agents
            .iter()
            .find(|a| a.id == "op-root")
            .expect("open root present");
        assert_eq!(root.depth, 0);
        assert_eq!(root.parent_id, None);
        assert_eq!(
            root.child_count, 1,
            "open root counts its one open subagent"
        );
        let sub = state
            .agents
            .iter()
            .find(|a| a.id == "op-sub")
            .expect("open subagent present");
        assert_eq!(sub.depth, 1, "subagent rides on its open root, nested");
        assert_eq!(sub.parent_id.as_deref(), Some("op-root"));

        // The closed tree is gone entirely (root AND its orphaned subagent).
        assert!(
            !state.agents.iter().any(|a| a.id == "cl-root"),
            "closed root excluded"
        );
        assert!(
            !state.agents.iter().any(|a| a.id == "cl-sub"),
            "a subagent under a closed root is excluded, not orphaned"
        );

        // No surviving agent points at a parent that isn't itself in the forest.
        let present: std::collections::HashSet<&str> =
            state.agents.iter().map(|a| a.id.as_str()).collect();
        for a in &state.agents {
            if let Some(p) = &a.parent_id {
                assert!(
                    present.contains(p.as_str()),
                    "agent {} dangles parentId {} not in the forest",
                    a.id,
                    p
                );
            }
        }
    }

    /// Build a `(Turn, EventRecord)` carrying a single `ToolResult` for `call_id` at
    /// timestamp `ts` — the parent-side termination fact `subagent_terminated_at` reads.
    fn mk_tool_result_event(call_id: &str, ts: DateTime<Utc>) -> (Turn, EventRecord) {
        mk_tool_result_event_with_summary(call_id, ts, None)
    }

    fn mk_tool_result_event_with_summary(
        call_id: &str,
        ts: DateTime<Utc>,
        summary: Option<&str>,
    ) -> (Turn, EventRecord) {
        let turn = Turn {
            id: "p-t".into(),
            session_id: "parent".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: format!("res-{call_id}"),
            turn_id: "p-t".into(),
            session_id: "parent".into(),
            ts,
            event: Event::ToolResult {
                call_id: call_id.into(),
                status: ToolStatus::Ok,
                bytes: 0,
                summary: summary.map(str::to_string),
            },
            raw_ref: RawRef {
                source_path: PathBuf::from("/tmp/parent.jsonl"),
                offset: 0,
                line: 1,
            },
        };
        (turn, rec)
    }

    fn mk_user_prompt_event(text: &str, ts: DateTime<Utc>) -> (Turn, EventRecord) {
        let turn = Turn {
            id: "p-u".into(),
            session_id: "parent".into(),
            parent_id: None,
            role: Role::User,
            index: 2,
            started_at: ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: "prompt".into(),
            turn_id: "p-u".into(),
            session_id: "parent".into(),
            ts,
            event: Event::UserPrompt {
                text: text.into(),
                attachments: vec![],
                is_meta: false,
            },
            raw_ref: RawRef {
                source_path: PathBuf::from("/tmp/parent.jsonl"),
                offset: 0,
                line: 1,
            },
        };
        (turn, rec)
    }

    // ── B4: pure termination decision ────────────────────────────────────────────
    #[test]
    fn subagent_terminated_at_uses_result_then_timeout() {
        let now = Utc::now();
        let result_ts = now - chrono::Duration::seconds(2);
        let parent_events = vec![mk_tool_result_event("toolu_5", result_ts)];

        // Primary: a matching tool-result → terminated at the result's ts.
        assert_eq!(
            subagent_terminated_at(Some("toolu_5"), &parent_events, Some(now), now, 90_000),
            Some(result_ts)
        );
        // No result, recently active → still live.
        assert_eq!(
            subagent_terminated_at(
                Some("toolu_x"),
                &[],
                Some(now - chrono::Duration::seconds(3)),
                now,
                90_000
            ),
            None
        );
        // No result, silent past the backstop → terminated at (last + timeout).
        let last = now - chrono::Duration::seconds(200);
        assert_eq!(
            subagent_terminated_at(Some("toolu_x"), &[], Some(last), now, 90_000),
            Some(last + chrono::Duration::milliseconds(90_000))
        );
        // No tool_use_id and no last activity → never terminated.
        assert_eq!(subagent_terminated_at(None, &[], None, now, 90_000), None);
    }

    #[test]
    fn subagent_terminated_at_ignores_async_launch_ack() {
        let now = Utc::now();
        let parent_events = vec![mk_tool_result_event_with_summary(
            "toolu_async",
            now - chrono::Duration::seconds(1),
            Some("Async agent launched successfully.\nThe agent is working in the background."),
        )];

        assert_eq!(
            subagent_terminated_at(Some("toolu_async"), &parent_events, Some(now), now, 90_000),
            None,
            "the launch acknowledgment starts a background subagent; it is not the completion signal"
        );
    }

    #[test]
    fn subagent_terminated_at_uses_async_task_completion_notification() {
        let now = Utc::now();
        let completed_ts = now - chrono::Duration::seconds(1);
        let text = "<task-notification>\n\
<task-id>a04f87f14f439d3f3</task-id>\n\
<tool-use-id>toolu_done</tool-use-id>\n\
<status>completed</status>\n\
<summary>Agent came to rest</summary>\n\
</task-notification>";
        let parent_events = vec![mk_user_prompt_event(text, completed_ts)];

        assert_eq!(
            subagent_terminated_at(Some("toolu_done"), &parent_events, Some(now), now, 90_000),
            Some(completed_ts),
            "Claude async subagents finish via the parent task-notification completion record"
        );
    }

    // ── B1: folder/subagent naming ───────────────────────────────────────────────
    #[test]
    fn display_label_names_root_by_folder_and_subagent_by_ordinal() {
        // root with a folder → the folder name
        assert_eq!(
            display_label(0, Some("WARDEN"), None, None, "fallback"),
            "WARDEN"
        );
        // a second live root in the same folder → circled disambiguator (oldest keeps bare name)
        assert_eq!(
            display_label(0, Some("WARDEN"), None, Some(2), "fallback"),
            "WARDEN ②"
        );
        assert_eq!(
            display_label(0, Some("WARDEN"), None, Some(1), "fallback"),
            "WARDEN"
        );
        // root with no folder → falls back to the identity label
        assert_eq!(
            display_label(0, None, None, None, "diagnose the bug"),
            "diagnose the bug"
        );
        // subagent → strictly "subagent N", regardless of any role/description
        assert_eq!(
            display_label(1, Some("WARDEN"), Some(1), None, "Explore"),
            "subagent 1"
        );
        assert_eq!(display_label(2, None, Some(3), None, "x"), "subagent 3");
    }

    /// Seed: a live Claude ROOT that logged a `ToolResult` for `call_id` (the
    /// subagent's completion signal) + a Claude SUBAGENT under `/subagents/` carrying
    /// `meta.toolUseId == call_id`, linked to the root. The root's tool-result is
    /// timestamped at `result_ts` (a fixed point so the test can advance `now` around
    /// the grace window). Returns nothing; the root's external id is `{root}-ext`.
    fn seed_root_with_terminated_subagent(
        store: &Store,
        root: &str,
        sub: &str,
        call_id: &str,
        result_ts: DateTime<Utc>,
    ) {
        // Root session with a ToolResult event for `call_id`.
        let root_session = Session {
            id: root.into(),
            harness: Harness::ClaudeCode,
            external_id: format!("{root}-ext"),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/Users/k/Developer/MyRepo"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: result_ts - chrono::Duration::seconds(10),
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{root}.jsonl")),
            raw_hash: 0,
            ingested_at: result_ts,
            meta: serde_json::json!({}),
        };
        let root_turn = Turn {
            id: format!("{root}-t0"),
            session_id: root.into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: result_ts,
            duration_ms: None,
            is_sidechain: false,
        };
        let root_result = EventRecord {
            id: format!("{root}-res"),
            turn_id: format!("{root}-t0"),
            session_id: root.into(),
            ts: result_ts,
            event: Event::ToolResult {
                call_id: call_id.into(),
                status: ToolStatus::Ok,
                bytes: 0,
                summary: None,
            },
            raw_ref: RawRef {
                source_path: root_session.source_path.clone(),
                offset: 0,
                line: 1,
            },
        };
        store
            .upsert_session_batch(&root_session, &[root_turn], &[root_result], 0)
            .unwrap();

        // Subagent session under /subagents/ with meta.toolUseId == call_id.
        let sub_session = Session {
            id: sub.into(),
            harness: Harness::ClaudeCode,
            external_id: format!("{sub}-ext"),
            project: None,
            model_ids: vec![],
            started_at: result_ts - chrono::Duration::seconds(5),
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/proj/sess/subagents/agent-{sub}.jsonl")),
            raw_hash: 0,
            ingested_at: result_ts,
            meta: serde_json::json!({}),
        };
        store
            .upsert_session_batch(&sub_session, &[], &[], 0)
            .unwrap();
        store
            .merge_session_meta(sub, &serde_json::json!({ "toolUseId": call_id }))
            .unwrap();
        store.link_child_session(sub, root).unwrap();
    }

    /// Read back the timestamp of the root's `ToolResult` for `call_id` (the fixed t0
    /// the termination decision keys on).
    fn result_ts_of(store: &Store, root: &str, call_id: &str) -> DateTime<Utc> {
        store
            .session_events(root)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match &e.event {
                Event::ToolResult { call_id: c, .. } if c == call_id => Some(e.ts),
                _ => None,
            })
            .expect("root must carry the tool-result")
    }

    /// B4 end-to-end: a subagent whose parent logged its tool-result is emitted ONCE
    /// as `terminated` (within the grace window so the FACE can implode it), then
    /// DROPPED from the forest past the grace window, and stays dropped on every later
    /// recompute (a permanent fact ⇒ no resurrection).
    #[test]
    fn terminated_subagent_is_emitted_once_then_dropped_and_never_resurrects() {
        let store = Store::memory().unwrap();
        let t0 = Utc::now() - chrono::Duration::seconds(120); // a fixed past instant
        seed_root_with_terminated_subagent(&store, "root", "sub", "toolu_1", t0);
        let reg = claude_registry(&[(4242, "root-ext")]); // root is registry-open
        let t0 = result_ts_of(&store, "root", "toolu_1");

        // Within the 5s grace window → present as "terminated".
        let s1 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(1),
        );
        let sub = s1
            .agents
            .iter()
            .find(|a| a.id == "sub")
            .expect("present within grace");
        assert_eq!(sub.status, "terminated");

        // Past the grace window → dropped from the forest.
        let s2 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(30),
        );
        assert!(
            s2.agents.iter().all(|a| a.id != "sub"),
            "dropped past grace"
        );

        // Stays dropped (no resurrection) on a still-later recompute.
        let s3 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            t0 + chrono::Duration::seconds(60),
        );
        assert!(
            s3.agents.iter().all(|a| a.id != "sub"),
            "stays dropped (no resurrection)"
        );

        // The root itself is never terminated — it remains in the forest.
        assert!(s2.agents.iter().any(|a| a.id == "root"), "root persists");
    }
}
