//! Live-ingest scheduler: byte-offset watermark resume + FSEvents tailing.
//!
//! Two layers, deliberately separated so the logic is unit-testable without a
//! running event loop:
//!
//! * [`ingest_file_once`] — the **testable core**. Given a single `*.jsonl`
//!   path it figures out which adapter owns it, consults the saved byte
//!   watermark, reads only the new tail (or the whole file on first sight / after
//!   a rewrite), maps it to IR via [`Adapter::parse_range`], and persists the
//!   batch advancing the watermark to the new EOF. No watcher, no `AppHandle`.
//! * [`spawn_watchers`] — the **notify glue**. One `RecommendedWatcher`
//!   (FSEvents on macOS) per adapter root; on a debounced `*.jsonl`
//!   create/modify it calls [`ingest_file_once`] and emits `ingest_progress`.
//!
//! Watermarks are byte offsets, not record counts: FSEvents coalesces rapid
//! writes, so on every event we seek to the saved offset and read to EOF rather
//! than trusting how many events the OS reported (see CLAUDE.md “Watermarks are
//! byte-offset”).

use crate::ingest::AdapterRegistry;
use crate::ir::Harness;
use crate::store::Store;
use crate::util::hash64;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::radar::RadarDirtySignal;

/// Debounce window for coalescing FSEvents bursts on a single file.
/// Override with `WARDEN_WATCH_DEBOUNCE_MS` (mirrors the `util.rs` env pattern).
fn debounce() -> Duration {
    let ms = std::env::var("WARDEN_WATCH_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(250);
    Duration::from_millis(ms)
}

#[derive(Clone, Copy)]
struct LivePathSeen {
    at: Instant,
    len: Option<u64>,
}

fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn should_handle_live_path(
    path: &Path,
    last: &mut HashMap<PathBuf, LivePathSeen>,
    now: Instant,
    debounce: Duration,
) -> bool {
    let len = file_len(path);
    if let Some(prev) = last.get(path) {
        if now.duration_since(prev.at) < debounce && prev.len == len {
            return false;
        }
    }
    last.insert(path.to_path_buf(), LivePathSeen { at: now, len });
    true
}

/// True when `path` lives under (or is) one of `root`'s subtrees.
fn path_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn complete_jsonl_prefix(slice: &[u8], start_offset: u64) -> (&[u8], u64) {
    if slice.is_empty() {
        return (slice, start_offset);
    }
    if slice.ends_with(b"\n") {
        return (slice, start_offset + slice.len() as u64);
    }
    match slice.iter().rposition(|b| *b == b'\n') {
        Some(idx) => (&slice[..=idx], start_offset + idx as u64 + 1),
        None => (&[], start_offset),
    }
}

/// What one live-ingest pass actually did.
///
/// `activity` is the number the watcher has always reported. The other two are split
/// out because they mean very different things to the radar: an append is a globe
/// growing, and only a globe ARRIVING or LEAVING is allowed to skip the recompute rate
/// floor (see `RadarDirtySignal::mark_dirty_urgent`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveIngest {
    /// Parsed events plus newly persisted sessions that have no events yet.
    pub activity: usize,
    /// Sessions persisted for the first time by this pass. A globe ARRIVING.
    pub new_sessions: usize,
    /// Events that can flip a globe's liveness verdict this instant. A globe LEAVING.
    ///
    /// This half was missing, and its absence is what "subagent tracking is slow"
    /// actually was. Only `new_sessions` took the urgent path, so a subagent SPAWNING
    /// jumped the rate floor while the same subagent FINISHING sat behind up to a full
    /// second of throttle built for token counts, or up to the 2s heartbeat when the
    /// worker was busy. The module doc has always claimed both halves were urgent.
    ///
    /// A root has a PID and a live registry entry whose status is authoritative, and
    /// the registry file is watched separately (a `Create`/`Remove` there is already
    /// structural). A SUBAGENT or an in-process TEAMMATE has neither, so a completed
    /// turn on its own transcript is the only signal it ever finishes by. That makes
    /// this counter the one thing standing between a teammate going quiet and the
    /// board showing it.
    ///
    /// Rare by construction, which is what keeps the exemption safe: a turn writes
    /// dozens of events and exactly one of them carries `stop_reason: end_turn`, and a
    /// turn lasts seconds. See `is_lifecycle_event`.
    pub lifecycle: usize,
}

