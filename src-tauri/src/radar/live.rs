//! Live transcript sources: pull current on-disk transcript tails into the store and
//! scan the Claude/Codex roots for the set of currently-OPEN sessions, then hand the
//! resolved liveness off to the pure [`super::assemble::assemble`] join.

use super::assemble::assemble;
use super::liveness::{self, read_claude_registry};
use crate::ir::Session;
use crate::store::Store;
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;

/// Pull current live transcript tails into the store. This is intentionally
/// explicit: startup/cold-read paths can close the "agent was already running before
/// WARDEN" gap, while steady heartbeat recomputes stay read-only.
pub fn refresh_live_context(store: &Store, sessions_dir: &Path) -> usize {
    let claude_projects_dir = crate::util::default_claude_projects();
    let claude_events = refresh_live_claude_transcripts(store, &claude_projects_dir, sessions_dir);
    let codex_sessions_dir = crate::util::default_codex_sessions();
    let codex_archived_dir = crate::util::default_codex_archived_sessions();
    let codex_events = refresh_live_codex_rollouts(store, &codex_sessions_dir, &codex_archived_dir);
    claude_events + codex_events
}

/// Recompute the forest and return it. The scheduler's watcher calls this on each
/// relevant FS/liveness event; `lib.rs` then emits it as `radar_state`. Uses the real
/// `pid_alive` syscall and the current clock.
///
/// Linkage is derived when transcript bytes are ingested (startup backfill, explicit
/// live refresh, or live tail watcher). Keeping this steady-state recompute read-only
/// avoids re-reading/hashing live transcript files on every heartbeat while preserving
/// live nesting when new data actually arrives.
pub fn recompute_radar_state(store: &Store, sessions_dir: &Path) -> super::RadarState {
    // The Codex live set is the set of rollout uuids whose file currently sits under
    // `~/.codex/sessions/` (and NOT under `~/.codex/archived_sessions/`). We scan the
    // two roots ONCE here, then close over the resulting set so `assemble` stays a
    // pure join (no per-session FS walk inside the tested path). `source_path` in the
    // store can be stale after Codex moves a rollout to the archive, so membership is
    // decided by the CURRENT on-disk location, never by the stored path.
    let codex_sessions_dir = crate::util::default_codex_sessions();
    let codex_archived_dir = crate::util::default_codex_archived_sessions();
    let live_codex = live_codex_rollout_ids(&codex_sessions_dir, &codex_archived_dir);
    let is_codex_open = |s: &Session| live_codex.contains(s.external_id.as_str());
    assemble(
        store,
        sessions_dir,
        &liveness::pid_alive,
        &is_codex_open,
        Utc::now(),
    )
}

/// Pull current live Claude transcript tails into the store before RADAR assembles
/// the forest. The liveness registry can say "this PID/session is open" while the
/// store still holds an old tail (or no row at all) because the session predates
/// WARDEN startup and no fresh watcher event fired. Reuse the scheduler's
/// byte-watermark ingester so unchanged transcripts are cheap, while appended root
/// and subagent bytes become live context/log rows immediately.
fn refresh_live_claude_transcripts(
    store: &Store,
    projects_dir: &Path,
    sessions_dir: &Path,
) -> usize {
    let paths = live_claude_transcript_paths(projects_dir, sessions_dir);
    if paths.is_empty() {
        return 0;
    }
    let registry = crate::ingest::AdapterRegistry::from_adapters(vec![Box::new(
        crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            projects_dir.to_path_buf(),
            store.clone(),
        ),
    )]);
    let mut events = 0usize;
    for path in paths {
        match crate::scheduler::ingest_file_once(&registry, store, &path) {
            Ok(n) => events += n,
            Err(e) => tracing::warn!(
                path=%path.display(),
                error=%format!("{e:#}"),
                "live Claude radar refresh failed"
            ),
        }
    }
    if events > 0 {
        let _ = crate::ingest::claude_code::link_claude_subagents_in_store(store);
    }
    events
}

/// Pull current live Codex rollout tails into the store before RADAR assembles the
/// forest. This closes the startup gap: a rollout that was already running before
/// WARDEN launched has a file on disk, but no watcher event may fire after startup,
/// so the live-id scan alone can find the agent while the store still has stale or
/// missing context. Reuse the scheduler's byte-watermark ingester so unchanged files
/// are cheap and appended bytes become live activity rows.
fn refresh_live_codex_rollouts(store: &Store, sessions_dir: &Path, archived_dir: &Path) -> usize {
    let paths = live_codex_rollout_paths(sessions_dir, archived_dir);
    if paths.is_empty() {
        return 0;
    }
    let registry = crate::ingest::AdapterRegistry::from_adapters(vec![Box::new(
        crate::ingest::codex::CodexAdapter::with_root(
            sessions_dir.to_path_buf(),
            archived_dir.to_path_buf(),
            store.clone(),
        ),
    )]);
    let mut events = 0usize;
    for path in paths {
        match crate::scheduler::ingest_file_once(&registry, store, &path) {
            Ok(n) => events += n,
            Err(e) => tracing::warn!(
                path=%path.display(),
                error=%format!("{e:#}"),
                "live Codex radar refresh failed"
            ),
        }
    }
    if events > 0 {
        let _ = crate::ingest::codex::link_codex_subagents_in_store(store);
    }
    events
}

