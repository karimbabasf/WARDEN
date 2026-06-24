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
use crate::store::Store;
use crate::util::hash64;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Debounce window for coalescing FSEvents bursts on a single file.
/// Override with `WARDEN_WATCH_DEBOUNCE_MS` (mirrors the `util.rs` env pattern).
fn debounce() -> Duration {
    let ms = std::env::var("WARDEN_WATCH_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(250);
    Duration::from_millis(ms)
}

/// True when `path` lives under (or is) one of `root`'s subtrees.
fn path_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Ingest the new bytes of a single transcript file, advancing its watermark.
///
/// Returns the number of events ingested (0 when nothing new). This is the
/// testable core invoked by the watcher and reusable for an on-ask trigger.
///
/// Algorithm (brief §Task 4):
/// * `len` = current file length; `off` = saved byte watermark (0 if unseen).
/// * `len == off` AND the full-file hash is unchanged → nothing new, skip.
/// * `len  < off` (file rewritten/truncated) → reset `off = 0`, full reparse.
/// * otherwise read `bytes[off..]` and `parse_range(path, slice, off, hash)`.
/// * persist each batch with `watermark_offset = len` (the new EOF).
pub fn ingest_file_once(
    registry: &AdapterRegistry,
    store: &Store,
    path: &Path,
) -> Result<usize> {
    // Only transcript files are ingestible.
    if path.extension().map(|x| x != "jsonl").unwrap_or(true) {
        return Ok(0);
    }

    // Find the adapter that owns this path (its root is an ancestor).
    let adapter = registry
        .adapters()
        .iter()
        .find(|a| a.roots().iter().any(|r| path_under(path, r)));
    let adapter = match adapter {
        Some(a) => a,
        None => return Ok(0), // not under any watched root — ignore
    };

    // A file may vanish between the FSEvent and our read; treat as nothing to do.
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Ok(0),
    };
    let len = bytes.len() as u64;
    let hash = hash64(&bytes);
    let mut off = store.watermark_offset(path)?;

    if len == off {
        // No growth. If the content hash also matches what we stored, there is
        // genuinely nothing new (a touch / metadata-only event). Otherwise the
        // file was rewritten in place at the same length → full reparse.
        let unchanged = store
            .source_raw_hash(path)?
            .map(|h| h == hash)
            .unwrap_or(false);
        if unchanged {
            return Ok(0);
        }
        off = 0;
    } else if len < off {
        // Truncated or rewritten shorter than the watermark → start over.
        off = 0;
    }

    let slice = &bytes[off as usize..];
    let batches = adapter
        .parse_range(path, slice, off, hash)
        .with_context(|| format!("parse_range {} @ {off}", path.display()))?;

    let mut events = 0usize;
    for b in &batches {
        events += b.events.len();
        // Persist with the absolute EOF as the new watermark.
        store.upsert_session_batch(&b.session, &b.turns, &b.events, len)?;
    }
    Ok(events)
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
/// emitted (`{harness, path, events, phase:"live"}`).
///
/// The returned watchers must be kept alive for the lifetime of the app (drop =
/// stop watching), so callers store them in long-lived state (see
/// [`WatcherGuard`]).
pub fn spawn_watchers(
    registry: Arc<AdapterRegistry>,
    store: Store,
    app: tauri::AppHandle,
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
        let debounce = debounce();
        // Per-file last-handled timestamp for debouncing coalesced bursts.
        let mut last: HashMap<PathBuf, Instant> = HashMap::new();

        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
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
                    // Debounce: skip if we handled this exact path within the window.
                    let now = Instant::now();
                    if let Some(prev) = last.get(&path) {
                        if now.duration_since(*prev) < debounce {
                            continue;
                        }
                    }
                    last.insert(path.clone(), now);

                    match ingest_file_once(&registry, &store, &path) {
                        Ok(events) if events > 0 => {
                            let harness = registry
                                .adapters()
                                .iter()
                                .find(|a| a.roots().iter().any(|r| path.starts_with(r)))
                                .map(|a| a.harness().as_str().to_string())
                                .unwrap_or_default();
                            let _ = app.emit(
                                "ingest_progress",
                                serde_json::json!({
                                    "harness": harness,
                                    "path": path.to_string_lossy(),
                                    "events": events,
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
            },
        )
        .context("create fs watcher")?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", root.display()))?;
        tracing::info!(root=%root.display(), "watching for live transcripts");
        watchers.push(watcher);
    }
    Ok(watchers)
}

/// RADAR (Task 9): recompute the live forest and emit it as `radar_state`. Thin
/// wrapper over [`crate::radar::recompute_radar_state`] + the Tauri emit, so the
/// watcher closure stays small.
pub fn recompute_and_emit_radar(
    store: &Store,
    sessions_dir: &std::path::Path,
    app: &tauri::AppHandle,
) {
    use tauri::Emitter;
    let state = crate::radar::recompute_radar_state(store, sessions_dir);
    let _ = app.emit("radar_state", &state);
}

/// A cheap, thread-safe "the live forest changed" signal shared between the FS
/// watchers (producers) and the single recompute worker (consumer).
///
/// `mark_dirty` is non-blocking and safe to call from a `notify` callback thread:
/// it sets a flag and wakes the worker. Many rapid calls collapse onto the one flag
/// — the burst is coalesced for free, so a storm of FS events can never fan out to a
/// storm of recomputes.
#[derive(Clone)]
pub struct RadarDirtySignal {
    inner: Arc<RadarDirtyInner>,
}

struct RadarDirtyInner {
    dirty: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl RadarDirtySignal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RadarDirtyInner {
                dirty: std::sync::atomic::AtomicBool::new(false),
                notify: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Mark the forest dirty and wake the worker. Cheap + non-blocking; callable
    /// from any thread (including a `notify` watcher callback).
    pub fn mark_dirty(&self) {
        self.inner
            .dirty
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify.notify_one();
    }
}

impl Default for RadarDirtySignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn THE single, long-lived radar recompute worker (Fix #1 — the 800%→≤1-core
/// cap). It is the ONLY place a recompute is dispatched, and it runs recomputes
/// strictly one-at-a-time:
///
/// 1. wait until the forest is dirty (sleep on the `Notify` otherwise);
/// 2. claim the work (clear the dirty flag) and coalesce a `debounce` window so a
///    rapid burst becomes ONE recompute;
/// 3. run `recompute` EXACTLY ONCE, on a blocking thread so it cannot starve the
///    async runtime, and `await` it so the next iteration cannot start until this
///    recompute has finished — **never two concurrent**;
/// 4. loop. A signal raised during the run left the flag set, so the latest state is
///    always eventually recomputed (at most one in-flight + one queued).
///
/// `recompute` is the (blocking) work — in production it recomputes the forest and
/// emits `radar_state`; tests inject a counting closure. Returns the worker's
/// `JoinHandle` (drop/abort to stop it).
///
/// Spawned via [`tauri::async_runtime::spawn`] so it does NOT require an ambient
/// Tokio runtime at the call site (Tauri's `setup()` runs without one) — Tauri's
/// global runtime is a full Tokio runtime, so the worker's internal
/// `tokio::time::sleep` / `spawn_blocking` resolve against it once the future runs.
pub fn spawn_radar_recompute_worker<F>(
    signal: RadarDirtySignal,
    debounce: Duration,
    recompute: F,
) -> tauri::async_runtime::JoinHandle<()>
where
    F: Fn() + Send + Sync + 'static,
{
    use std::sync::atomic::Ordering;
    let recompute = Arc::new(recompute);
    tauri::async_runtime::spawn(async move {
        loop {
            // The dirty flag is the SINGLE source of truth; `Notify` is only a wakeup
            // hint. Claim work by clearing the flag — if it was not set, sleep until a
            // signal and loop back to re-check (a stale/leftover `Notify` permit then
            // just causes a harmless re-check, never an extra recompute).
            if !signal.inner.dirty.swap(false, Ordering::SeqCst) {
                signal.inner.notify.notified().await;
                continue;
            }

            // Coalesce the burst: further signals during this window just re-set the
            // flag (claimed by the next iteration) — they do not stack recomputes.
            if !debounce.is_zero() {
                tokio::time::sleep(debounce).await;
                // Re-claim anything that arrived during the debounce window so it is
                // folded into THIS recompute rather than triggering another.
                signal.inner.dirty.store(false, Ordering::SeqCst);
            }

            // Run exactly one recompute, serialized: a blocking task we await, so the
            // loop cannot launch a second recompute until this one returns. A signal
            // raised during the run sets the flag again → exactly one follow-up.
            let job = recompute.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || job()).await {
                tracing::warn!(error=?e, "radar recompute task failed");
            }
        }
    })
}

/// Spawn the RADAR liveness watchers: the Claude `~/.claude/sessions` registry
/// (create/delete ⇒ globe bloom/implode) plus the Codex live + archived roots
/// (archive-move ⇒ done). On any change we recompute the whole forest from files
/// and emit `radar_state` — the forest is ephemeral, so a full recompute is the
/// honest, race-free path (FSEvents coalesces; see CLAUDE.md).
///
/// Fix #1: the watchers no longer recompute inline. Each FS event only calls
/// `signal.mark_dirty()` (cheap, non-blocking); a SINGLE [`spawn_radar_recompute_worker`]
/// drains the signal and runs `recompute_and_emit_radar` strictly one-at-a-time. This
/// caps a multi-root FSEvents storm at ~one core instead of N overlapping recomputes.
/// Best-effort: a missing root is skipped; the returned watchers + worker handle must
/// outlive `setup()`.
pub fn spawn_radar_watcher(
    store: Store,
    sessions_dir: PathBuf,
    extra_roots: Vec<PathBuf>,
    app: tauri::AppHandle,
) -> Result<(
    Vec<notify::RecommendedWatcher>,
    tauri::async_runtime::JoinHandle<()>,
)> {
    use notify::{EventKind, RecursiveMode, Watcher};

    let mut roots: Vec<PathBuf> = Vec::new();
    for r in std::iter::once(sessions_dir.clone()).chain(extra_roots) {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }

    // The single dirty signal shared by every watcher, drained by one worker that
    // recomputes + emits at most once per debounce window.
    let signal = RadarDirtySignal::new();
    let worker = {
        let store = store.clone();
        let app = app.clone();
        let sessions_dir = sessions_dir.clone();
        let signal = signal.clone();
        spawn_radar_recompute_worker(signal, debounce(), move || {
            recompute_and_emit_radar(&store, &sessions_dir, &app);
        })
    };

    let mut watchers = Vec::new();
    for root in roots {
        if !root.exists() {
            tracing::info!(root=%root.display(), "radar watch root absent; skipping");
            continue;
        }
        let signal = signal.clone();

        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error=?e, "radar watch error");
                        return;
                    }
                };
                // Any create/modify/remove changes liveness; ignore access events.
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                // Do NOT recompute on the watcher thread — just signal the worker. The
                // burst is coalesced + serialized there, so overlapping events across
                // roots cannot spawn overlapping recomputes.
                signal.mark_dirty();
            },
        )
        .context("create radar watcher")?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("radar watch {}", root.display()))?;
        tracing::info!(root=%root.display(), "watching for live agents (radar)");
        watchers.push(watcher);
    }
    Ok((watchers, worker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::claude_code::ClaudeCodeAdapter;
    use crate::ingest::codex::CodexAdapter;
    use crate::ingest::AdapterRegistry;
    use crate::ir::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    /// Fix #1 — COALESCING: a burst of N `mark_dirty()` signals must collapse to a
    /// SINGLE recompute per debounce window, and recomputes must NEVER overlap. This
    /// is what caps CPU at ~1 core: 800 FS events no longer fan out to 800 (or even
    /// N-concurrent) recomputes — at most one runs, at most one is queued behind it.
    #[tokio::test]
    async fn radar_recompute_worker_coalesces_burst_and_never_overlaps() {
        let runs = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));

        let signal = RadarDirtySignal::new();
        // Short debounce so the test is fast; the callback simulates a recompute that
        // takes real time, so an overlapping dispatch would be observable.
        let worker = {
            let runs = runs.clone();
            let in_flight = in_flight.clone();
            let max_in_flight = max_in_flight.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(40), move || {
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(cur, Ordering::SeqCst);
                // Simulate a non-trivial recompute body.
                std::thread::sleep(Duration::from_millis(30));
                runs.fetch_add(1, Ordering::SeqCst);
                in_flight.fetch_sub(1, Ordering::SeqCst);
            })
        };

        // Burst 1: 50 rapid signals (mimics an FSEvents storm across roots).
        for _ in 0..50 {
            signal.mark_dirty();
        }
        // Wait past the debounce + the simulated recompute body.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after_burst1 = runs.load(Ordering::SeqCst);
        assert!(
            (1..=2).contains(&after_burst1),
            "a 50-signal burst must collapse to 1 recompute (≤2 with a trailing edge), got {after_burst1}"
        );

        // Burst 2: the worker is long-lived and serves the next burst too.
        for _ in 0..50 {
            signal.mark_dirty();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after_burst2 = runs.load(Ordering::SeqCst);
        assert!(
            after_burst2 > after_burst1,
            "the worker must keep serving subsequent bursts ({after_burst1} -> {after_burst2})"
        );
        assert!(
            after_burst2 <= after_burst1 + 2,
            "the second burst must also coalesce, got {after_burst2} total"
        );

        // THE CAP: no two recomputes were ever in flight at once.
        assert_eq!(
            max_in_flight.load(Ordering::SeqCst),
            1,
            "recomputes must be strictly serialized — never two concurrent"
        );

        worker.abort();
    }

    /// A signal raised WHILE a recompute is running is not lost: the worker observes
    /// the dirty bit set during the run and performs exactly one follow-up recompute
    /// (the "+1 queued" guarantee — the latest state is always eventually emitted).
    #[tokio::test]
    async fn radar_recompute_worker_runs_once_more_for_signal_during_run() {
        let runs = Arc::new(AtomicUsize::new(0));
        let signal = RadarDirtySignal::new();
        let started = Arc::new(tokio::sync::Notify::new());

        let worker = {
            let runs = runs.clone();
            let started = started.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(10), move || {
                started.notify_one();
                std::thread::sleep(Duration::from_millis(60));
                runs.fetch_add(1, Ordering::SeqCst);
            })
        };

        // Kick the first recompute and wait until it has actually started running.
        signal.mark_dirty();
        started.notified().await;
        // Raise a new signal mid-run — it must trigger exactly one more recompute.
        signal.mark_dirty();

        tokio::time::sleep(Duration::from_millis(250)).await;
        let total = runs.load(Ordering::SeqCst);
        assert_eq!(
            total, 2,
            "a signal during a run yields exactly one follow-up recompute, got {total}"
        );
        worker.abort();
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
        let p = dir.join(
            "rollout-2026-06-19T16-33-00-019ee0ba-8295-7ba0-9971-c5af95e77191.jsonl",
        );
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
        assert!(first >= 1, "first ingest must add events, got {first}");
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
        assert_eq!(again, 0, "unchanged file must add 0 events (watermark resume)");
        let (_, events_after_resume, _) = store.counts().unwrap();
        assert_eq!(
            events_after_resume, events_after_first,
            "event count unchanged after a resume run"
        );
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
                Event::AssistantText { text } if text == "second" => Some(e.raw_ref.offset),
                _ => None,
            })
            .expect("appended 'second' event present");
        assert_eq!(
            appended, off_before,
            "appended event offset must be the absolute start of its line"
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
        let registry = AdapterRegistry::for_test(vec![Box::new(
            ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store.clone()),
        )]);
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