/// Does this event change a globe's LIVENESS the moment it lands?
///
/// Deliberately narrow. Every event is activity; almost none of it is lifecycle, and
/// the rate floor only stays effective while that is true.
///
/// * `AssistantText { turn_complete: Some(true) }` is a turn genuinely ending. It is
///   the ONLY judge a subagent or teammate has (no PID, no registry entry), so it is
///   the whole point of this function. `Some(false)` is a mid-turn preamble with a
///   tool call coming, and 79% of text-only assistant lines are that; `None` carries
///   no information at all. Neither is urgent.
/// * `SubagentSpawn` is a globe arriving named by an EXISTING session's transcript,
///   which `new_sessions` misses whenever the child's own file has not appeared yet.
fn is_lifecycle_event(event: &crate::ir::Event) -> bool {
    use crate::ir::Event;
    matches!(
        event,
        Event::AssistantText {
            turn_complete: Some(true),
            ..
        } | Event::SubagentSpawn { .. }
    )
}

/// Ingest the new bytes of a single transcript file, advancing its watermark.
///
/// Returns the amount of live-ingest activity (0 when nothing new): parsed events
/// plus newly persisted sessions that do not have events yet. This is the testable
/// core invoked by the watcher and reusable for an on-ask trigger.
///
/// Algorithm (brief §Task 4):
/// * `len` = current file length; `off` = saved byte watermark (0 if unseen).
/// * `len == off` AND the full-file hash is unchanged → nothing new, skip.
/// * `len  < off` (file rewritten/truncated) → reset `off = 0`, full reparse.
/// * otherwise read `bytes[off..]` and `parse_range(path, slice, off, hash)`.
/// * persist each batch with `watermark_offset = len` (the new EOF).
pub fn ingest_file_once(registry: &AdapterRegistry, store: &Store, path: &Path) -> Result<usize> {
    Ok(ingest_file_once_reporting(registry, store, path)?.activity)
}

/// [`ingest_file_once`], with the new-session count kept rather than folded away.
pub fn ingest_file_once_reporting(
    registry: &AdapterRegistry,
    store: &Store,
    path: &Path,
) -> Result<LiveIngest> {
    // Only transcript files are ingestible.
    if path.extension().map(|x| x != "jsonl").unwrap_or(true) {
        return Ok(LiveIngest::default());
    }

    // Find the adapter that owns this path (its root is an ancestor).
    let adapter = registry
        .adapters()
        .iter()
        .find(|a| a.roots().iter().any(|r| path_under(path, r)));
    let adapter = match adapter {
        Some(a) => a,
        None => return Ok(LiveIngest::default()), // not under any watched root, ignore
    };

    // STAT BEFORE READ. A file may vanish between the FSEvent and our look; treat as
    // nothing to do. This used to open with `fs::read` + SHA-256 of the WHOLE file, on
    // every event, for every live transcript, purely to decide whether anything had
    // changed — which is why `refresh_live_context` measured 3.1s on a real corpus while
    // ingesting 508 events. Length and mtime answer the same question from one `stat`.
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(LiveIngest::default());
    };
    let len = meta.len();
    if len == 0 {
        return Ok(LiveIngest::default());
    }
    let mtime_ns = crate::util::mtime_nanos(&meta);
    let mark = store.source_mark(path)?;

    // Nothing has touched the file since we last read it. `0` is the reserved "unknown"
    // mtime (an older schema row, or a platform that will not say) and never matches, so
    // an unknown mark falls through to the read path exactly as before.
    if mtime_ns != 0 && mark.mtime_ns == mtime_ns && mark.size == len {
        return Ok(LiveIngest::default());
    }

    let mut off = mark.offset;
    if len < off {
        // Truncated or rewritten shorter than the watermark → start over.
        off = 0;
    }

    // READ ONLY WHAT WE HAVE NOT PARSED. An append is the overwhelmingly common case and
    // it needs the tail alone; the whole file is read only on first sight, after a
    // truncation, or to settle a same-length rewrite. `bytes` is always `file[off..]`,
    // so the parse below is unchanged either way.
    let (bytes, hash) = if off > 0 && off < len {
        let tail = match read_from(path, off) {
            Ok(b) => b,
            Err(_) => return Ok(LiveIngest::default()),
        };
        let hash = hash64(&tail);
        (tail, hash)
    } else {
        let all = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return Ok(LiveIngest::default()),
        };
        let hash = hash64(&all);
        if off == len {
            // No growth, but the stat moved: either a touch that left the bytes alone or
            // a rewrite in place at the same length. The content hash is the only thing
            // that separates them, and this is now the ONLY path that pays for it.
            if store.source_raw_hash(path)?.map(|h| h == hash) == Some(true) {
                store.set_source_stat(path, len, mtime_ns)?;
                return Ok(LiveIngest::default());
            }
            off = 0;
        }
        (all, hash)
    };

    let (slice, watermark_offset) = complete_jsonl_prefix(&bytes, off);
    if slice.is_empty() {
        // A half-written line and nothing else. Stamp the stat anyway or the next event
        // re-reads these same bytes to reach the same conclusion.
        store.set_source_stat(path, len, mtime_ns)?;
        return Ok(LiveIngest::default());
    }
    let batches = adapter
        .parse_range(path, slice, off, hash)
        .with_context(|| format!("parse_range {} @ {off}", path.display()))?;

    let mut report = LiveIngest::default();
    for b in &batches {
        report.activity += b.events.len();
        report.lifecycle += b
            .events
            .iter()
            .filter(|e| is_lifecycle_event(&e.event))
            .count();
        // A primary-key probe per batch, not a scan of every session on the machine.
        // A pass touches one or two sessions; the old `sessions()` + `HashSet` built
        // the whole table on every FSEvent, so the cost of noticing a new subagent
        // grew with how long WARDEN had been running. See `Store::session_exists`.
        if !store.session_exists(&b.session.id)? {
            report.activity += 1;
            report.new_sessions += 1;
        }
        // Persist through the last complete JSONL record. If a watcher fired while
        // a line was half-written, leave those bytes unwatermarked for retry.
        store.upsert_session_batch(&b.session, &b.turns, &b.events, watermark_offset)?;
    }
    // The size/mtime the SKIP above compares against, stamped from the stat taken BEFORE
    // the read: if the file grew while we were reading it, storing the smaller length
    // means the next event looks again, which is the safe direction to be wrong in.
    store.set_source_stat(path, len, mtime_ns)?;
    Ok(report)
}