/// Resolve live Claude registry entries to their current transcript files. Root
/// transcripts live at `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`; Claude
/// subagent transcripts for that live root live below
/// `<encoded-cwd>/<sessionId>/subagents/**.jsonl`. Missing files are skipped: the
/// registry is the liveness source, but the transcript is the renderable context.
fn live_claude_transcript_paths(
    projects_dir: &Path,
    sessions_dir: &Path,
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (pid, v) in read_claude_registry(sessions_dir) {
        if !liveness::pid_alive(pid) {
            continue;
        }
        let Some(session_id) = v.get("sessionId").and_then(|s| s.as_str()) else {
            continue;
        };
        let Some(cwd) = v.get("cwd").and_then(|s| s.as_str()) else {
            continue;
        };
        let root = projects_dir
            .join(claude_project_dir_name(cwd))
            .join(format!("{session_id}.jsonl"));
        if !root.is_file() {
            continue;
        }
        if seen.insert(root.clone()) {
            out.push(root.clone());
        }

        let subagents = root
            .parent()
            .map(|p| p.join(session_id).join("subagents"))
            .filter(|p| p.exists());
        let Some(subagents) = subagents else {
            continue;
        };
        for entry in walkdir::WalkDir::new(subagents)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if !entry.file_type().is_file() || p.extension().map(|x| x != "jsonl").unwrap_or(true) {
                continue;
            }
            let p = entry.into_path();
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_whitespace() {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Whether a non-archived Codex rollout is recent enough to still count as "present"
/// on the radar (hybrid stale policy). Codex has no process/termination signal, so a
/// rollout the user never archived would linger forever; we hide one idle longer than
/// `stale_secs` (`WARDEN_RADAR_CODEX_STALE_HRS`, default 6h). `stale_secs == 0`
/// disables the cutoff; an unknown mtime is KEPT (never drop on a missing stat).
fn codex_fresh(mtime_secs_ago: Option<u64>, stale_secs: u64) -> bool {
    if stale_secs == 0 {
        return true; // cutoff disabled
    }
    match mtime_secs_ago {
        Some(secs) => secs <= stale_secs,
        None => true, // unknown mtime → keep (never drop on a missing stat)
    }
}

/// Scan the two Codex roots and return the set of rollout UUIDs that are currently
/// OPEN — i.e. present under `sessions_dir` and absent from `archived_dir`. The
/// archive move is Codex's "done" signal (spec §4.3), so an id in the archive is
/// closed even if a stale `sessions/` copy lingers. Thin FS wrapper, kept OUT of the
/// unit-tested path (`assemble` receives the resolved set as a closure). A missing
/// dir contributes nothing (yields an empty contribution, not an error).
fn live_codex_rollout_ids(
    sessions_dir: &Path,
    archived_dir: &Path,
) -> std::collections::HashSet<String> {
    live_codex_rollout_paths(sessions_dir, archived_dir)
        .into_iter()
        .map(|p| crate::ingest::codex::external_id_from_filename(&p))
        .collect()
}

/// Scan Codex roots and return currently-open live rollout paths: present under
/// `sessions_dir`, absent from `archived_dir`, and still fresh under the hybrid
/// stale policy.
fn live_codex_rollout_paths(sessions_dir: &Path, archived_dir: &Path) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;
    let is_rollout = |entry: &walkdir::DirEntry| -> bool {
        let p = entry.path();
        entry.file_type().is_file()
            && p.extension().map(|x| x == "jsonl").unwrap_or(false)
            && p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("rollout-"))
                .unwrap_or(false)
    };
    // Archived rollouts are closed regardless of age — collect their ids to exclude.
    let mut archived = HashSet::new();
    for entry in walkdir::WalkDir::new(archived_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if is_rollout(&entry) {
            archived.insert(crate::ingest::codex::external_id_from_filename(
                entry.path(),
            ));
        }
    }
    // Live rollouts: under sessions/, not archived, and not stale. The freshness
    // cutoff (hybrid policy) drops a rollout the user abandoned without archiving —
    // Codex has no PID/termination signal, so mtime age is the only "still active" cue.
    let stale_secs = crate::util::radar_codex_stale_secs();
    let now = std::time::SystemTime::now();
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !is_rollout(&entry) {
            continue;
        }
        let id = crate::ingest::codex::external_id_from_filename(entry.path());
        if archived.contains(&id) {
            continue;
        }
        let mtime_secs_ago = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mt| now.duration_since(mt).ok())
            .map(|d| d.as_secs());
        if codex_fresh(mtime_secs_ago, stale_secs) {
            out.push(entry.into_path());
        }
    }
    out
}

#[cfg(test)]
mod codex_stale_tests {
    use super::codex_fresh;

    #[test]
    fn keeps_recent_drops_stale_handles_unknown_and_disabled() {
        assert!(codex_fresh(Some(60), 21_600), "1m ago within 6h → keep");
        assert!(
            !codex_fresh(Some(7 * 3600), 21_600),
            "7h ago past 6h → drop"
        );
        assert!(
            codex_fresh(Some(21_600), 21_600),
            "exactly at cutoff → keep (<=)"
        );
        assert!(
            codex_fresh(None, 21_600),
            "unknown mtime → keep (never drop on a missing stat)"
        );
        assert!(
            codex_fresh(Some(999_999), 0),
            "cutoff disabled (0) → keep all"
        );
    }
}
