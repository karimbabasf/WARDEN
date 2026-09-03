//! Top-level forest assembly: the pure, deterministic join of store sessions +
//! liveness + composition + identity into the final [`RadarState`]. Membership and
//! the clock are injected so this is unit-testable without real PIDs or a real clock;
//! the live filesystem orchestration lives in [`super::live`].

use super::agent::build_agent;
use super::identity::{
    display_label, dispatches_settled_at, parent_completion_at, subagent_terminated_at,
    teammate_dispatch_call_id,
};
use super::awaiting::AwaitingReason;
use super::liveness::{partition_claude, read_claude_registry, AgentStatus};
use super::model::RadarState;
use super::procs::{Liveness, ProcessIndex};
use super::status::{
    agent_status, agent_verdict, claude_conversation_status, codex_subagent_completed_at,
    transcript_is_missing, transcript_mtime_secs_ago, StatusVerdict,
};
use crate::ir::{Harness, Session};
use crate::store::Store;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Assemble the live agent forest from the store + the Claude liveness registry.
///
/// `is_alive`/`procs`/`now` are injected so the join is deterministic and unit-testable
/// without real PIDs, a real process table or a real clock. The forest is the set of store sessions;
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
    procs: &ProcessIndex,
    now: DateTime<Utc>,
) -> RadarState {
    let sessions = store.sessions().unwrap_or_default();

    // Liveness: map a Claude external session id → status from the registry.
    //
    // PID REUSE. `~/.claude/sessions/<pid>.json` is removed on a clean exit, so the
    // file lingering is itself the signature of a session that was killed, crashed or
    // OOM'd. `is_alive` alone reads that leftover as open again the moment the OS
    // hands its number to any other process, and because nothing ever revisits the
    // verdict the dead session's globe then stays for as long as the borrower runs.
    // The registry states `procStart` precisely so the number can be checked against
    // an identity, so check it: a pid the sweep saw starting at a DIFFERENT time is a
    // different process, and the row behind it is gone.
    //
    // Only ever subtracts. An unobserved pid, an absent `procStart` and a failed sweep
    // all leave the entry alone for `is_alive` to judge, so the worst case is exactly
    // the behaviour that shipped before.
    let registry: Vec<(u32, serde_json::Value)> = read_claude_registry(sessions_dir)
        .into_iter()
        .filter(|(pid, v)| {
            let claimed = v.get("procStart").and_then(|s| s.as_str());
            procs.is_same_process(*pid, claimed).unwrap_or(true)
        })
        .collect();
    let mtime_secs_ago = |sid: &str| transcript_mtime_secs_ago(&sessions, sid, now);
    // "The harness deleted this transcript", which is NOT the same as `mtime_secs_ago`
    // returning None (that also covers a session id we hold no row for).
    let transcript_gone = |sid: &str| transcript_is_missing(&sessions, sid);
    // FAULT B: when the registry carries no authoritative `status`, decide working/idle
    // from the session's CONVERSATION STATE (its last ingested event), not file mtime —
    // deterministic across reads, so the working↔idle flicker is gone. The closure
    // bridges the registry's external `sessionId` → the store row → its events.
    let stale_secs = crate::util::radar_working_stale_secs();
    let fallback_status = |ext: &str| {
        claude_conversation_status(store, &sessions, ext, now, stale_secs, &mtime_secs_ago)
    };
    let live = partition_claude(&registry, is_alive, &fallback_status);
    // The registry's `waitingFor` reason, folded into the closed vocabulary AT THE EDGE:
    // the raw harness string never travels past this line, so nothing downstream (and in
    // particular nothing the observer projection can reach) holds free dialog text.
    // A row with no `waitingFor` is deliberately absent rather than mapped to a default:
    // that is either a fallback-derived Awaiting or a registry that stated no reason, and
    // in both cases the transcript is a better source than a shrug.
    let claude_awaiting: HashMap<String, AwaitingReason> = live
        .iter()
        .filter(|(_, st)| matches!(st, AgentStatus::Awaiting))
        .filter_map(|(s, _)| {
            let reason = AwaitingReason::from_registry(Some(s.waiting_for.as_deref()?));
            Some((s.session_id.clone(), reason))
        })
        .collect();
    let claude_status: HashMap<String, AgentStatus> =
        live.into_iter().map(|(s, st)| (s.session_id, st)).collect();

    // parent link per session id (None = root). Built for every stored session so
    // we can resolve a subagent's root before deciding membership. ONE query, not
    // one per session: this runs on every recompute, and the per-session form was
    // most of what a recompute cost once the store held a few thousand sessions.
    let parent_of: HashMap<String, Option<String>> = store.parent_links().unwrap_or_default();

    // The OPEN FOREST: include a session ONLY if it is currently open (spec §3 "the
    // set of agent trees currently open", §5 "the live forest"). A root is directly
    // open when — Claude: its `external_id` is in the live registry partition (a dead
    // PID was already dropped by `partition_claude`); Codex: its rollout currently
    // lives under `~/.codex/sessions/` and has NOT been archived (`is_codex_open`).
    // A subagent is open iff the ROOT of its parent-chain is directly open — close
    // the root and the whole tree implodes; this also guarantees no kept subagent
    // ever dangles (its parent shares the same open root, so it is kept too).
    // The FILE-shaped openness rule, one arm per harness that has one. This is what
    // RADAR has always used, kept intact: Claude's registry partition, Codex's
    // "under sessions/ and not archived/".
    //
    // The catch-all leans on the process table below, which is the only honest
    // evidence available for a harness that publishes no registry and no archive.
    // It is a RECENCY test rather than the flat `true` it used to be, because `true`
    // means open forever: the table answers `Unknown` whenever `lsof` is denied, the
    // session row carries no cwd, or the running process is one the classifier cannot
    // name, and `Unknown` defers straight back to this rule. Every one of those cases
    // pinned a session from days ago onto the board with no way off it. Age it out on
    // the window Codex already uses, for the same reason Codex has one: with no
    // termination signal, "nothing has been written for hours" is the last honest cue.
    // A transcript that is GONE is closed outright, whatever the window says.
    let unsignalled_stale_secs = crate::util::radar_codex_stale_secs();
    let file_rule = |s: &Session| -> bool {
        match s.harness {
            Harness::ClaudeCode => claude_status.contains_key(&s.external_id),
            Harness::Codex => is_codex_open(s),
            _ => mtime_secs_ago(&s.external_id)
                .map(|secs| unsignalled_stale_secs == 0 || secs <= unsignalled_stale_secs)
                .unwrap_or(false),
        }
    };

    // PROCESS TRUTH, and the direction it is allowed to push.
    //
    // The table may only ever CLOSE a globe, never open one: `Open` and `Unknown`
    // both defer to the file rule, and only `Closed` overrides it. That asymmetry is
    // deliberate. Termination is the fault being fixed, and closing faster cannot
    // invent an agent that is not there, while letting the table open things would
    // add a brand new way to draw a phantom.
    //
    // Claude is exempt because its registry is strictly better evidence: it maps a
    // session id to an exact pid, where the table can only match on a directory. Two
    // Claude sessions in one folder are distinguishable to the registry and identical
    // to `lsof`, so overriding the registry here could only ever lose information.
    let directly_open = |s: &Session| -> bool {
        if matches!(s.harness, Harness::ClaudeCode) {
            return file_rule(s);
        }
        let cwd = s
            .project
            .as_ref()
            .map(|p| p.cwd.to_string_lossy().into_owned());
        match procs.resolve(&s.harness, cwd.as_deref()) {
            Liveness::Closed => false,
            Liveness::Open | Liveness::Unknown => file_rule(s),
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
    // A subagent has no PID, so liveness can't see it finish. How we derive "finished"
    // is harness-specific:
    //  * Claude: the parent logged a tool-result for the subagent's call (permanent, so
    //    idempotent), or the subagent fell silent past the 90s file backstop.
    //  * Codex Desktop: a subagent has NO tool_use_id, and it legitimately goes quiet
    //    for long stretches (minutes) while its orchestrator parent keeps running (it
    //    finished a step, or is waiting to be read). Its real lifecycle signal is the
    //    Codex openness policy (rollout present under ~/.codex/sessions/, not archived,
    //    and fresh), the SAME signal roots use. Applying the Claude file-silence
    //    backstop here wrongly imploded every idle subagent, so a parent orchestrating
    //    N subagents rendered with zero children. A Codex subagent is "finished" only
    //    when its own rollout stops being open (Codex archived it, or it went stale
    //    past the Codex window).
    // Within a grace window we EMIT the finished subagent as `terminated` (the FACE
    // implodes it); past the window we DROP it from the forest so it never lingers as
    // idle and never resurrects.
    let terminate_ms = crate::util::radar_subagent_terminate_ms();
    let grace_ms = crate::util::radar_terminate_grace_ms();
    let teammate_done_ms = crate::util::radar_teammate_done_ms();
    let mut terminated_now: HashSet<String> = HashSet::new();
    let mut terminated_drop: HashSet<String> = HashSet::new();
    // A lead's event set is large and several subagents share one lead, so fetch a
    // parent's events once per recompute rather than once per child.
    let mut parent_events_cache: HashMap<String, Vec<(crate::ir::Turn, crate::ir::EventRecord)>> =
        HashMap::new();
    for id in &keep {
        let Some(Some(parent)) = kept_parent.get(id) else {
            continue; // roots are never "terminated" (they Close instead)
        };
        let Some(child) = by_id.get(id.as_str()) else {
            continue;
        };
        // The child's last transcript write (file mtime), used as the termination
        // timestamp that drives the grace/implode window.
        let last = mtime_secs_ago(&child.external_id)
            .map(|secs| now - chrono::Duration::seconds(secs as i64));
        let terminated_at = if matches!(child.harness, Harness::Codex) {
            if !directly_open(child) {
                // Definitive close: Codex archived the rollout, or it went stale past
                // the Codex window. Retire it at its last write.
                Some(last.unwrap_or(now))
            } else {
                // Still open: retire only when its OWN transcript reports the spawned
                // task actually completed (a real `task_complete`, not a mid-task pause
                // or a between-turns idle). An idle-but-not-complete subagent stays
                // nested under its live parent. Never the file-silence backstop.
                codex_subagent_completed_at(&store.session_events(id).unwrap_or_default())
            }
        } else if is_in_process_teammate(child) && !has_tool_use_id(child) {
            // An in-process TEAMMATE carries no `toolUseId` in its sidecar and has no PID,
            // so it used to ride the lead's whole lifetime and left the board only when the
            // lead went idle. For a lead left working (a monitor, or an orchestrator doing
            // its own work after members returned) that means a finished member lingers for
            // hours, which is the "you cannot tell when a subagent is done" complaint.
            //
            // But a teammate IS spawned by an `Agent`/`Task` call, and the lead logs a
            // result (or a completed task-notification) for that call when the member
            // finishes. The sidecar merely drops the `toolUseId`; recover it by matching the
            // member's `name` to the lead's `Agent` call, then retire the member on that
            // completion EVEN WHILE THE LEAD WORKS. Gate on `member_idle` so a member that
            // was re-messaged and resumed (it reads Working) is never retired mid-run.
            //
            // Recomputed from live state every pass, so a member that speaks again simply
            // reappears; nothing here is persisted.
            let parent_events = parent_events_cache
                .entry(parent.clone())
                .or_insert_with(|| store.session_events(parent).unwrap_or_default())
                .as_slice();
            // "Finished its own turn", not "has nothing to say". `agent_status` promotes a
            // member that is mid-tool to Working, so anything quiet here closed its turn
            // cleanly. Awaiting counts as quiet on purpose: a member that ended on a
            // question is still done from the LEAD's point of view, and excluding it would
            // pin a finished teammate to the board for as long as its last line had a
            // question mark in it.
            let member_idle = matches!(
                agent_status(store, child, &claude_status, &mtime_secs_ago, now),
                AgentStatus::Idle | AgentStatus::Awaiting
            );
            let member_name = child
                .meta
                .get("memberName")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let explicit = member_idle
                .then(|| teammate_dispatch_call_id(member_name, parent_events))
                .flatten()
                .and_then(|cid| parent_completion_at(&cid, parent_events));
            if explicit.is_some() {
                explicit
            } else {
                // No recoverable dispatch (or the member is still assigned): fall back to
                // the honest AND. (1) the LEAD is idle, so the run is not mid-flight (a
                // member is quiet BETWEEN TURNS while the lead works, so a working lead
                // keeps every member). (2) the member's own last turn COMPLETED, not
                // dangling a tool call (`agent_status` promotes a mid-tool member to
                // Working, so Idle is exactly "finished its turn"). (3) it has stayed quiet
                // past the window. The 90s file-silence backstop stays OFF for teammates:
                // silence alone would implode a live team.
                let lead_working = by_id
                    .get(parent.as_str())
                    .and_then(|lead| claude_status.get(&lead.external_id))
                    .map(|st| matches!(st, AgentStatus::Working))
                    .unwrap_or(false);
                match last {
                    Some(last_ts) if !lead_working && member_idle => {
                        let quiet_ms =
                            now.signed_duration_since(last_ts).num_milliseconds().max(0) as u64;
                        (quiet_ms > teammate_done_ms).then(|| {
                            last_ts + chrono::Duration::milliseconds(teammate_done_ms as i64)
                        })
                    }
                    _ => None,
                }
            }
        } else {
            let tid = child
                .meta
                .get("toolUseId")
                .and_then(|v| v.as_str())
                .filter(|t| !t.is_empty());
            let parent_events = parent_events_cache
                .entry(parent.clone())
                .or_insert_with(|| store.session_events(parent).unwrap_or_default())
                .as_slice();
            subagent_terminated_at(tid, parent_events, last, now, terminate_ms).or_else(|| {
                // No `toolUseId`, so nothing above could match this child to its
                // dispatch, and the 90s file-silence backstop was the only rule left.
                // The parent knows sooner: see `dispatches_settled_at`. Restricted to
                // the no-id case so a child that HAS an id keeps waiting for its own
                // result rather than for its siblings'.
                tid.is_none()
                    .then(|| dispatches_settled_at(parent_events, child.started_at))
                    .flatten()
            })
        };
        // LAST RESORT: the harness DELETED the child's transcript.
        //
        // Every rule above is an inference from something that is still there. This one
        // is a fact, and it is the only rule that covers the case none of them can: a
        // subagent with no `toolUseId` to match against its parent and no member name to
        // recover one, whose file is gone and so has no mtime for the silence backstop to
        // measure. `subagent_terminated_at` answers None to that (`child_last_activity?`),
        // and None means "still running", so the globe stayed nested under its root for
        // the whole life of the root. On this machine 763 of the 821 subagent rows with
        // no other signal are in exactly that state, which is the bulk of the agents the
        // board was still drawing hours after they finished.
        //
        // Last rather than first because a parent's logged completion is a BETTER
        // timestamp than "the file is gone by now": it says when the agent finished, so
        // one that just returned still plays its implode. This arm only fires where the
        // alternative was never.
        let terminated_at = terminated_at.or_else(|| {
            transcript_gone(&child.external_id)
                .then(|| child.ended_at.unwrap_or(child.started_at))
        });
        if let Some(ts) = terminated_at {
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

    // Agent-team rosters, read once per recompute rather than once per agent. Absent on
    // machines with no teams, which collapses every join below to `None`.
    let team_index = super::teams::TeamIndex::load();

    // Build one agent per kept session. Depth is the parent-chain length within the
    // (kept) tree (root = 0). Iterate `sessions` for a stable, source-ordered forest.
    let mut agents = Vec::with_capacity(keep.len());
    for s in &sessions {
        if !keep.contains(&s.id) {
            continue;
        }
        let parent_id = kept_parent.get(&s.id).cloned().flatten();
        let depth = depth_of(&s.id, &kept_parent);
        let verdict = if terminated_now.contains(&s.id) {
            StatusVerdict::terminated()
        } else {
            agent_verdict(
                store,
                s,
                &claude_status,
                &claude_awaiting,
                &mtime_secs_ago,
                now,
            )
        };
        let mut agent = build_agent(
            store,
            s,
            parent_id,
            depth,
            *child_count.get(&s.id).unwrap_or(&0),
            verdict,
        );
        // Team join. A subagent transcript is named `agent-<name>@session-<hex>.jsonl`,
        // so its own file stem carries the membership; a lead is matched by session id.
        let stem = s
            .source_path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // The sidecar states the membership outright; the stem parse is the fallback
        // for transcripts written in the older `agent-<name>@session-<hex>` shape.
        let meta_str = |k: &str| s.meta.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let member = team_index
            .team_for_member(meta_str("teamName"), meta_str("memberName"))
            .or_else(|| team_index.team_for_member_stem(&stem));
        agent.team = match &member {
            Some((team, name)) => Some(super::teams::radar_team(team, Some(name), false)),
            None => team_index.team_for_lead(&s.external_id).map(|team| {
                super::teams::radar_team(team, team.lead_member_name.as_deref(), true)
            }),
        };

        // Label precedence, most specific first:
        //   1. a name the user set in WARDEN (they renamed it; nothing may override that),
        //   2. the agent's own team-roster name, the name its operator actually uses,
        //   3. the harness's own session title, else the derived positional label (cwd
        //      basename, or `subagent N`).
        // Without (2) every teammate reads as an interchangeable `subagent 3`, and
        // without the title in (3) every root reads as its folder even when the harness
        // has already named the session. This assignment runs AFTER `build_agent` and
        // overwrites `.label` unconditionally, so it is the ONLY seam where a naming
        // change takes effect.
        let user_name = s
            .meta
            .get("warden_display_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| crate::util::truncate_chars(v, 120));
        agent.label = match (user_name, &member) {
            (Some(name), _) => name,
            (None, Some((_, member_name))) => member_name.clone(),
            (None, None) => display_label(
                depth,
                agent.title.as_deref(),
                agent.cwd.as_deref(),
                subagent_ordinal.get(&s.id).copied(),
                root_dup_ordinal.get(&s.id).copied(),
                &agent.label,
            ),
        };
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

/// An in-process agent-team member, from the facts its own sidecar recorded. Either
/// signal alone is sufficient: `taskKind` is explicit, and a non-empty `teamName` is
/// only ever written for a roster member.
fn is_in_process_teammate(s: &Session) -> bool {
    s.meta.get("taskKind").and_then(|v| v.as_str()) == Some("in_process_teammate")
        || s.meta
            .get("teamName")
            .and_then(|v| v.as_str())
            .is_some_and(|t| !t.is_empty())
}

/// Was this subagent dispatched by a `Task`/`Agent` call we can match a result to?
fn has_tool_use_id(s: &Session) -> bool {
    s.meta
        .get("toolUseId")
        .and_then(|v| v.as_str())
        .is_some_and(|t| !t.is_empty())
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
    use crate::radar::live::{recompute_radar_state_with, refresh_live_context};
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
                    turn_complete: None,
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

    /// Like [`claude_registry`], but with an explicit per-session `status` ("busy" →
    /// Working, anything else → Idle), for tests where a LEAD's working/idle verdict is
    /// what decides a teammate's fate.
    fn claude_registry_status(entries: &[(u32, &str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (pid, sid, status) in entries {
            std::fs::write(
                dir.path().join(format!("{pid}.json")),
                serde_json::json!({ "pid": pid, "sessionId": sid, "cwd": "/work", "status": status })
                    .to_string(),
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

    /// `~/.claude/sessions/<pid>.json` carrying the `procStart` a real registry writes
    /// (UTC), for the pid-reuse guard.
    fn claude_registry_proc_start(pid: u32, sid: &str, started: DateTime<Utc>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{pid}.json")),
            serde_json::json!({
                "pid": pid,
                "sessionId": sid,
                "cwd": "/work",
                "procStart": started.format("%a %b %e %H:%M:%S %Y").to_string(),
            })
            .to_string(),
        )
        .unwrap();
        dir
    }

    /// A `ProcessIndex` that observed `pid` starting at `started`, the way one `ps`
    /// sweep reports it: local time, and with no agent process claimed at all (Claude's
    /// real argv[0]s are ones the classifier rejects).
    fn sweep_saw(pid: u32, started: DateTime<Utc>) -> ProcessIndex {
        ProcessIndex::with_starts(
            Vec::new(),
            Default::default(),
            [(
                pid,
                started
                    .with_timezone(&chrono::Local)
                    .format("%a %b %e %H:%M:%S %Y")
                    .to_string(),
            )]
            .into_iter()
            .collect(),
        )
    }

    /// A killed Claude session leaves its `<pid>.json` behind (only a clean exit removes
    /// it). `is_alive` alone then says "open" again as soon as the OS hands that number
    /// to anything else, and nothing ever revisits the verdict, so the dead session's
    /// globe stays for as long as the borrower runs. `procStart` is in the file for
    /// exactly this reason.
    #[test]
    fn a_recycled_pid_does_not_reopen_a_killed_session() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let started = now - chrono::Duration::hours(9);
        seed(&store, "root", "root-ext", Harness::ClaudeCode, Some("/work"), None);

        // Same pid, same start time: the session really is the one still running.
        let reg = claude_registry_proc_start(4242, "root-ext", started);
        let alive = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &sweep_saw(4242, started),
            now,
        );
        assert_eq!(alive.agents.len(), 1, "the live session must still render");

        // Same pid, a start time two minutes ago: the number was recycled and the
        // session behind that file died hours back.
        let recycled = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &sweep_saw(4242, now - chrono::Duration::minutes(2)),
            now,
        );
        assert!(
            recycled.agents.is_empty(),
            "a recycled pid must not reopen the dead session behind a leftover registry file"
        );

        // A sweep that never saw the pid leaves the old `is_alive` rule in charge.
        let unobserved = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &ProcessIndex::unscanned(),
            now,
        );
        assert_eq!(
            unobserved.agents.len(),
            1,
            "an unobserved pid is not evidence of death"
        );
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);

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
            &codex_all_open, &ProcessIndex::unscanned(),
            now);
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);
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
            &codex_all_open, &ProcessIndex::unscanned(),
            now + chrono::Duration::seconds(1));
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
        let state1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
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
        let state2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);

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
        // fresh and cache_write are reported separately: they bill at different
        // rates, so merging them made the cache-write premium unapplyable.
        assert_eq!(root.composition.exact.fresh, 2);
        assert_eq!(root.composition.exact.cache_write, 13_761);
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
            &is_codex_open, &ProcessIndex::unscanned(),
            Utc::now());

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

    /// Set a file's mtime to `secs_ago` seconds in the past via `touch -t` (local
    /// time), so `transcript_mtime_secs_ago` sees a genuinely silent transcript. Used
    /// to arm the file-silence backstop in the Codex subagent regression tests.
    fn set_old_mtime(path: &std::path::Path, secs_ago: i64) {
        let t = chrono::Local::now() - chrono::Duration::seconds(secs_ago);
        let stamp = t.format("%Y%m%d%H%M.%S").to_string();
        let ok = std::process::Command::new("touch")
            .args(["-t", &stamp])
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "touch -t must set the fixture's old mtime");
    }

    /// Regression, on the exact shape of a live agent team: an in-process TEAMMATE
    /// carries no `toolUseId`, because the lead spawns it when the roster is built
    /// rather than through a `Task` call, so no tool-result is ever logged for it and
    /// the 90s file-silence backstop is wrong (a teammate is quiet for minutes WHILE
    /// THE LEAD WORKS). Here the lead IS working, so every member stays however long it
    /// has been quiet: a live 17-member team must not implode to a childless lead. A
    /// PLAIN subagent in the same silence is still terminated by the backstop, so the
    /// exemption is scoped to the one case with no completion signal of its own. (The
    /// companion test below covers the other half: once the lead goes idle and a member
    /// is done and quiet, it is finally retired.)
    #[test]
    fn an_idle_in_process_teammate_stays_nested_while_a_plain_subagent_still_retires() {
        let dir = tempfile::tempdir().unwrap();
        let subs = dir.path().join("lead-session/subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let mate_path = subs.join("agent-amev-l1-253c6f98.jsonl");
        let plain_path = subs.join("agent-aplain-77.jsonl");
        for p in [&mate_path, &plain_path] {
            std::fs::write(p, "{}\n").unwrap();
            set_old_mtime(p, 3600);
        }

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let claude = |id: &str, ext: &str, path: PathBuf, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now - chrono::Duration::seconds(7200),
            ended_at: None,
            source_path: path,
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        let lead = claude(
            "lead",
            "lead-ext",
            dir.path().join("lead-session.jsonl"),
            serde_json::json!({}),
        );
        let mate = claude(
            "mate",
            "mate-ext",
            mate_path,
            serde_json::json!({
                "agentType": "mev-l1",
                "memberName": "mev-l1",
                "teamName": "session-9854a095",
                "taskKind": "in_process_teammate",
                "spawnDepth": 0,
            }),
        );
        let plain = claude("plain", "plain-ext", plain_path, serde_json::json!({}));
        for s in [&lead, &mate, &plain] {
            store.upsert_session_batch(s, &[], &[], 0).unwrap();
        }
        store.link_child_session("mate", "lead").unwrap();
        store.link_child_session("plain", "lead").unwrap();

        // The lead holds a PID and is WORKING: a member is quiet between turns while the
        // lead generates, so a working lead keeps every member on the board.
        let reg = claude_registry_status(&[(4242, "lead-ext", "busy")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);

        let mate = state
            .agents
            .iter()
            .find(|a| a.id == "mate")
            .expect("an idle teammate must stay in the forest while its lead works");
        assert_eq!(mate.parent_id.as_deref(), Some("lead"));
        assert_ne!(mate.status, "terminated", "a quiet teammate is not a finished one");
        assert!(
            state.agents.iter().all(|a| a.id != "plain"),
            "a plain subagent silent past the backstop is still retired"
        );
        let lead = state.agents.iter().find(|a| a.id == "lead").unwrap();
        assert_eq!(lead.child_count, 1, "the lead keeps the teammate as a child");
    }

    /// The other half of the teammate rule, and the one that answers "you cannot tell
    /// when a subagent is done". Once the LEAD goes idle (the run is not mid-flight) and
    /// a member has finished its turn and stayed quiet past the window, that member is
    /// really done and leaves the board, instead of riding the lead until the terminal
    /// is closed. A member still active stays, so a normal pause never implodes it.
    #[test]
    fn an_idle_teammate_under_an_idle_lead_retires_once_quiet_past_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let subs = dir.path().join("lead-session/subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let done_path = subs.join("agent-adone-11.jsonl");
        let active_path = subs.join("agent-aactive-22.jsonl");
        std::fs::write(&done_path, "{}\n").unwrap();
        std::fs::write(&active_path, "{}\n").unwrap();
        set_old_mtime(&done_path, 3600); // done its turn, quiet far past the window
        set_old_mtime(&active_path, 3); // still writing: a live member, not a finished one

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let mk = |id: &str, ext: &str, path: PathBuf, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now - chrono::Duration::seconds(7200),
            ended_at: None,
            source_path: path,
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        let teammate = serde_json::json!({
            "taskKind": "in_process_teammate",
            "teamName": "session-9854a095",
            "spawnDepth": 0,
        });
        let lead = mk(
            "lead",
            "lead-ext",
            dir.path().join("lead-session.jsonl"),
            serde_json::json!({}),
        );
        let done = mk("done", "done-ext", done_path, teammate.clone());
        let active = mk("active", "active-ext", active_path, teammate.clone());
        for s in [&lead, &done, &active] {
            store.upsert_session_batch(s, &[], &[], 0).unwrap();
        }
        store.link_child_session("done", "lead").unwrap();
        store.link_child_session("active", "lead").unwrap();

        // The lead is alive but IDLE: the run is over, so a finished member is free to go.
        let reg = claude_registry_status(&[(4242, "lead-ext", "idle")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);

        assert!(
            state.agents.iter().all(|a| a.id != "done"),
            "a member done and quiet under an idle lead must finally retire"
        );
        let active = state
            .agents
            .iter()
            .find(|a| a.id == "active")
            .expect("a member still writing is live, never retired by the quiet window");
        assert_eq!(active.parent_id.as_deref(), Some("lead"));
        let lead = state.agents.iter().find(|a| a.id == "lead").unwrap();
        assert_eq!(lead.child_count, 1, "only the still-live member is a child");
    }

    /// Build one open Codex root sitting in `cwd`, plus the file rule that calls it
    /// open. Shared by the process-liveness tests below so they differ ONLY in what
    /// the process table says.
    fn open_codex_root_in(cwd: &str) -> (Store, Session, DateTime<Utc>) {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let s = Session {
            id: "cx-root".into(),
            harness: Harness::Codex,
            external_id: "thread-root".into(),
            project: Some(crate::ir::ProjectRef {
                cwd: std::path::PathBuf::from(cwd),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: std::path::PathBuf::from("/tmp/rollout-root.jsonl"),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        store.upsert_session_batch(&s, &[], &[], 0).unwrap();
        (store, s, now)
    }

    fn codex_proc(pid: u32) -> crate::radar::procs::AgentProcess {
        crate::radar::procs::AgentProcess {
            harness: Harness::Codex,
            pid,
            ppid: 1,
            started_at: "Sun Aug 30 02:10:32 2026".into(),
            argv0: "codex".into(),
        }
    }

    /// THE TERMINATION FIX. Codex publishes no pid, so its only file-shaped signal
    /// that an agent is gone is the rollout being moved to `archived/`, which does
    /// not happen when the agent is killed. The file rule below says OPEN, and the
    /// globe used to stay on the board indefinitely on the strength of it.
    ///
    /// With a sweep that ran and found no Codex process, the agent is closed on this
    /// recompute instead.
    #[test]
    fn a_codex_session_with_no_live_process_closes_immediately() {
        let (store, _s, now) = open_codex_root_in("/repo/alpha");
        let is_codex_open = |_: &Session| true; // the file rule still says OPEN
        let procs = ProcessIndex::new(Vec::new(), Default::default());
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            &procs,
            now,
        );
        assert!(
            state.agents.is_empty(),
            "a Codex agent whose process is gone must implode, not wait for the archive"
        );
    }

    /// The other direction, so the fix cannot pass by simply closing everything: a
    /// live Codex process in the session's own directory keeps its globe.
    #[test]
    fn a_codex_session_with_a_live_process_stays_open() {
        let (store, _s, now) = open_codex_root_in("/repo/alpha");
        let is_codex_open = |_: &Session| true;
        let procs = ProcessIndex::new(
            vec![codex_proc(4242)],
            [(4242u32, "/repo/alpha".to_string())].into_iter().collect(),
        );
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            &procs,
            now,
        );
        assert_eq!(state.agents.len(), 1, "a running Codex agent still renders");
    }

    /// Two agents of one harness, one killed. The survivor must not hold the dead
    /// one's globe open, which is what a coarse "is the harness running at all"
    /// rule would have done.
    #[test]
    fn a_killed_codex_session_closes_while_its_sibling_survives() {
        let (store, _s, now) = open_codex_root_in("/repo/killed");
        let is_codex_open = |_: &Session| true;
        // The only live Codex process is working somewhere else entirely.
        let procs = ProcessIndex::new(
            vec![codex_proc(4242)],
            [(4242u32, "/repo/survivor".to_string())]
                .into_iter()
                .collect(),
        );
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            &procs,
            now,
        );
        assert!(
            state.agents.is_empty(),
            "a sibling in another directory must not keep a killed agent alive"
        );
    }

    /// The safety valve at the assemble level: a FAILED sweep must leave the board
    /// exactly as the file rules left it, never implode it.
    #[test]
    fn an_unscanned_process_table_leaves_the_file_rules_in_charge() {
        let (store, _s, now) = open_codex_root_in("/repo/alpha");
        let is_codex_open = |_: &Session| true;
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &is_codex_open,
            &ProcessIndex::unscanned(),
            now,
        );
        assert_eq!(
            state.agents.len(),
            1,
            "a failed process sweep must not empty the radar"
        );
    }

    /// The latent multi-harness bug, as a test. A Grok session used to be looked up
    /// in Claude's registry by the `_ =>` fall-through, match nothing, and be
    /// undrawable however healthy it was. It now leans on the process table, which
    /// is the only evidence a harness without a file rule has.
    #[test]
    fn a_harness_with_no_file_rule_opens_on_its_process() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        // A REAL transcript on disk: a harness with no file rule is now aged out on
        // transcript recency (see `file_rule`), so a session whose transcript never
        // existed is closed before the process table is ever consulted, and this test
        // would be asserting the wrong thing.
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("chat_history.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        let s = Session {
            id: "gk-root".into(),
            harness: Harness::Grok,
            external_id: "grok-session-1".into(),
            project: Some(crate::ir::ProjectRef {
                cwd: std::path::PathBuf::from("/repo/grok"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: transcript,
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&s, &[], &[], 0).unwrap();

        let running = ProcessIndex::new(
            vec![crate::radar::procs::AgentProcess {
                harness: Harness::Grok,
                pid: 77,
                ppid: 1,
                started_at: "Sun Aug 30 02:10:32 2026".into(),
                argv0: "grok".into(),
            }],
            [(77u32, "/repo/grok".to_string())].into_iter().collect(),
        );
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &|_| false,
            &running,
            now,
        );
        assert_eq!(state.agents.len(), 1, "a running Grok session must render");

        let gone = ProcessIndex::new(Vec::new(), Default::default());
        let state = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &|_| false,
            &gone,
            now,
        );
        assert!(state.agents.is_empty(), "and must close when it exits");
    }

    /// Regression: a Codex Desktop subagent silent far longer than the Claude 90s file
    /// backstop, but whose own rollout is still open (present, not archived, fresh),
    /// must stay nested under its parent instead of being imploded. This is the real
    /// 4-subagent Codex Desktop case: the orchestrator parent keeps running while each
    /// spawned subagent goes quiet between steps. Before the harness-aware termination
    /// fix, every such subagent was dropped and the parent rendered with zero children.
    /// Uses a REAL hour-old transcript file so the silence backstop is genuinely armed.
    #[test]
    fn codex_idle_but_open_subagent_stays_nested() {
        let dir = tempfile::tempdir().unwrap();
        let child_path = dir.path().join("rollout-child.jsonl");
        std::fs::write(&child_path, "{}\n").unwrap();
        set_old_mtime(&child_path, 3600);

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let parent = Session {
            id: "cx-parent".into(),
            harness: Harness::Codex,
            external_id: "thread-parent".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: dir.path().join("rollout-parent.jsonl"),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({ "originator": "Codex Desktop" }),
        };
        let child = Session {
            id: "cx-child".into(),
            harness: Harness::Codex,
            external_id: "thread-child".into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: child_path,
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({
                "thread_source": "subagent",
                "parent_thread_id": "thread-parent",
                "agent_nickname": "Dirac",
                "originator": "Codex Desktop",
            }),
        };
        store.upsert_session_batch(&parent, &[], &[], 0).unwrap();
        store.upsert_session_batch(&child, &[], &[], 0).unwrap();
        relink_store_subagents(&store);

        // Both rollouts are open (present, not archived, fresh) in the Codex sense.
        let open = ["thread-parent", "thread-child"];
        let is_codex_open = |s: &Session| open.contains(&s.external_id.as_str());
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &is_codex_open, &ProcessIndex::unscanned(), now);

        let child = state
            .agents
            .iter()
            .find(|a| a.id == "cx-child")
            .expect("idle-but-open Codex subagent must stay in the forest");
        assert_eq!(child.parent_id.as_deref(), Some("cx-parent"));
        assert_eq!(child.depth, 1, "renders nested, not dropped");
        assert_eq!(child.nickname.as_deref(), Some("Dirac"));
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 1, "parent shows its live subagent");
    }

    /// Honest counterpart: a Codex subagent whose own rollout is NO LONGER open (Codex
    /// archived it, or it went stale past the Codex window) is finished, so it is
    /// retired from the forest even though its parent stays open. Proves the fix does
    /// not make subagents immortal: openness-of-own-rollout, not mere membership under
    /// an open root, is what keeps a Codex subagent alive.
    #[test]
    fn codex_closed_subagent_is_retired() {
        let dir = tempfile::tempdir().unwrap();
        let child_path = dir.path().join("rollout-child.jsonl");
        std::fs::write(&child_path, "{}\n").unwrap();
        set_old_mtime(&child_path, 3600); // silent an hour → past the 5s implode grace

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let mk = |id: &str, ext: &str, path: std::path::PathBuf, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::Codex,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: path,
            raw_hash: 0,
            ingested_at: now,
            meta,
        };
        store
            .upsert_session_batch(
                &mk(
                    "cx-parent",
                    "thread-parent",
                    dir.path().join("rollout-parent.jsonl"),
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
                    child_path,
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
        relink_store_subagents(&store);

        // Parent open; child's own rollout CLOSED (archived or stale).
        let is_codex_open = |s: &Session| s.external_id == "thread-parent";
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &is_codex_open, &ProcessIndex::unscanned(), now);

        assert!(
            state.agents.iter().all(|a| a.id != "cx-child"),
            "a closed (archived/stale) Codex subagent must be retired from the forest"
        );
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 0);
    }

    /// Build a Codex subagent (child of `thread-parent`) whose transcript is `events`,
    /// under a single closed turn `t1`, with both parent and child sessions in `store`.
    /// Returns nothing; the caller assembles and inspects.
    fn seed_codex_subagent_with_events(store: &Store, now: DateTime<Utc>, events: Vec<EventRecord>) {
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
        let turn = Turn {
            id: "t1".into(),
            session_id: "cx-child".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        store
            .upsert_session_batch(
                &mk("cx-parent", "thread-parent", serde_json::json!({ "originator": "Codex Desktop" })),
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
                        "agent_nickname": "Dirac",
                        "originator": "Codex Desktop",
                    }),
                ),
                &[turn],
                &events,
                0,
            )
            .unwrap();
        relink_store_subagents(store);
    }

    fn codex_child_event(offset: u64, ts: DateTime<Utc>, event: Event) -> EventRecord {
        EventRecord {
            id: format!("cx-child-e{offset}"),
            turn_id: "t1".into(),
            session_id: "cx-child".into(),
            ts,
            event,
            raw_ref: RawRef {
                source_path: PathBuf::from("/tmp/cx-child.jsonl"),
                offset,
                line: offset as u32,
            },
        }
    }

    /// A Codex subagent whose OWN transcript reported `task_complete` (its spawned task
    /// is done) is retired from the forest even though its rollout is still present on
    /// disk (is_codex_open = true) and its parent stays open. This is the "tell when a
    /// subagent is completely finished" signal: it fires on the real task_complete
    /// record, so the finished subagent implodes/retires instead of lingering as idle.
    #[test]
    fn codex_subagent_retires_on_task_complete_signal() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let done = now - chrono::Duration::minutes(30); // finished 30 min ago
        let events = vec![
            codex_child_event(10, done, Event::AssistantText { text: "final result".into(), turn_complete: None }),
            codex_child_event(
                20,
                done,
                Event::SystemNotice {
                    subtype: "codex_task_complete".into(),
                    data: serde_json::Value::Null,
                },
            ),
        ];
        seed_codex_subagent_with_events(&store, now, events);

        // Both rollouts still present (open); the child reported task_complete.
        let is_codex_open =
            |s: &Session| ["thread-parent", "thread-child"].contains(&s.external_id.as_str());
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &is_codex_open, &ProcessIndex::unscanned(), now);

        assert!(
            state.agents.iter().all(|a| a.id != "cx-child"),
            "a subagent that reported task_complete must retire even while its rollout is open"
        );
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 0, "finished subagent no longer counts");
    }

    /// A Codex subagent still mid-task (its newest event is a tool call, no trailing
    /// task_complete) stays nested even if its file has been quiet, because the
    /// completion signal has not fired. Guards against retiring a working subagent.
    #[test]
    fn codex_working_subagent_without_task_complete_stays_nested() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let quiet = now - chrono::Duration::minutes(30);
        let events = vec![
            codex_child_event(10, quiet, Event::AssistantText { text: "thinking out loud".into(), turn_complete: None }),
            codex_child_event(
                20,
                quiet,
                Event::ToolCall {
                    tool: "exec".into(),
                    input: serde_json::Value::Null,
                    call_id: "c1".into(),
                    kind: ToolKind::Unknown,
                },
            ),
        ];
        seed_codex_subagent_with_events(&store, now, events);

        let is_codex_open =
            |s: &Session| ["thread-parent", "thread-child"].contains(&s.external_id.as_str());
        let state = assemble(&store, Path::new("/no/registry"), &|_| true, &is_codex_open, &ProcessIndex::unscanned(), now);

        let child = state
            .agents
            .iter()
            .find(|a| a.id == "cx-child")
            .expect("a mid-task Codex subagent (no task_complete) must stay nested");
        assert_eq!(child.depth, 1);
        let parent = state.agents.iter().find(|a| a.id == "cx-parent").unwrap();
        assert_eq!(parent.child_count, 1);
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
        let _ = recompute_radar_state_with(&store, registry.path(), &ProcessIndex::unscanned());

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
        let state = recompute_radar_state_with(&store, claude_registry.path(), &ProcessIndex::unscanned());

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
        let state = recompute_radar_state_with(&store, claude_registry.path(), &ProcessIndex::unscanned());

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
        let state = recompute_radar_state_with(&store, registry.path(), &ProcessIndex::unscanned());

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
                .any(|a| a.kind == "read" && a.label.contains("orbLayout.ts")),
            "a Read must be classified `read` and name the file it touches, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "run" && a.label.contains("cargo test radar")),
            "a Bash must be classified `run` and name the command, got {acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| a.kind == "run" && a.label.contains("cargo build")),
            "a Codex exec must be classified `run` and name the command, got {acts:?}"
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
                    turn_complete: None,
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
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

    /// `est_cost_usd` bills cache reads 10x cheaper than fresh input. The RATES here
    /// were corrected on 2026-07-27: this test used to assert $15.00 input and $1.50
    /// cache-read, which were Opus 4.1-era numbers. Opus 4.5 and newer (including
    /// 4.8) are $5.00 input, so cache read is 0.1x that, $0.50. The 10x relationship
    /// is the invariant being tested; the absolute numbers just have to track the
    /// real table in `radar/pricing.rs`.
    #[test]
    fn est_cost_bills_cache_read_cheaper_than_fresh() {
        let model = Some("claude-opus-4-8".to_string());

        // Pure cache-read: 1M tokens at the cache-read rate (0.1x input).
        let cache_only = composition::ExactComposition {
            cache_read: 1_000_000,
            fresh: 0,
            cache_write: 0,
            output: 0,
        };
        let cost = est_cost_usd(&model, &cache_only).expect("opus -> a cost");
        assert!(
            (cost - 0.50).abs() < 1e-6,
            "1M cache-read tokens bill at the cache-read rate ($0.50), got {cost}"
        );

        // Pure fresh input: 1M tokens at the full input rate.
        let fresh_only = composition::ExactComposition {
            cache_read: 0,
            fresh: 1_000_000,
            cache_write: 0,
            output: 0,
        };
        let fresh_cost = est_cost_usd(&model, &fresh_only).expect("opus -> a cost");
        assert!(
            (fresh_cost - 5.0).abs() < 1e-6,
            "1M fresh tokens bill at the input rate ($5.00), got {fresh_cost}"
        );
        // The invariant that actually matters, independent of the absolute rates.
        assert!(
            (fresh_cost / cost - 10.0).abs() < 1e-6,
            "cache reads bill at exactly 0.1x input, got a ratio of {}",
            fresh_cost / cost
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
            &codex_all_open, &ProcessIndex::unscanned(),
            Utc::now());
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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());

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
                turn_complete: None,
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

        let s1 = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);
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
        let s2 = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);
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
            &codex_all_open, &ProcessIndex::unscanned(),
            base + chrono::Duration::seconds(240));

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
            &codex_all_open, &ProcessIndex::unscanned(),
            base + chrono::Duration::seconds(60));

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
                turn_complete: None,
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
            &codex_all_open, &ProcessIndex::unscanned(),
            base + chrono::Duration::seconds(60));

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
                turn_complete: None,
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
            &codex_all_open, &ProcessIndex::unscanned(),
            patch_ts + chrono::Duration::seconds(20));

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
            &is_codex_open, &ProcessIndex::unscanned(),
            Utc::now());

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
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());

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
            display_label(0, None, Some("WARDEN"), None, None, "fallback"),
            "WARDEN"
        );
        // a second live root in the same folder → circled disambiguator (oldest keeps bare name)
        assert_eq!(
            display_label(0, None, Some("WARDEN"), None, Some(2), "fallback"),
            "WARDEN ②"
        );
        assert_eq!(
            display_label(0, None, Some("WARDEN"), None, Some(1), "fallback"),
            "WARDEN"
        );
        // root with no folder → falls back to the identity label
        assert_eq!(
            display_label(0, None, None, None, None, "diagnose the bug"),
            "diagnose the bug"
        );
        // subagent → strictly "subagent N", regardless of any role/description
        assert_eq!(
            display_label(1, None, Some("WARDEN"), Some(1), None, "Explore"),
            "subagent 1"
        );
        assert_eq!(
            display_label(2, None, None, Some(3), None, "x"),
            "subagent 3"
        );
    }

    /// The harness's own session name beats the folder: two agents in one repo read as
    /// what each is doing, not as "WARDEN" and "WARDEN ②".
    #[test]
    fn display_label_prefers_the_harness_session_title() {
        assert_eq!(
            display_label(
                0,
                Some("Fix slow tailorings in trigger"),
                Some("WARDEN"),
                None,
                Some(2),
                "fallback"
            ),
            "Fix slow tailorings in trigger"
        );
        // Blank/whitespace title is not a name: fall through to the folder.
        assert_eq!(
            display_label(0, Some("   "), Some("WARDEN"), None, None, "fallback"),
            "WARDEN"
        );
        // A subagent keeps its ordinal: a title belongs to the session, and inheriting
        // the parent's would make every child read as the parent.
        assert_eq!(
            display_label(1, Some("Parent title"), Some("WARDEN"), Some(2), None, "x"),
            "subagent 2"
        );
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

    /// Seed a registry-open Claude ROOT plus one SUBAGENT under it that carries no
    /// `toolUseId` and no member name, so nothing but the file rules can retire it.
    /// `sub_path` is used verbatim as the child's transcript, `dispatch` optionally adds
    /// an `Agent` tool-call (and its result) to the root.
    fn seed_root_with_signalless_subagent(
        store: &Store,
        sub_path: PathBuf,
        sub_started: DateTime<Utc>,
        sub_ended: Option<DateTime<Utc>>,
        dispatch: Option<(DateTime<Utc>, Option<DateTime<Utc>>)>,
    ) {
        let root = Session {
            id: "root".into(),
            harness: Harness::ClaudeCode,
            external_id: "root-ext".into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/work"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: sub_started - chrono::Duration::seconds(60),
            ended_at: None,
            source_path: PathBuf::from("/tmp/root-signalless.jsonl"),
            raw_hash: 0,
            ingested_at: sub_started,
            meta: serde_json::json!({}),
        };
        let turn = Turn {
            id: "root-t0".into(),
            session_id: "root".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: root.started_at,
            duration_ms: None,
            is_sidechain: false,
        };
        let mut events = Vec::new();
        if let Some((call_ts, result_ts)) = dispatch {
            events.push(EventRecord {
                id: "root-call".into(),
                turn_id: "root-t0".into(),
                session_id: "root".into(),
                ts: call_ts,
                event: Event::ToolCall {
                    tool: "Agent".into(),
                    input: serde_json::json!({ "description": "sweep" }),
                    call_id: "toolu_dispatch".into(),
                    kind: crate::ir::ToolKind::SubagentTask,
                },
                raw_ref: RawRef {
                    source_path: root.source_path.clone(),
                    offset: 0,
                    line: 1,
                },
            });
            if let Some(result_ts) = result_ts {
                events.push(EventRecord {
                    id: "root-res".into(),
                    turn_id: "root-t0".into(),
                    session_id: "root".into(),
                    ts: result_ts,
                    event: Event::ToolResult {
                        call_id: "toolu_dispatch".into(),
                        status: ToolStatus::Ok,
                        bytes: 0,
                        summary: None,
                    },
                    raw_ref: RawRef {
                        source_path: root.source_path.clone(),
                        offset: 1,
                        line: 2,
                    },
                });
            }
        }
        store
            .upsert_session_batch(&root, &[turn], &events, 0)
            .unwrap();

        let sub = Session {
            id: "sub".into(),
            harness: Harness::ClaudeCode,
            external_id: "sub-ext".into(),
            project: None,
            model_ids: vec![],
            started_at: sub_started,
            ended_at: sub_ended,
            source_path: sub_path,
            raw_hash: 0,
            ingested_at: sub_started,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&sub, &[], &[], 0).unwrap();
        store.link_child_session("sub", "root").unwrap();
    }

    /// The reported bug, at its largest. A subagent with no `toolUseId` and no member
    /// name has only the file rules left, and when the harness has DELETED its
    /// transcript those rules had nothing to work with: no id to match, and no mtime for
    /// the 90s silence backstop to measure, so `subagent_terminated_at` answered None and
    /// None means "still running". The globe then stayed nested under its root for the
    /// root's whole life. 763 of the 821 subagent rows on this machine with no other
    /// signal are in that state.
    #[test]
    fn a_subagent_whose_transcript_was_deleted_is_retired() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("subagents/agent-gone.jsonl"); // never created
        seed_root_with_signalless_subagent(
            &store,
            gone,
            now - chrono::Duration::hours(3),
            Some(now - chrono::Duration::hours(2)),
            None,
        );
        let reg = claude_registry(&[(4242, "root-ext")]);
        let state = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &ProcessIndex::unscanned(),
            now,
        );
        assert!(
            state.agents.iter().all(|a| a.id != "sub"),
            "a subagent whose transcript the harness deleted must not still be drawn"
        );
        let root = state.agents.iter().find(|a| a.id == "root").unwrap();
        assert_eq!(root.child_count, 0, "and it must not still be counted");
    }

    /// A transcript that is still on disk and still fresh is NOT a deleted one: the
    /// child stays. Guards the rule above against closing live agents.
    #[test]
    fn a_subagent_whose_transcript_is_still_there_stays() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-live.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        seed_root_with_signalless_subagent(
            &store,
            path,
            now - chrono::Duration::seconds(20),
            None,
            None,
        );
        let reg = claude_registry(&[(4242, "root-ext")]);
        let state = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &ProcessIndex::unscanned(),
            now,
        );
        assert!(
            state.agents.iter().any(|a| a.id == "sub"),
            "a running subagent still has its transcript and must stay on the board"
        );
    }

    /// The other half of the complaint: not late, but not INSTANT. A subagent with no
    /// `toolUseId` used to need 90 seconds of file silence before it was called
    /// finished, even though the parent had already logged the result of the dispatch
    /// that launched it. The transcript here is fresh (one second old), so the silence
    /// backstop cannot fire and the parent's result is the only thing that can retire
    /// this child.
    #[test]
    fn a_signalless_subagent_retires_on_its_parents_dispatch_result() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-x.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let reg = claude_registry(&[(4242, "root-ext")]);
        let started = now - chrono::Duration::seconds(30);

        // Dispatch logged, no result yet: the child is out and must stay.
        seed_root_with_signalless_subagent(
            &store,
            path.clone(),
            started,
            None,
            Some((started - chrono::Duration::seconds(1), None)),
        );
        let open = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &ProcessIndex::unscanned(),
            now,
        );
        assert!(
            open.agents.iter().any(|a| a.id == "sub"),
            "an open dispatch means the subagent is still running"
        );

        // The dispatch returns. The child is finished NOW, not in 90 seconds.
        let store = Store::memory().unwrap();
        seed_root_with_signalless_subagent(
            &store,
            path,
            started,
            None,
            Some((
                started - chrono::Duration::seconds(1),
                Some(now - chrono::Duration::seconds(1)),
            )),
        );
        let done = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open,
            &ProcessIndex::unscanned(),
            now,
        );
        let sub = done
            .agents
            .iter()
            .find(|a| a.id == "sub")
            .expect("still emitted, inside the implode grace");
        assert_eq!(
            sub.status, "terminated",
            "the parent's result retires the child on the pass that ingests it"
        );
    }

    /// A harness with no file rule of its own (Grok here) used to answer a flat `true`
    /// to "is this open", which is open forever: the process table is its only close
    /// signal and it answers `Unknown` whenever `lsof` is denied or the row carries no
    /// cwd. Age it out on the same window Codex uses.
    #[test]
    fn a_harness_with_no_file_rule_ages_out_on_a_stale_transcript() {
        let store = Store::memory().unwrap();
        let now = Utc::now();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let s = Session {
            id: "gk".into(),
            harness: Harness::Grok,
            external_id: "grok-1".into(),
            project: None, // no cwd: the process table can only answer Unknown
            model_ids: vec![],
            started_at: now - chrono::Duration::hours(30),
            ended_at: None,
            source_path: path.clone(),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        store.upsert_session_batch(&s, &[], &[], 0).unwrap();
        let running = ProcessIndex::new(
            vec![crate::radar::procs::AgentProcess {
                harness: Harness::Grok,
                pid: 77,
                ppid: 1,
                started_at: "Sun Aug 30 02:10:32 2026".into(),
                argv0: "grok".into(),
            }],
            Default::default(),
        );

        set_old_mtime(&path, 30 * 3600); // 30h, past the 6h window
        let stale = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &|_| false,
            &running,
            now,
        );
        assert!(
            stale.agents.is_empty(),
            "a session untouched for 30h must not still be drawn on a bare Unknown"
        );

        set_old_mtime(&path, 60); // a minute ago
        let fresh = assemble(
            &store,
            Path::new("/no/registry"),
            &|_| true,
            &|_| false,
            &running,
            now,
        );
        assert_eq!(fresh.agents.len(), 1, "a live session must still render");
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
            &codex_all_open, &ProcessIndex::unscanned(),
            t0 + chrono::Duration::seconds(1));
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
            &codex_all_open, &ProcessIndex::unscanned(),
            t0 + chrono::Duration::seconds(30));
        assert!(
            s2.agents.iter().all(|a| a.id != "sub"),
            "dropped past grace"
        );

        // Stays dropped (no resurrection) on a still-later recompute.
        let s3 = assemble(
            &store,
            reg.path(),
            &|_| true,
            &codex_all_open, &ProcessIndex::unscanned(),
            t0 + chrono::Duration::seconds(60));
        assert!(
            s3.agents.iter().all(|a| a.id != "sub"),
            "stays dropped (no resurrection)"
        );

        // The root itself is never terminated — it remains in the forest.
        assert!(s2.agents.iter().any(|a| a.id == "root"), "root persists");
    }

    /// Regression for "you cannot tell when a subagent is done". An in-process TEAMMATE is
    /// spawned by an `Agent` call, and the lead logs a tool-result for that call when the
    /// member finishes — a precise per-member completion signal. The sidecar drops the
    /// `toolUseId`, but matching the member's `name` to the lead's `Agent` call recovers
    /// it, so a finished member leaves the board EVEN WHILE THE LEAD IS STILL WORKING (a
    /// monitor or orchestrator left open). A sibling member with no logged completion is
    /// untouched: the working-lead exemption still holds for a member merely quiet between
    /// turns, so a live team never implodes to a childless lead.
    #[test]
    fn an_in_process_teammate_retires_on_its_agent_result_even_while_the_lead_works() {
        let dir = tempfile::tempdir().unwrap();
        let subs = dir.path().join("lead-session/subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let done_path = subs.join("agent-amev-l1-253c6f98.jsonl");
        let live_path = subs.join("agent-aoracles-f3be9d69.jsonl");
        for p in [&done_path, &live_path] {
            std::fs::write(p, "{}\n").unwrap();
            // Both are quiet on disk; only the LEAD's logged result distinguishes them.
            set_old_mtime(p, 3600);
        }

        let store = Store::memory().unwrap();
        let now = Utc::now();
        let result_ts = now - chrono::Duration::seconds(120); // past the 5s grace → dropped
        let mk = |id: &str, ext: &str, path: PathBuf, meta: serde_json::Value| Session {
            id: id.into(),
            harness: Harness::ClaudeCode,
            external_id: ext.into(),
            project: None,
            model_ids: vec![],
            started_at: now - chrono::Duration::seconds(7200),
            ended_at: None,
            source_path: path,
            raw_hash: 0,
            ingested_at: now,
            meta,
        };

        // The LEAD carries the `Agent` dispatch + its tool-result for member "mev-l1" only.
        let lead = mk(
            "lead",
            "lead-ext",
            dir.path().join("lead-session.jsonl"),
            serde_json::json!({}),
        );
        let lead_turn = Turn {
            id: "lead-t0".into(),
            session_id: "lead".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: result_ts - chrono::Duration::seconds(10),
            duration_ms: None,
            is_sidechain: false,
        };
        let raw = |off: u64| RawRef {
            source_path: lead.source_path.clone(),
            offset: off,
            line: (off + 1) as u32,
        };
        let lead_call = EventRecord {
            id: "lead-call".into(),
            turn_id: "lead-t0".into(),
            session_id: "lead".into(),
            ts: result_ts - chrono::Duration::seconds(5),
            event: Event::ToolCall {
                tool: "Agent".into(),
                input: serde_json::json!({ "name": "mev-l1", "subagent_type": "mev-l1" }),
                call_id: "toolu_mev".into(),
                kind: crate::ir::ToolKind::SubagentTask,
            },
            raw_ref: raw(0),
        };
        let lead_result = EventRecord {
            id: "lead-res".into(),
            turn_id: "lead-t0".into(),
            session_id: "lead".into(),
            ts: result_ts,
            event: Event::ToolResult {
                call_id: "toolu_mev".into(),
                status: ToolStatus::Ok,
                bytes: 0,
                summary: None,
            },
            raw_ref: raw(1),
        };
        store
            .upsert_session_batch(&lead, &[lead_turn], &[lead_call, lead_result], 0)
            .unwrap();

        let teammate = |name: &str| {
            serde_json::json!({
                "taskKind": "in_process_teammate",
                "teamName": "session-9854a095",
                "memberName": name,
                "agentType": name,
                "spawnDepth": 0,
            })
        };
        let done = mk("done", "done-ext", done_path, teammate("mev-l1"));
        let live = mk("live", "live-ext", live_path, teammate("oracles"));
        for s in [&done, &live] {
            store.upsert_session_batch(s, &[], &[], 0).unwrap();
        }
        store.link_child_session("done", "lead").unwrap();
        store.link_child_session("live", "lead").unwrap();

        // The lead is alive and BUSY — the old rule held BOTH members on the board.
        let reg = claude_registry_status(&[(4242, "lead-ext", "busy")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), now);

        // "mev-l1" has a logged Agent result → retired despite the busy lead.
        assert!(
            state.agents.iter().all(|a| a.id != "done"),
            "a teammate whose Agent dispatch the lead has completed must retire even while \
             the lead works"
        );
        // "oracles" has no logged completion → the working-lead exemption keeps it nested.
        let live = state
            .agents
            .iter()
            .find(|a| a.id == "live")
            .expect("a member with no completion signal stays while its lead works");
        assert_eq!(live.parent_id.as_deref(), Some("lead"));
        let lead = state.agents.iter().find(|a| a.id == "lead").unwrap();
        assert_eq!(lead.child_count, 1, "only the un-finished member remains a child");
    }

    // ── the third state: AWAITING (stopped on the operator) ──────────────────────
    //
    // End to end, because the value of this state is that it survives the WHOLE
    // pipeline: registry read, status verdict, agent build, wire serialization. A unit
    // test of the detector alone would still let a plumbing gap ship a globe that never
    // turns red.

    /// Seed a session whose transcript ENDS on the given events, after one operator
    /// prompt. Timestamps increase so the store's ordering matches the write order.
    fn seed_tail(store: &Store, id: &str, external: &str, harness: Harness, tail: Vec<Event>) {
        let now = Utc::now();
        let source_path = PathBuf::from(format!("/tmp/{id}.jsonl"));
        let session = Session {
            id: id.into(),
            harness,
            external_id: external.into(),
            project: Some(ProjectRef {
                cwd: PathBuf::from("/work"),
                repo_root: None,
                git_branch: None,
            }),
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: source_path.clone(),
            raw_hash: 0,
            ingested_at: now,
            meta: serde_json::json!({}),
        };
        let tid = format!("{id}-t0");
        let mut events = vec![Event::UserPrompt {
            text: "do the thing".into(),
            attachments: vec![],
            is_meta: false,
        }];
        events.extend(tail);
        let records: Vec<EventRecord> = events
            .into_iter()
            .enumerate()
            .map(|(i, event)| EventRecord {
                id: format!("{id}-e{i}"),
                turn_id: tid.clone(),
                session_id: id.into(),
                ts: now + chrono::Duration::milliseconds(i as i64 * 10),
                event,
                raw_ref: RawRef {
                    source_path: source_path.clone(),
                    offset: i as u64,
                    line: i as u32 + 1,
                },
            })
            .collect();
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
            .upsert_session_batch(&session, &[turn], &records, 0)
            .unwrap();
    }

    fn done_text(t: &str) -> Event {
        Event::AssistantText {
            text: t.into(),
            turn_complete: Some(true),
        }
    }

    /// A registry with an explicit `status` and `waitingFor`, the pair newer Claude
    /// writes while a prompt is on screen.
    fn claude_registry_waiting(pid: u32, sid: &str, waiting_for: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{pid}.json")),
            serde_json::json!({
                "pid": pid, "sessionId": sid, "cwd": "/work",
                "status": "waiting", "waitingFor": waiting_for,
            })
            .to_string(),
        )
        .unwrap();
        dir
    }

    /// The harness's own verdict is the strongest signal there is: `status: "waiting"`
    /// used to be swept into Idle by the `busy ⇒ Working, else Idle` rule, so a blocked
    /// agent looked exactly like a finished one.
    #[test]
    fn registry_waiting_becomes_awaiting_with_its_reason() {
        let store = Store::memory().unwrap();
        seed_tail(
            &store,
            "root",
            "sid-wait",
            Harness::ClaudeCode,
            vec![done_text("Ready when you are.")],
        );
        let reg = claude_registry_waiting(11, "sid-wait", "permission prompt");
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
        let a = state.agents.iter().find(|a| a.id == "root").unwrap();
        assert_eq!(a.status, "awaiting");
        assert_eq!(
            a.awaiting_reason.as_deref(),
            Some("approval"),
            "a permission prompt wants a yes/no, not an answer"
        );
    }

    /// The case the harness cannot see: the agent asked in plain prose and closed its
    /// turn normally, so the registry honestly reports `idle` and only the transcript
    /// knows better.
    #[test]
    fn a_question_in_the_output_becomes_awaiting_even_when_the_registry_says_idle() {
        let store = Store::memory().unwrap();
        seed_tail(
            &store,
            "asked",
            "sid-asked",
            Harness::ClaudeCode,
            vec![done_text(
                "I can go two ways here.\n\nWhich do you want?\n\n1. Patch it\n2. Rewrite it",
            )],
        );
        seed_tail(
            &store,
            "reported",
            "sid-reported",
            Harness::ClaudeCode,
            vec![done_text("Done. 214 tests pass, nothing left to decide.")],
        );
        let reg = claude_registry_status(&[(21, "sid-asked", "idle"), (22, "sid-reported", "idle")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
        let by = |id: &str| state.agents.iter().find(|a| a.id == id).unwrap();
        assert_eq!(by("asked").status, "awaiting");
        assert_eq!(by("asked").awaiting_reason.as_deref(), Some("question"));
        assert_eq!(
            by("reported").status,
            "idle",
            "a finished report is idle; only a question is awaiting"
        );
        assert_eq!(by("reported").awaiting_reason, None);
    }

    fn seed_open_question_tool(store: &Store, id: &str, external: &str) {
        seed_tail(
            store,
            id,
            external,
            Harness::ClaudeCode,
            vec![
                Event::AssistantText {
                    text: "Let me check with you.".into(),
                    turn_complete: Some(false),
                },
                Event::ToolCall {
                    tool: "AskUserQuestion".into(),
                    input: serde_json::json!({
                        "questions": [{ "question": "Ship the quick fix or rewrite it?" }]
                    }),
                    call_id: "c1".into(),
                    kind: ToolKind::Builtin,
                },
            ],
        );
    }

    /// An unresolved `AskUserQuestion` looks exactly like a slow tool from the outside
    /// (newest event, no result), which is why it has to OUTRANK the Working verdict the
    /// in-flight-call rule would otherwise produce, rather than being promoted from Idle.
    /// This is the path for a Claude build whose registry entry carries no `status` at
    /// all, which is most of the live sessions on this machine. The panel readout is the
    /// question itself, not the tool name.
    #[test]
    fn an_open_question_tool_outranks_working_and_shows_what_it_asked() {
        let store = Store::memory().unwrap();
        seed_open_question_tool(&store, "prompting", "sid-prompt");
        let reg = claude_registry(&[(31, "sid-prompt")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
        let a = state.agents.iter().find(|a| a.id == "prompting").unwrap();
        assert_eq!(
            a.status, "awaiting",
            "a question on screen is not the same as a tool running"
        );
        assert_eq!(a.awaiting_reason.as_deref(), Some("question"));
        let action = a
            .current_action
            .as_ref()
            .expect("the open prompt is the action");
        assert_eq!(action.kind, "ask");
        assert_eq!(action.label, "Ship the quick fix or rewrite it?");
    }

    /// The precedence rule, stated as a test because it is a judgement call and not an
    /// obvious one: a registry that says `busy` WINS over the same dangling question
    /// tool. Newer Claude reports an open `AskUserQuestion` as `waiting` itself, so a
    /// `busy` here means the harness believes it is generating and the transcript tail is
    /// simply behind. Believing the live registry keeps a false red off the board, and a
    /// false red is the one failure that teaches the operator to ignore the colour.
    #[test]
    fn a_busy_registry_outranks_a_dangling_question_tool() {
        let store = Store::memory().unwrap();
        seed_open_question_tool(&store, "prompting", "sid-prompt");
        let reg = claude_registry_status(&[(31, "sid-prompt", "busy")]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
        let a = state.agents.iter().find(|a| a.id == "prompting").unwrap();
        assert_eq!(a.status, "working");
        assert_eq!(a.awaiting_reason, None);
    }

    /// Codex writes no approval or dialog record of any kind, so the trailing question is
    /// its ONLY awaiting signal. It also reports no stop reason, which is exactly the
    /// `turn_complete: None` path.
    #[test]
    fn codex_reaches_awaiting_through_its_trailing_question() {
        let store = Store::memory().unwrap();
        seed_tail(
            &store,
            "cx",
            "sid-cx",
            Harness::Codex,
            vec![Event::AssistantText {
                text: "Both migrations apply cleanly. Do you want me to run them now?".into(),
                turn_complete: None,
            }],
        );
        let reg = claude_registry(&[]);
        let state = assemble(&store, reg.path(), &|_| true, &codex_all_open, &ProcessIndex::unscanned(), Utc::now());
        let a = state.agents.iter().find(|a| a.id == "cx").unwrap();
        assert_eq!(a.status, "awaiting");
        assert_eq!(a.awaiting_reason.as_deref(), Some("question"));
    }
}