/// Read a file from `offset` to EOF.
///
/// The append path's whole point: a live transcript is mostly bytes we have already
/// parsed, and re-reading them is the single most expensive thing the live ingest did.
fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Owns the live `RecommendedWatcher`s so they outlive `setup()`. A bare
/// `Vec<RecommendedWatcher>` is `Send` but not `Sync`, so it can't go into
/// Tauri-managed state directly; the `Mutex` makes the holder `Sync`. Dropping
/// this stops all watchers.
pub struct WatcherGuard {
    _watchers: std::sync::Mutex<Vec<notify::RecommendedWatcher>>,
}

impl WatcherGuard {
    pub fn new(watchers: Vec<notify::RecommendedWatcher>) -> Self {
        Self {
            _watchers: std::sync::Mutex::new(watchers),
        }
    }
}

/// Spawn one filesystem watcher per adapter root. On a debounced `*.jsonl`
/// create/modify, [`ingest_file_once`] runs and an `ingest_progress` event is
/// emitted (`{harness, path, activity, phase:"live"}`; `events` is retained as a
/// compatibility alias).
///
/// The returned watchers must be kept alive for the lifetime of the app (drop =
/// stop watching), so callers store them in long-lived state (see
/// [`WatcherGuard`]).
pub fn spawn_watchers(
    registry: Arc<AdapterRegistry>,
    store: Store,
    app: tauri::AppHandle,
    radar_signal: Option<RadarDirtySignal>,
) -> Result<Vec<notify::RecommendedWatcher>> {
    use notify::{EventKind, RecursiveMode, Watcher};
    use tauri::Emitter;

    // Unique roots across all adapters (an adapter may expose several).
    let mut roots: Vec<PathBuf> = Vec::new();
    for a in registry.adapters() {
        for r in a.roots() {
            if !roots.contains(&r) {
                roots.push(r);
            }
        }
    }

    let mut watchers = Vec::new();
    for root in roots {
        // FSEvents fires on a parent even if `root` does not yet exist; only watch
        // roots that are present to avoid a watcher error on a fresh machine.
        if !root.exists() {
            tracing::info!(root=%root.display(), "watch root absent; skipping (backfill will create on first session)");
            continue;
        }
        let registry = registry.clone();
        let store = store.clone();
        let app = app.clone();
        let radar_signal = radar_signal.clone();
        let debounce = debounce();
        // Per-file last-handled timestamp + length for debouncing coalesced bursts.
        let mut last: HashMap<PathBuf, LivePathSeen> = HashMap::new();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error=?e, "fs watch error");
                    return;
                }
            };
            // Only creations and content modifications matter.
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                return;
            }
            for path in event.paths {
                if path.extension().map(|x| x != "jsonl").unwrap_or(true) {
                    continue;
                }
                // Debounce duplicate same-size bursts only. If the file grew inside
                // the debounce window (common create-empty -> append startup path),
                // ingest immediately so a new subagent appears on the first bytes.
                if !should_handle_live_path(&path, &mut last, Instant::now(), debounce) {
                    continue;
                }

                match ingest_file_once_reporting(&registry, &store, &path) {
                    Ok(report) if report.activity > 0 => {
                        relink_after_live_ingest(&registry, &store, &path);
                        kick_radar_after_live_ingest(report, radar_signal.as_ref());
                        let harness = harness_for_live_path(&registry, &path)
                            .map(|h| h.as_str().to_string())
                            .unwrap_or_default();
                        let _ = app.emit(
                            "ingest_progress",
                            serde_json::json!({
                                "harness": harness,
                                "path": path.to_string_lossy(),
                                "activity": report.activity,
                                "events": report.activity,
                                "phase": "live",
                            }),
                        );
                    }
                    Ok(_) => {} // nothing new
                    Err(e) => {
                        tracing::warn!(path=%path.display(), error=?e, "live ingest failed")
                    }
                }
            }
        })
        .context("create fs watcher")?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", root.display()))?;
        tracing::info!(root=%root.display(), "watching for live transcripts");
        watchers.push(watcher);
    }
    Ok(watchers)
}

/// Wake the radar after a live ingest.
///
/// A pass that persisted a NEW session is a globe arriving (a subagent spawning is
/// exactly this) and a pass that ingested a completed turn is a globe leaving (the
/// only signal a subagent or teammate has). Both take the urgent path and skip the
/// recompute rate floor. A pass that only grew a transcript is a globe growing, and
/// waits its turn like every other append: a token count that lands a second late is
/// invisible, and that is what the floor was built for.
fn kick_radar_after_live_ingest(report: LiveIngest, radar_signal: Option<&RadarDirtySignal>) {
    if report.activity == 0 {
        return;
    }
    if let Some(signal) = radar_signal {
        if report.new_sessions > 0 || report.lifecycle > 0 {
            signal.mark_dirty_urgent();
        } else {
            signal.mark_dirty();
        }
    }
}

fn harness_for_live_path(registry: &AdapterRegistry, path: &Path) -> Option<Harness> {
    registry
        .adapters()
        .iter()
        .find(|a| a.roots().iter().any(|r| path.starts_with(r)))
        .map(|a| a.harness())
}

fn relink_after_live_ingest(registry: &AdapterRegistry, store: &Store, path: &Path) {
    match harness_for_live_path(registry, path) {
        Some(Harness::ClaudeCode) => {
            if let Err(e) = crate::ingest::claude_code::link_claude_subagents_in_store(store) {
                tracing::warn!(path=%path.display(), error=%format!("{e:#}"), "live Claude relink failed");
            }
        }
        Some(Harness::Codex) => {
            if let Err(e) = crate::ingest::codex::link_codex_subagents_in_store(store) {
                tracing::warn!(path=%path.display(), error=%format!("{e:#}"), "live Codex relink failed");
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::claude_code::ClaudeCodeAdapter;
    use crate::ingest::codex::CodexAdapter;
    use crate::ingest::AdapterRegistry;
    use crate::ir::*;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    #[test]
    fn live_ingest_with_new_events_marks_radar_dirty() {
        let signal = RadarDirtySignal::new();
        kick_radar_after_live_ingest(
            LiveIngest {
                activity: 3,
                new_sessions: 0,
                lifecycle: 0,
            },
            Some(&signal),
        );

        assert!(
            signal.inner.dirty.load(Ordering::SeqCst),
            "successful live ingest must immediately wake the radar recompute worker"
        );
        assert!(
            !signal.inner.urgent.load(Ordering::SeqCst),
            "a plain append must not skip the recompute rate floor, or the pinned-core bug is back"
        );
    }

    /// A subagent spawning is a NEW session, and it is the case the radar is judged
    /// on: it must not sit behind the one-second floor built for token appends.
    #[test]
    fn a_new_session_wakes_the_radar_urgently() {
        let signal = RadarDirtySignal::new();
        kick_radar_after_live_ingest(
            LiveIngest {
                activity: 1,
                new_sessions: 1,
                lifecycle: 0,
            },
            Some(&signal),
        );

        assert!(signal.inner.dirty.load(Ordering::SeqCst));
        assert!(
            signal.inner.urgent.load(Ordering::SeqCst),
            "a globe arriving must skip the rate floor"
        );
    }

    /// The other half of the subagent-latency fix, and the half that was missing.
    ///
    /// A subagent or an in-process TEAMMATE has no PID and no live registry entry, so
    /// a completed turn on its own transcript is the only signal it ever finishes by.
    /// Only ARRIVING used to be urgent, so a teammate going quiet sat behind the 1s
    /// append floor, or the 2s heartbeat when the worker was busy.
    #[test]
    fn a_completed_turn_wakes_the_radar_urgently() {
        let signal = RadarDirtySignal::new();
        kick_radar_after_live_ingest(
            LiveIngest {
                activity: 4,
                new_sessions: 0,
                lifecycle: 1,
            },
            Some(&signal),
        );

        assert!(signal.inner.dirty.load(Ordering::SeqCst));
        assert!(
            signal.inner.urgent.load(Ordering::SeqCst),
            "a teammate finishing must skip the rate floor, exactly as one spawning does"
        );
    }

    /// The guard rail. The floor only keeps capping CPU while lifecycle stays RARE, so
    /// the classifier has to stay narrow: a mid-turn preamble and an unknown stop
    /// reason are not a globe leaving, and 79% of text-only assistant lines are the
    /// first of those.
    #[test]
    fn only_a_genuinely_completed_turn_counts_as_lifecycle() {
        assert!(is_lifecycle_event(&Event::AssistantText {
            text: "Done.".into(),
            turn_complete: Some(true),
        }));
        assert!(is_lifecycle_event(&Event::SubagentSpawn {
            source_assistant_uuid: "u1".into(),
            child_session: None,
        }));

        // A preamble with a tool call coming: the turn CONTINUES.
        assert!(!is_lifecycle_event(&Event::AssistantText {
            text: "Let me check the config.".into(),
            turn_complete: Some(false),
        }));
        // No information (an old row, or Codex, which reports no stop reason). Keep
        // the old assumption rather than promoting a guess to urgent.
        assert!(!is_lifecycle_event(&Event::AssistantText {
            text: "Done.".into(),
            turn_complete: None,
        }));
        // The bulk of a transcript. If any of these counted, the rate floor would be
        // gone and the pinned-core bug would be back.
        assert!(!is_lifecycle_event(&Event::Thinking { tokens: 40 }));
        assert!(!is_lifecycle_event(&Event::ToolResult {
            call_id: "c1".into(),
            status: ToolStatus::Ok,
            bytes: 12,
            summary: None,
        }));
        assert!(!is_lifecycle_event(&Event::UserPrompt {
            text: "go".into(),
            attachments: vec![],
            is_meta: false,
        }));
    }

    #[test]
    fn ingest_reports_the_new_session_that_a_first_sighting_created() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);

        let first = ingest_file_once_reporting(&registry, &store, &p).unwrap();
        assert_eq!(first.new_sessions, 1, "first sighting persists the session");

        // An append to a session the store already has is growth, not arrival.
        std::fs::write(&p, format!("{body}{MSG2}")).unwrap();
        let second = ingest_file_once_reporting(&registry, &store, &p).unwrap();
        assert_eq!(second.activity, 1);
        assert_eq!(
            second.new_sessions, 0,
            "an append must not be reported as a new session"
        );
    }

    #[test]
    fn live_ingest_relink_links_claude_subagent_before_radar_recompute() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("proj");
        let session = root.join("019sess");
        let subs = session.join("subagents");
        std::fs::create_dir_all(&subs).unwrap();

        let parent_jsonl = session.join("019sess.jsonl");
        std::fs::write(&parent_jsonl, "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"019sess\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"sourceToolAssistantUuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_live\",\"name\":\"Task\",\"input\":{}}]}}\n").unwrap();

        let child_jsonl = subs.join("agent-c0ffee.jsonl");
        std::fs::write(&child_jsonl, "{\"type\":\"user\",\"uuid\":\"cu\",\"sessionId\":\"019sess-sub\",\"isSidechain\":true,\"agentId\":\"c0ffee\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"content\":\"work\"}}\n").unwrap();
        std::fs::write(
            subs.join("agent-c0ffee.meta.json"),
            r#"{"agentType":"Explore","description":"watch live ingest relink","toolUseId":"toolu_live"}"#,
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let registry = AdapterRegistry::from_adapters(vec![Box::new(
            ClaudeCodeAdapter::with_root(root, store.clone()),
        )]);
        ingest_file_once(&registry, &store, &parent_jsonl).unwrap();
        ingest_file_once(&registry, &store, &child_jsonl).unwrap();

        let child_sid = crate::util::stable_id(&[
            "claude_code",
            "agent-c0ffee",
            &child_jsonl.to_string_lossy(),
        ]);
        assert_eq!(store.parent_of(&child_sid).unwrap(), None);

        relink_after_live_ingest(&registry, &store, &child_jsonl);

        let parent_sid =
            crate::util::stable_id(&["claude_code", "019sess", &parent_jsonl.to_string_lossy()]);
        assert_eq!(
            store.parent_of(&child_sid).unwrap(),
            Some(parent_sid),
            "live ingest must resolve nesting before the cached radar state is refreshed"
        );
    }

    #[test]
    fn live_path_debounce_allows_growth_inside_window() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("live.jsonl");
        std::fs::write(&path, "").unwrap();

        let mut last = HashMap::new();
        let now = Instant::now();
        let debounce = Duration::from_millis(250);

        assert!(should_handle_live_path(&path, &mut last, now, debounce));
        assert!(
            !should_handle_live_path(&path, &mut last, now + Duration::from_millis(10), debounce),
            "same-size duplicate inside debounce should still coalesce"
        );

        std::fs::write(&path, "new transcript bytes\n").unwrap();
        assert!(
            should_handle_live_path(&path, &mut last, now + Duration::from_millis(20), debounce),
            "a transcript that grew inside the debounce window must be ingested immediately"
        );
    }

    /// Build a registry over a single Codex adapter rooted at `root` (its archived
    /// root points at the same dir, harmless for these tests).
    fn codex_registry(root: &Path, store: Store) -> AdapterRegistry {
        AdapterRegistry::for_test(vec![Box::new(CodexAdapter::with_root(
            root.to_path_buf(),
            root.to_path_buf(),
            store,
        ))])
    }

    /// A realistically-named rollout file whose filename encodes the session uuid
    /// that also appears in `session_meta.payload.id` — required so a tail parse
    /// (deriving the id from the filename) lands on the same session as backfill.
    fn write_rollout(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("rollout-2026-06-19T16-33-00-019ee0ba-8295-7ba0-9971-c5af95e77191.jsonl");
        std::fs::write(&p, body).unwrap();
        p
    }

    const META: &str = "{\"timestamp\":\"2026-06-19T16:33:45.869Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019ee0ba-8295-7ba0-9971-c5af95e77191\",\"cwd\":\"/work\",\"model_provider\":\"openai\"}}\n";
    const MSG1: &str = "{\"timestamp\":\"2026-06-19T16:34:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"first\"}}\n";
    const MSG2: &str = "{\"timestamp\":\"2026-06-19T16:35:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"second\"}}\n";

    /// `ingest_file_once` on a fresh file ingests everything and sets the watermark
    /// to the file length; a re-run on the unchanged file adds nothing (resume).
    #[test]
    fn ingest_file_once_seeds_then_resumes() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);
        let len = std::fs::metadata(&p).unwrap().len();

        let first = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(first >= 1, "first ingest must report activity, got {first}");
        let (sessions, events_after_first, _) = store.counts().unwrap();
        assert_eq!(sessions, 1, "one session ingested");
        assert!(events_after_first >= 1);
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            len,
            "watermark must equal the file length after the first ingest"
        );

        // Re-run on the byte-identical file → resume, nothing new.
        let again = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(
            again, 0,
            "unchanged file must report 0 activity (watermark resume)"
        );
        let (_, events_after_resume, _) = store.counts().unwrap();
        assert_eq!(
            events_after_resume, events_after_first,
            "event count unchanged after a resume run"
        );
    }

    #[test]
    fn ingest_file_once_treats_empty_new_file_as_not_ready() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());
        let path = dir.path().join("rollout-2026-06-25T00-00-00-empty.jsonl");
        std::fs::write(&path, "").unwrap();

        let added = ingest_file_once(&registry, &store, &path).unwrap();

        assert_eq!(
            added, 0,
            "empty live-created transcript is not parse-ready yet"
        );
        assert_eq!(store.watermark_offset(&path).unwrap(), 0);
    }

    #[test]
    fn ingest_file_once_reports_new_codex_session_even_without_events() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());
        let path = dir
            .path()
            .join("rollout-2026-06-25T00-00-00-019efd6c-8f60-7f42-8da1-3977122aa6be.jsonl");
        let body = "{\"timestamp\":\"2026-06-25T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019efd6c-8f60-7f42-8da1-3977122aa6be\",\"cwd\":\"/tmp/BornCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\",\"thread_source\":\"user\"}}\n";
        std::fs::write(&path, body).unwrap();

        let added = ingest_file_once(&registry, &store, &path).unwrap();

        assert_eq!(
            added, 1,
            "a new metadata-only Codex rollout must still count as live ingest activity"
        );
        let (sessions, events, _) = store.counts().unwrap();
        assert_eq!(
            sessions, 1,
            "the new Codex session is persisted immediately"
        );
        assert_eq!(events, 0, "session_meta alone does not fabricate events");
        assert_eq!(store.watermark_offset(&path).unwrap(), body.len() as u64);
    }

    /// Appending one line yields exactly one new event and advances the watermark;
    /// the appended event's RawRef.offset is the ABSOLUTE position of its line.
    #[test]
    fn ingest_file_once_tails_appended_line() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);
        let _ = ingest_file_once(&registry, &store, &p).unwrap();
        let (_, base_events, _) = store.counts().unwrap();
        let off_before = std::fs::metadata(&p).unwrap().len();

        // Append a second agent_message line.
        let full = format!("{body}{MSG2}");
        std::fs::write(&p, &full).unwrap();
        let new_len = std::fs::metadata(&p).unwrap().len();

        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "exactly one appended event");
        let (_, events_now, _) = store.counts().unwrap();
        assert_eq!(
            events_now,
            base_events + 1,
            "store gained exactly one event"
        );
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            new_len,
            "watermark advanced to the new EOF"
        );

        // The newest AssistantText event must point at the appended line's start.
        let sid = store.sessions().unwrap()[0].id.clone();
        let appended = store
            .session_events(&sid)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match &e.event {
                Event::AssistantText { text, .. } if text == "second" => Some(e.raw_ref.offset),
                _ => None,
            })
            .expect("appended 'second' event present");
        assert_eq!(
            appended, off_before,
            "appended event offset must be the absolute start of its line"
        );
    }

    #[test]
    fn ingest_file_once_keeps_incomplete_trailing_line_unwatermarked() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let p = write_rollout(dir.path(), META);
        let _ = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(store.watermark_offset(&p).unwrap(), META.len() as u64);

        let partial_second = MSG2.trim_end_matches('\n').split_at(MSG2.len() / 2).0;
        let complete_prefix = format!("{META}{MSG1}");
        std::fs::write(&p, format!("{complete_prefix}{partial_second}")).unwrap();

        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "only the complete appended line is ingestible");
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            complete_prefix.len() as u64,
            "watermark must stop before the incomplete trailing line so it can be retried"
        );

        std::fs::write(&p, format!("{META}{MSG1}{MSG2}")).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "the completed trailing line is ingested on retry");

        let sid = store.sessions().unwrap()[0].id.clone();
        assert!(
            store.session_events(&sid).unwrap().iter().any(
                |(_, e)| matches!(&e.event, Event::AssistantText { text, .. } if text == "second")
            ),
            "the line that was initially partial must not be lost"
        );
    }

    /// A shrink/rewrite below the watermark forces a full reparse from offset 0.
    #[test]
    fn ingest_file_once_reparses_on_shrink() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let long = format!("{META}{MSG1}{MSG2}");
        let p = write_rollout(dir.path(), &long);
        let _ = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(store.watermark_offset(&p).unwrap(), long.len() as u64);

        // Rewrite the file shorter (e.g. log rotation) — watermark now exceeds len.
        let short = format!("{META}{MSG1}");
        std::fs::write(&p, &short).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(
            added >= 1,
            "a shrink must trigger a full reparse (offset reset to 0)"
        );
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            short.len() as u64,
            "watermark reset to the new (shorter) EOF"
        );
    }

    /// CRITICAL no-clobber: a live tail parse (which lacks session_meta) must not
    /// overwrite the good `model_ids` / `started_at` written by the full backfill.
    ///
    /// Simulated end-to-end: full-backfill the file via the adapter+store, then
    /// append a line and run `ingest_file_once` (the live path). Re-read the
    /// session row and assert the original model_ids and started_at survived.
    #[test]
    fn live_tail_does_not_clobber_backfilled_session() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);

        // Full backfill (offset 0) — resolves model_ids=["openai"] and the true
        // started_at from session_meta. Use the adapter's parse_range directly so
        // the session row mirrors a real backfill.
        let full_hash = hash64(body.as_bytes());
        let backfilled = registry.adapters()[0]
            .parse_range(&p, body.as_bytes(), 0, full_hash)
            .unwrap();
        let original = &backfilled[0].session;
        let original_model_ids = original.model_ids.clone();
        let original_started_at = original.started_at;
        assert_eq!(
            original_model_ids,
            vec!["openai".to_string()],
            "precondition: backfill resolved model_ids from session_meta"
        );
        store
            .upsert_session_batch(
                &backfilled[0].session,
                &backfilled[0].turns,
                &backfilled[0].events,
                body.len() as u64,
            )
            .unwrap();

        // Now a LIVE append + tail ingest (offset>0, no session_meta in slice).
        let full = format!("{body}{MSG2}");
        std::fs::write(&p, &full).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "tail added the appended event");

        // Re-read the persisted session and assert nothing good was clobbered.
        let row = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == original.id)
            .expect("session row present");
        assert_eq!(
            row.model_ids, original_model_ids,
            "tail parse must NOT erase the backfilled model_ids"
        );
        assert_eq!(
            row.started_at, original_started_at,
            "tail parse must NOT push started_at later than the true session start"
        );
        // And the project survived too (tail has no cwd → null project).
        assert!(
            row.project.is_some(),
            "tail parse must NOT null out the backfilled project"
        );
    }

    /// Paths outside every adapter root, and non-jsonl files, are ignored (0).
    #[test]
    fn ingest_file_once_ignores_foreign_and_nonjsonl() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        // Non-jsonl inside the root.
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "hello").unwrap();
        assert_eq!(ingest_file_once(&registry, &store, &txt).unwrap(), 0);

        // jsonl OUTSIDE the root.
        let other = tempdir().unwrap();
        let foreign = write_rollout(other.path(), &format!("{META}{MSG1}"));
        assert_eq!(
            ingest_file_once(&registry, &store, &foreign).unwrap(),
            0,
            "a path under no watched root must be ignored"
        );
    }

    /// Claude path smoke-test: the Claude adapter participates in `ingest_file_once`
    /// (filename == sessionId so backfill and tail share a session id).
    #[test]
    fn ingest_file_once_handles_claude() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = AdapterRegistry::for_test(vec![Box::new(ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store.clone(),
        ))]);
        // Filename stem IS the sessionId, matching the real Claude layout.
        let p = dir.path().join("019ee0ba.jsonl");
        let l1 = "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"019ee0ba\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hi\"}}\n";
        std::fs::write(&p, l1).unwrap();
        let first = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(first >= 1);
        let len1 = std::fs::metadata(&p).unwrap().len();
        assert_eq!(store.watermark_offset(&p).unwrap(), len1);

        let l2 = "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"019ee0ba\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":\"tail\"}}\n";
        std::fs::write(&p, format!("{l1}{l2}")).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(added >= 1, "claude tail adds the appended assistant event");
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            std::fs::metadata(&p).unwrap().len()
        );
    }

}
