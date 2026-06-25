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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type RadarStateCache = Arc<std::sync::RwLock<Option<crate::radar::RadarState>>>;

pub fn new_radar_state_cache() -> RadarStateCache {
    Arc::new(std::sync::RwLock::new(None))
}

pub fn cache_radar_state(cache: &RadarStateCache, state: crate::radar::RadarState) {
    if let Ok(mut cached) = cache.write() {
        *cached = Some(state);
    }
}

pub fn latest_cached_radar_state(cache: &RadarStateCache) -> Option<crate::radar::RadarState> {
    cache.read().ok().and_then(|cached| cached.clone())
}

/// Debounce window for coalescing FSEvents bursts on a single file.
/// Override with `WARDEN_WATCH_DEBOUNCE_MS` (mirrors the `util.rs` env pattern).
fn debounce() -> Duration {
    let ms = std::env::var("WARDEN_WATCH_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(250);
    Duration::from_millis(ms)
}

/// RADAR recompute latency knob. Unlike ingest, RADAR should emit immediately by
/// default; the dirty flag already coalesces bursts and serializes recomputes.
/// Override with `WARDEN_RADAR_DEBOUNCE_MS` only when debugging event storms.
fn radar_recompute_debounce() -> Duration {
    let ms = std::env::var("WARDEN_RADAR_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
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
    if len == 0 {
        return Ok(0);
    }
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

    let (slice, watermark_offset) = complete_jsonl_prefix(&bytes[off as usize..], off);
    if slice.is_empty() {
        return Ok(0);
    }
    let batches = adapter
        .parse_range(path, slice, off, hash)
        .with_context(|| format!("parse_range {} @ {off}", path.display()))?;

    let existing_session_ids: HashSet<String> =
        store.sessions()?.into_iter().map(|s| s.id).collect();
    let mut activity = 0usize;
    for b in &batches {
        activity += b.events.len();
        if !existing_session_ids.contains(&b.session.id) {
            activity += 1;
        }
        // Persist through the last complete JSONL record. If a watcher fired while
        // a line was half-written, leave those bytes unwatermarked for retry.
        store.upsert_session_batch(&b.session, &b.turns, &b.events, watermark_offset)?;
    }
    Ok(activity)
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

                match ingest_file_once(&registry, &store, &path) {
                    Ok(activity) if activity > 0 => {
                        relink_after_live_ingest(&registry, &store, &path);
                        kick_radar_after_live_ingest(activity, radar_signal.as_ref());
                        let harness = harness_for_live_path(&registry, &path)
                            .map(|h| h.as_str().to_string())
                            .unwrap_or_default();
                        let _ = app.emit(
                            "ingest_progress",
                            serde_json::json!({
                                "harness": harness,
                                "path": path.to_string_lossy(),
                                "activity": activity,
                                "events": activity,
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

fn kick_radar_after_live_ingest(activity: usize, radar_signal: Option<&RadarDirtySignal>) {
    if activity > 0 {
        if let Some(signal) = radar_signal {
            signal.mark_dirty();
        }
    }
}

fn kick_radar_after_watch_event(signal: &RadarDirtySignal) {
    signal.mark_dirty_with_live_refresh();
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

/// RADAR (Task 9): recompute the live forest and emit it as `radar_state`. Thin
/// wrapper over [`crate::radar::recompute_radar_state`] + the Tauri emit, so the
/// watcher closure stays small.
pub fn recompute_and_emit_radar(
    store: &Store,
    sessions_dir: &std::path::Path,
    app: &tauri::AppHandle,
    cache: &RadarStateCache,
    refresh_live_context: bool,
) -> usize {
    use tauri::Emitter;
    if refresh_live_context {
        crate::radar::refresh_live_context(store, sessions_dir);
    }
    let state = crate::radar::recompute_radar_state(store, sessions_dir);
    let agent_count = state.agents.len();
    // Status breakdown for the logs — so "why is everything idle?" is answerable from
    // a single recompute line (esp. the startup kick) without attaching a debugger.
    let mut working = 0usize;
    let mut idle = 0usize;
    let mut terminated = 0usize;
    for a in &state.agents {
        match a.status.as_str() {
            "working" => working += 1,
            "terminated" => terminated += 1,
            _ => idle += 1,
        }
    }
    tracing::debug!(
        agents = agent_count,
        working,
        idle,
        terminated,
        "radar recompute emitted"
    );
    cache_radar_state(cache, state.clone());
    let _ = app.emit("radar_state", &state);
    agent_count
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
    refresh_live_context: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl RadarDirtySignal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RadarDirtyInner {
                dirty: std::sync::atomic::AtomicBool::new(false),
                refresh_live_context: std::sync::atomic::AtomicBool::new(false),
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

    /// Mark the forest dirty and request a one-shot live transcript refresh before
    /// the recompute. Used for startup/cold-read gaps only; heartbeat ticks should
    /// call [`mark_dirty`] so they remain cheap liveness checks.
    pub fn mark_dirty_with_live_refresh(&self) {
        self.inner
            .refresh_live_context
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.mark_dirty();
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
    F: Fn(bool) + Send + Sync + 'static,
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

            let refresh_live_context = signal
                .inner
                .refresh_live_context
                .swap(false, Ordering::SeqCst);

            // Run exactly one recompute, serialized: a blocking task we await, so the
            // loop cannot launch a second recompute until this one returns. A signal
            // raised during the run sets the flag again → exactly one follow-up.
            let job = recompute.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || job(refresh_live_context)).await {
                tracing::warn!(error=?e, "radar recompute task failed");
            }
        }
    })
}

/// Heartbeat interval (ms) for the radar liveness tick — see [`spawn_radar_tick`].
/// `WARDEN_RADAR_TICK_MS` overrides (default 2000); `0` disables the heartbeat.
fn radar_tick_ms() -> u64 {
    std::env::var("WARDEN_RADAR_TICK_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(2000)
}

/// Spawn the radar liveness heartbeat: every `interval`, IF the last recomputed forest
/// had at least one agent (`agent_count > 0`), mark the forest dirty so the single
/// recompute worker re-derives liveness.
///
/// Why: most transitions are driven by FS events (a registry status flip, a rollout
/// write, an archive move). But an agent that goes quiet writes nothing more, so a
/// purely event-driven radar can leave a globe stuck "working" (mtime-fallback agents:
/// older Claude / Codex) or fail to drop a terminated agent if FSEvents coalesces away
/// its removal. The heartbeat closes both gaps within one interval.
///
/// CPU-safe (the 800%→1-core invariant holds): when the forest is EMPTY this only
/// sleeps + loads an atomic — ZERO recomputes; when agents are open it raises at most
/// ONE coalesced recompute per interval (the worker still serializes + debounces).
/// `interval == 0` disables the heartbeat entirely.
pub fn spawn_radar_tick(
    signal: RadarDirtySignal,
    agent_count: Arc<std::sync::atomic::AtomicUsize>,
    interval: Duration,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if interval.is_zero() {
            return; // heartbeat disabled
        }
        loop {
            tokio::time::sleep(interval).await;
            // Gate on a non-empty forest: an idle machine only sleeps + loads an atomic,
            // never recomputes. A live forest raises ONE coalesced recompute per tick.
            if agent_count.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                signal.mark_dirty();
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
    cache: RadarStateCache,
) -> Result<(
    Vec<notify::RecommendedWatcher>,
    tauri::async_runtime::JoinHandle<()>,
    tauri::async_runtime::JoinHandle<()>,
    RadarDirtySignal,
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
    // Last forest size — written by the worker after each recompute, read by the
    // liveness heartbeat so it only ticks while at least one agent is open.
    let agent_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker = {
        let store = store.clone();
        let app = app.clone();
        let sessions_dir = sessions_dir.clone();
        let cache = cache.clone();
        let signal = signal.clone();
        let agent_count = agent_count.clone();
        spawn_radar_recompute_worker(signal, radar_recompute_debounce(), move |refresh| {
            let n = recompute_and_emit_radar(&store, &sessions_dir, &app, &cache, refresh);
            agent_count.store(n, std::sync::atomic::Ordering::SeqCst);
        })
    };
    // Liveness heartbeat: re-derive liveness every tick WHILE agents are open (settles a
    // stuck "working" globe / drops a termination FSEvents may have coalesced away);
    // zero recomputes when the forest is empty.
    let tick = spawn_radar_tick(
        signal.clone(),
        agent_count,
        Duration::from_millis(radar_tick_ms()),
    );

    let mut watchers = Vec::new();
    for root in roots {
        if !root.exists() {
            tracing::info!(root=%root.display(), "radar watch root absent; skipping");
            continue;
        }
        let signal = signal.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
            // roots cannot spawn overlapping recomputes. File events also request a
            // live transcript refresh so just-created subagent files are in the
            // store before the forest is assembled.
            kick_radar_after_watch_event(&signal);
        })
        .context("create radar watcher")?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("radar watch {}", root.display()))?;
        tracing::info!(root=%root.display(), "watching for live agents (radar)");
        watchers.push(watcher);
    }
    // STARTUP BOOTSTRAP: kick one recompute now, before any FS event. Without this the
    // worker sleeps on the dirty signal and the heartbeat is gated on `agent_count > 0`
    // (only set BY a recompute) — so neither can self-start, and already-running agents
    // render idle/absent until some unrelated FS write happens to fire. This first kick
    // evaluates the live registry + persistent store immediately AND seeds `agent_count`
    // so the heartbeat begins ticking. `lib.rs` kicks it a second time once startup
    // backfill has populated the store (handles a cold/empty DB with live agents).
    signal.mark_dirty_with_live_refresh();
    Ok((watchers, worker, tick, signal))
}

// ============================================================================
// HEARTBEAT (Living Habits, Piece 2): cadence-driven live habit refresh.
//
// The habits view refreshes on a cadence tied to the SELECTED window, in two
// tiers plus a liveness timestamp:
//
// * CHEAP tier — the local detector pass (`nominate_windowed`, free/on-device).
//   Periodic only for tight windows (Today ~90s, 7d ~5min); 30d/6mo/all-time are
//   on-demand (refreshed when the window is selected, no clock).
// * EXPENSIVE tier — the GLM pipeline (`run_pipeline`, a paid API call). Periodic
//   for 7d (~daily), 30d (~weekly), 6mo (~monthly), all-time (~every 2 months);
//   Today is event-driven only (on window-select/open AND on "material change"),
//   never on a clock — "live" must never mean "call the AI every tick".
//
// The cadence DECISION logic (`cheap_interval`, `expensive_interval`, `is_due`,
// `material_change`) is pure — no clock, no I/O — so it is unit-testable without
// real timers. The single coalesced worker reuses the radar Fix #1 structure: a
// burst of triggers collapses onto one flag and recomputes run strictly
// one-at-a-time (never two concurrent), so the heartbeat cannot reintroduce the
// 800%→1-core CPU storm.
// ============================================================================

use crate::window::Window;
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;

/// Cheap-tier (local `nominate_windowed`) refresh interval for `window`, or
/// `None` when the cheap tier has no periodic tick (the window only re-scans
/// when it is (re)selected). Pure — no clock, no I/O.
///
/// Per the spec's two-layer cadence table: Today ≈ 90s, 7d ≈ 5min, and the
/// wider windows (30d / 6mo / all-time) are on-demand only.
pub fn cheap_interval(window: Window) -> Option<Duration> {
    match window {
        Window::Today => Some(Duration::from_secs(90)),
        Window::D7 => Some(Duration::from_secs(5 * 60)),
        Window::D30 | Window::M6 | Window::AllTime => None,
    }
}

/// Expensive-tier (GLM `run_pipeline`) refresh interval for `window`, or `None`
/// when the expensive tier has no periodic tick. Pure — no clock, no I/O.
///
/// Per the spec (expensive refresh ≈ window ÷ 5): 7d ≈ daily, 30d ≈ weekly,
/// 6mo ≈ monthly, all-time ≈ every 2 months. Today returns `None`: its expensive
/// pass is event-driven only (window-select + material change), never clocked.
pub fn expensive_interval(window: Window) -> Option<Duration> {
    const DAY: u64 = 24 * 60 * 60;
    match window {
        Window::Today => None,
        Window::D7 => Some(Duration::from_secs(DAY)),
        Window::D30 => Some(Duration::from_secs(7 * DAY)),
        Window::M6 => Some(Duration::from_secs(30 * DAY)),
        Window::AllTime => Some(Duration::from_secs(60 * DAY)),
    }
}

/// Pure due-decision for a cadence tier — the seam that lets us test cadence
/// WITHOUT real wall-clock timers.
///
/// * `None` interval → `false` (this tier never ticks on a clock).
/// * `None` `last_run` → `true` (never run before → run now).
/// * otherwise → `true` iff at least `interval` has elapsed since `last_run`.
///
/// `now` is passed in (not read from the clock) so the decision is deterministic.
pub fn is_due(
    last_run: Option<DateTime<Utc>>,
    interval: Option<Duration>,
    now: DateTime<Utc>,
) -> bool {
    let Some(interval) = interval else {
        return false; // no periodic cadence for this tier/window
    };
    let Some(last_run) = last_run else {
        return true; // never run → due immediately
    };
    // `now - last_run` is a chrono Duration; compare against the std interval.
    // A clock skew that puts `last_run` in the future yields a negative delta →
    // not due (correct: we never run "early" because of a backwards clock).
    match (now - last_run).to_std() {
        Ok(elapsed) => elapsed >= interval,
        Err(_) => false, // negative elapsed (last_run is in the future)
    }
}

/// Pure material-change test for Today's event-driven expensive pass: `true` iff
/// the SET of raised pattern ids changed between two cheap scans (a new hole
/// appeared or an old one cleared). Order/count within a pattern is ignored —
/// only set membership matters, because the expensive pass re-reasons about the
/// *kinds* of holes, not their frequencies.
pub fn material_change(prev: &BTreeSet<String>, now: &BTreeSet<String>) -> bool {
    prev != now
}

/// The single source of truth the `set_habits_window` command and the background
/// heartbeat tick share. Held behind a `Mutex` so the command (which mutates the
/// active window + last-scan stamp) and the tick (which reads them to decide what
/// is due) never race. Cloned `Arc` handles point at the same inner state.
#[derive(Clone)]
pub struct HabitsHeartbeat {
    inner: Arc<std::sync::Mutex<HabitsHeartbeatState>>,
    /// Dirty signal + worker handshake guaranteeing scans never overlap. The
    /// command and the tick both `mark_dirty()`; one worker drains it serially.
    signal: RadarDirtySignal,
}

/// Inner heartbeat state (guarded by `HabitsHeartbeat.inner`).
struct HabitsHeartbeatState {
    /// The window the Habits view currently shows. Drives which cadences apply.
    window: Window,
    /// Wall-clock of the most recent cheap scan — emitted to the FACE as the
    /// `last_scanned_at` liveness stamp.
    last_scanned_at: Option<DateTime<Utc>>,
    /// Per-window timestamp of the most recent EXPENSIVE (GLM) pass. Keyed by the
    /// window wire string so 7d's daily clock is independent of 30d's weekly one.
    last_expensive_run: HashMap<&'static str, DateTime<Utc>>,
    /// The set of pattern ids the last Today cheap scan raised — compared against
    /// the next scan to detect a material change that arms Today's expensive pass.
    /// `None` means "no Today baseline yet": the first Today scan only establishes
    /// the baseline (it is NOT a change from nothing), so it never false-arms.
    prev_today_patterns: Option<BTreeSet<String>>,
    /// Armed when a Today material change is detected: the next tick runs the
    /// expensive pass once, then clears this (Today has no expensive clock).
    today_material_change: bool,
}

impl HabitsHeartbeat {
    /// Fresh heartbeat: defaults to the all-time window (the FACE's initial view)
    /// with no scans yet. `signal` is the dirty signal the worker drains.
    pub fn new(signal: RadarDirtySignal) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(HabitsHeartbeatState {
                window: Window::AllTime,
                last_scanned_at: None,
                last_expensive_run: HashMap::new(),
                prev_today_patterns: None,
                today_material_change: false,
            })),
            signal,
        }
    }

    /// The currently selected habits window.
    pub fn window(&self) -> Window {
        self.inner.lock().expect("habits heartbeat poisoned").window
    }

    /// The most recent cheap-scan timestamp (the liveness stamp), if any scan ran.
    pub fn last_scanned_at(&self) -> Option<DateTime<Utc>> {
        self.inner
            .lock()
            .expect("habits heartbeat poisoned")
            .last_scanned_at
    }

    /// Select a window: store it as active and reset the Today material-change
    /// baseline (a window switch is not itself a "material change"). The caller
    /// then runs one cheap scan immediately and kicks the worker.
    pub fn set_window(&self, window: Window) {
        let mut st = self.inner.lock().expect("habits heartbeat poisoned");
        st.window = window;
        // Switching windows resets Today's change tracking so the first Today
        // scan after a switch establishes a fresh baseline (no false-positive arm).
        if window != Window::Today {
            st.today_material_change = false;
            st.prev_today_patterns = None;
        }
    }

    /// Record a completed cheap scan: stamp `last_scanned_at = now`, and — when the
    /// active window is Today — diff the raised pattern ids against the previous
    /// Today scan; a change arms the (event-driven) expensive pass. Returns the
    /// `last_scanned_at` it just stored so the caller can emit it without re-locking.
    pub fn record_cheap_scan(&self, now: DateTime<Utc>, pattern_ids: BTreeSet<String>) -> DateTime<Utc> {
        let mut st = self.inner.lock().expect("habits heartbeat poisoned");
        st.last_scanned_at = Some(now);
        if st.window == Window::Today {
            // The FIRST Today scan (no baseline yet) only establishes the baseline —
            // it is not a "change from nothing", so it must not arm. Subsequent scans
            // diff against the established baseline.
            if let Some(prev) = &st.prev_today_patterns {
                if material_change(prev, &pattern_ids) {
                    st.today_material_change = true;
                }
            }
            st.prev_today_patterns = Some(pattern_ids);
        }
        now
    }

    /// True iff the cheap tier is due for the active window at `now` (the periodic
    /// cheap clock — Today/7d only). Pure decision over the stored last-scan stamp.
    pub fn cheap_due(&self, now: DateTime<Utc>) -> bool {
        let st = self.inner.lock().expect("habits heartbeat poisoned");
        is_due(st.last_scanned_at, cheap_interval(st.window), now)
    }

    /// True iff the expensive tier should run for the active window at `now`:
    /// either its per-window clock is due, OR (Today) a material change is armed.
    pub fn expensive_due(&self, now: DateTime<Utc>) -> bool {
        let st = self.inner.lock().expect("habits heartbeat poisoned");
        let clock_due = is_due(
            st.last_expensive_run.get(st.window.as_str()).copied(),
            expensive_interval(st.window),
            now,
        );
        clock_due || st.today_material_change
    }

    /// Record a completed expensive pass for the active window: stamp its
    /// per-window clock and disarm any Today material-change flag.
    pub fn record_expensive_run(&self, now: DateTime<Utc>) {
        let mut st = self.inner.lock().expect("habits heartbeat poisoned");
        let key = st.window.as_str();
        st.last_expensive_run.insert(key, now);
        st.today_material_change = false;
    }

    /// Wake the heartbeat worker (coalesced + serialized — the worker drains this).
    pub fn kick(&self) {
        self.signal.mark_dirty();
    }
}

/// Heartbeat tick interval (ms) — how often the coalesced worker re-evaluates
/// whether the active window's cheap/expensive tier is due. `WARDEN_HABITS_TICK_MS`
/// overrides (default 30000); `0` disables the periodic tick (event-driven only).
///
/// The tick is a cheap re-evaluation, NOT a scan: each tick only loads a few
/// timestamps and runs the pure due-decision. A scan happens only when `is_due`
/// fires, and the single worker serializes scans, so a fast tick cannot fan out
/// to overlapping GLM calls.
pub fn habits_tick_ms() -> u64 {
    std::env::var("WARDEN_HABITS_TICK_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30_000)
}

/// Coalesce window (ms) for the heartbeat worker: a burst of kicks (rapid
/// window-selects, or a tick firing while a select is mid-flight) folds into one
/// refresh. `WARDEN_HABITS_DEBOUNCE_MS` overrides (default 250); `0` disables.
pub fn habits_recompute_debounce() -> Duration {
    let ms = std::env::var("WARDEN_HABITS_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(250);
    Duration::from_millis(ms)
}

/// Spawn the heartbeat periodic tick: every `interval`, IF a cheap or expensive
/// tier is due for the active window, mark the worker dirty so it performs at most
/// ONE coalesced refresh. CPU-safe: when nothing is due (the common case — wide
/// windows with no cheap clock and an expensive clock measured in days) this only
/// sleeps + runs the pure due-check; it never scans on its own. `interval == 0`
/// disables the tick.
pub fn spawn_habits_tick(
    heartbeat: HabitsHeartbeat,
    interval: Duration,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if interval.is_zero() {
            return; // periodic tick disabled
        }
        loop {
            tokio::time::sleep(interval).await;
            let now = Utc::now();
            // Only wake the worker when something is actually due — an idle wide
            // window just sleeps + checks a couple of timestamps, zero scans.
            if heartbeat.cheap_due(now) || heartbeat.expensive_due(now) {
                heartbeat.kick();
            }
        }
    })
}

/// Run one cheap scan now for `window`: nominate windowed findings, project them
/// to `OrbIssue`s, and return them alongside the set of raised pattern ids (for
/// material-change tracking). Pure-ish: reads the store, no emit, no clock write.
/// Shared by the command (immediate scan on select) and the worker (periodic scan).
pub fn run_cheap_scan(
    store: &Store,
    now: DateTime<Utc>,
    window: Window,
) -> Result<(Vec<crate::commands::OrbIssue>, BTreeSet<String>)> {
    let issues = crate::commands::nominate_window_issues(store, window, now)?;
    let pattern_ids: BTreeSet<String> = issues.iter().map(|i| i.pattern_id.clone()).collect();
    Ok((issues, pattern_ids))
}

/// Emit `habits_refreshed` to the FACE: the windowed `OrbIssue`s + the
/// `last_scanned_at` liveness stamp (ISO-8601) + the active window. This is the
/// contract Piece 3 and the frontend consume for the cheap (live-feel) tier.
pub fn emit_habits_refreshed(
    app: &tauri::AppHandle,
    window: Window,
    issues: &[crate::commands::OrbIssue],
    last_scanned_at: DateTime<Utc>,
) {
    use tauri::Emitter;
    let _ = app.emit(
        "habits_refreshed",
        serde_json::json!({
            "window": window.as_str(),
            "issues": issues,
            "last_scanned_at": last_scanned_at.to_rfc3339(),
        }),
    );
}

/// Spawn a single coalesced worker over a [`RadarDirtySignal`] (the radar Fix #1
/// structure, generalized): drain the dirty flag, coalesce a burst across one
/// debounce window, then run the async `body` to completion before the next
/// iteration can start. Because the body is `await`ed, two bodies are NEVER in
/// flight at once — a storm of kicks collapses to at most one running + one queued
/// refresh. The radar worker keeps its own copy (it predates this and is
/// blocking-only); this is the async-body twin the heartbeat needs, and the seam
/// tests inject a counting closure into.
fn spawn_coalesced_signal_worker<F, Fut>(
    signal: RadarDirtySignal,
    debounce: Duration,
    body: F,
) -> tauri::async_runtime::JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    use std::sync::atomic::Ordering;
    let body = Arc::new(body);
    tauri::async_runtime::spawn(async move {
        loop {
            // Claim work by clearing the dirty flag; sleep on the Notify otherwise.
            if !signal.inner.dirty.swap(false, Ordering::SeqCst) {
                signal.inner.notify.notified().await;
                continue;
            }
            // Coalesce the burst: signals during the debounce window just re-set the
            // flag (claimed next iteration) rather than stacking refreshes.
            if !debounce.is_zero() {
                tokio::time::sleep(debounce).await;
                signal.inner.dirty.store(false, Ordering::SeqCst);
            }
            // Run the refresh to completion before looping — strictly serial, so the
            // next iteration cannot start a second refresh while this one runs. A
            // signal raised mid-run re-sets the flag → exactly one follow-up.
            body().await;
        }
    })
}

/// Spawn THE single heartbeat worker — the only place a habits scan is dispatched,
/// running scans strictly one-at-a-time (the radar Fix #1 cap, reused via
/// [`spawn_coalesced_signal_worker`]). On each wake, for the active window it
/// performs the cheap scan if its cheap tier is due, and runs the expensive GLM
/// pass if its expensive tier is due (or a Today material change is armed).
///
/// Serialization guarantee: the body runs inside an `await`ed coalesced worker so
/// two habit scans are never in flight at once — a burst of window-selects or ticks
/// cannot fan out to N concurrent GLM calls.
pub fn spawn_habits_worker(
    heartbeat: HabitsHeartbeat,
    store: Store,
    app: tauri::AppHandle,
    debounce: Duration,
) -> tauri::async_runtime::JoinHandle<()> {
    let signal = heartbeat.signal.clone();
    spawn_coalesced_signal_worker(signal, debounce, move || {
        let heartbeat = heartbeat.clone();
        let store = store.clone();
        let app = app.clone();
        async move {
            // CHEAP tier + the EXPENSIVE due-decision run on a blocking task (store
            // reads + a sync mutex); the async GLM pass runs after, still inside this
            // single awaited body so it serializes with the cheap scan.
            let join = tauri::async_runtime::spawn_blocking({
                let heartbeat = heartbeat.clone();
                let store = store.clone();
                let app = app.clone();
                move || {
                    let now = Utc::now();
                    // CHEAP tier: scan + emit only when its periodic clock is due.
                    if heartbeat.cheap_due(now) {
                        match run_cheap_scan(&store, now, heartbeat.window()) {
                            Ok((issues, pattern_ids)) => {
                                let stamp = heartbeat.record_cheap_scan(now, pattern_ids);
                                emit_habits_refreshed(&app, heartbeat.window(), &issues, stamp);
                            }
                            Err(e) => tracing::warn!(error=?e, "habits cheap scan failed"),
                        }
                    }
                    // EXPENSIVE tier: signal the GLM pass when its clock is due OR a
                    // Today material change is armed. Disarm + stamp regardless of
                    // outcome so a transient failure cannot wedge a tight retry loop.
                    if heartbeat.expensive_due(now) {
                        heartbeat.record_expensive_run(now);
                        Some(heartbeat.window())
                    } else {
                        None
                    }
                }
            })
            .await;

            match join {
                Ok(Some(window)) => run_habits_expensive(&store, &app, window).await,
                Ok(None) => {}
                Err(e) => tracing::warn!(error=?e, "habits heartbeat task failed"),
            }
        }
    })
}

/// Run the expensive GLM pass for `window` and emit `habits_diagnosed`. Windowed:
/// the GLM only re-reasons about the recent slice, which is what makes a frequent
/// refresh affordable. Best-effort — a brain error is logged, never fatal.
async fn run_habits_expensive(store: &Store, app: &tauri::AppHandle, window: Window) {
    use tauri::Emitter;
    let scope = crate::ir::RunScope {
        harness: None,
        query: None,
        force: None,
        max_files: None,
    };
    match crate::brain::Brain::new(store.clone())
        .with_app(app.clone())
        .run_pipeline(scope)
        .await
    {
        Ok(diagnosis) => {
            let _ = app.emit(
                "habits_diagnosed",
                serde_json::json!({
                    "window": window.as_str(),
                    "id": diagnosis.id,
                    "finding_count": diagnosis.ranked_findings.len(),
                }),
            );
        }
        Err(e) => tracing::warn!(error=?e, window=%window.as_str(), "habits expensive pass failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::claude_code::ClaudeCodeAdapter;
    use crate::ingest::codex::CodexAdapter;
    use crate::ingest::AdapterRegistry;
    use crate::ir::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::tempdir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ====================================================================
    // HEARTBEAT (Living Habits, Piece 2) — PURE cadence logic tests.
    // These never sleep on the wall clock: cadence is decided by `is_due`
    // over an injected `now`, so a "well after the interval" case is just a
    // timestamp far in the past, not a real timer.
    // ====================================================================

    use chrono::TimeZone;

    /// A fixed reference instant so cadence math is deterministic.
    fn hb_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap()
    }

    /// `is_due` truth table — the seam that lets cadence be tested without timers.
    #[test]
    fn is_due_truth_table() {
        let now = hb_now();
        let interval = Duration::from_secs(90);

        // None interval → never due, regardless of last_run.
        assert!(!is_due(None, None, now), "no interval, never run → not due");
        assert!(
            !is_due(Some(now - chrono::Duration::days(365)), None, now),
            "no interval → not due even if ancient"
        );

        // None last_run (with an interval) → due immediately.
        assert!(
            is_due(None, Some(interval), now),
            "never run, has interval → due now"
        );

        // Exactly at the interval boundary → due (>=).
        let exactly = now - chrono::Duration::seconds(90);
        assert!(
            is_due(Some(exactly), Some(interval), now),
            "elapsed == interval → due (inclusive)"
        );

        // Just before the interval → NOT due.
        let just_before = now - chrono::Duration::seconds(89);
        assert!(
            !is_due(Some(just_before), Some(interval), now),
            "elapsed just under interval → not due"
        );

        // Well after the interval → due.
        let well_after = now - chrono::Duration::hours(3);
        assert!(
            is_due(Some(well_after), Some(interval), now),
            "elapsed well past interval → due"
        );

        // last_run in the FUTURE (clock skew) → not due (never run early).
        let future = now + chrono::Duration::seconds(30);
        assert!(
            !is_due(Some(future), Some(interval), now),
            "last_run in the future → not due (no early run on backwards clock)"
        );
    }

    /// Cadence config returns the spec's intervals per window. Cheap: Today ~90s,
    /// 7d ~5min, wider = None. Expensive: Today None (event-driven), 7d daily,
    /// 30d weekly, 6mo monthly, all-time ~2 months.
    #[test]
    fn cadence_config_matches_spec_per_window() {
        const DAY: u64 = 24 * 60 * 60;

        // Cheap tier.
        assert_eq!(cheap_interval(Window::Today), Some(Duration::from_secs(90)));
        assert_eq!(cheap_interval(Window::D7), Some(Duration::from_secs(5 * 60)));
        assert_eq!(cheap_interval(Window::D30), None);
        assert_eq!(cheap_interval(Window::M6), None);
        assert_eq!(cheap_interval(Window::AllTime), None);

        // Expensive tier.
        assert_eq!(
            expensive_interval(Window::Today),
            None,
            "Today's expensive pass is event-driven only — no clock"
        );
        assert_eq!(
            expensive_interval(Window::D7),
            Some(Duration::from_secs(DAY))
        );
        assert_eq!(
            expensive_interval(Window::D30),
            Some(Duration::from_secs(7 * DAY))
        );
        assert_eq!(
            expensive_interval(Window::M6),
            Some(Duration::from_secs(30 * DAY))
        );
        assert_eq!(
            expensive_interval(Window::AllTime),
            Some(Duration::from_secs(60 * DAY))
        );
    }

    /// `material_change` is set-equality over pattern ids: identical → false; any
    /// add or removal → true.
    #[test]
    fn material_change_detects_set_membership_changes() {
        let a: BTreeSet<String> = ["p1".to_string(), "p2".to_string()].into_iter().collect();
        let same: BTreeSet<String> = ["p2".to_string(), "p1".to_string()].into_iter().collect();
        let added: BTreeSet<String> = ["p1".to_string(), "p2".to_string(), "p3".to_string()]
            .into_iter()
            .collect();
        let removed: BTreeSet<String> = ["p1".to_string()].into_iter().collect();
        let empty: BTreeSet<String> = BTreeSet::new();

        assert!(!material_change(&a, &same), "identical set (any order) → false");
        assert!(material_change(&a, &added), "an added id → true");
        assert!(material_change(&a, &removed), "a removed id → true");
        assert!(material_change(&empty, &a), "empty → populated → true");
        assert!(!material_change(&empty, &empty), "empty → empty → false");
    }

    /// Selecting a window stores it active; recording a cheap scan stamps
    /// `last_scanned_at` and (for Today) tracks the pattern-set baseline.
    #[test]
    fn heartbeat_set_window_and_record_scan_update_state() {
        let hb = HabitsHeartbeat::new(RadarDirtySignal::new());
        assert_eq!(hb.window(), Window::AllTime, "default window is all-time");
        assert_eq!(hb.last_scanned_at(), None, "no scan yet");

        hb.set_window(Window::D7);
        assert_eq!(hb.window(), Window::D7);

        let now = hb_now();
        let stamp = hb.record_cheap_scan(now, BTreeSet::new());
        assert_eq!(stamp, now);
        assert_eq!(hb.last_scanned_at(), Some(now));
    }

    /// Today's expensive pass arms on a material change between cheap scans, and a
    /// recorded expensive run disarms it (Today has no expensive clock).
    #[test]
    fn heartbeat_today_material_change_arms_and_disarms_expensive() {
        let hb = HabitsHeartbeat::new(RadarDirtySignal::new());
        hb.set_window(Window::Today);
        let now = hb_now();

        // First Today scan establishes a baseline — NOT itself a material change.
        let first: BTreeSet<String> = ["leak".to_string()].into_iter().collect();
        hb.record_cheap_scan(now, first);
        assert!(
            !hb.expensive_due(now),
            "first Today scan sets baseline, no clock → not due"
        );

        // A second scan with the SAME set → still not due.
        let same: BTreeSet<String> = ["leak".to_string()].into_iter().collect();
        hb.record_cheap_scan(now, same);
        assert!(!hb.expensive_due(now), "unchanged set → no arm");

        // A scan with a NEW pattern id → material change arms the expensive pass.
        let changed: BTreeSet<String> =
            ["leak".to_string(), "no_tests".to_string()].into_iter().collect();
        hb.record_cheap_scan(now, changed);
        assert!(
            hb.expensive_due(now),
            "new pattern id → Today expensive pass armed"
        );

        // Running the expensive pass disarms it.
        hb.record_expensive_run(now);
        assert!(
            !hb.expensive_due(now),
            "expensive run clears the Today material-change flag"
        );
    }

    /// Switching AWAY from Today clears any armed material-change flag (a window
    /// switch is not itself a material change for Today).
    #[test]
    fn heartbeat_switching_off_today_clears_arm() {
        let hb = HabitsHeartbeat::new(RadarDirtySignal::new());
        hb.set_window(Window::Today);
        let now = hb_now();
        hb.record_cheap_scan(now, ["a".to_string()].into_iter().collect());
        hb.record_cheap_scan(now, ["a".to_string(), "b".to_string()].into_iter().collect());
        assert!(hb.expensive_due(now), "armed on Today");

        hb.set_window(Window::D30);
        // D30's expensive clock has never run → it IS due (different reason), so to
        // isolate the disarm we stamp it and confirm no leftover Today arm remains.
        hb.record_expensive_run(now);
        assert!(
            !hb.expensive_due(now),
            "after switching off Today and stamping D30, no stale Today arm fires"
        );
    }

    /// Cheap-tier due-decision follows the per-window interval over the stored stamp.
    #[test]
    fn heartbeat_cheap_due_respects_window_interval() {
        let hb = HabitsHeartbeat::new(RadarDirtySignal::new());
        let now = hb_now();

        // Today: 90s cheap interval. Just-scanned → not due; 2min ago → due.
        hb.set_window(Window::Today);
        hb.record_cheap_scan(now, BTreeSet::new());
        assert!(!hb.cheap_due(now), "Today just scanned → not due");
        // Simulate a stale stamp by recording an old scan.
        hb.record_cheap_scan(now - chrono::Duration::seconds(120), BTreeSet::new());
        assert!(hb.cheap_due(now), "Today scanned 2min ago → due (>90s)");

        // All-time: no cheap clock → never periodically due.
        hb.set_window(Window::AllTime);
        hb.record_cheap_scan(now - chrono::Duration::days(30), BTreeSet::new());
        assert!(
            !hb.cheap_due(now),
            "all-time has no cheap clock → never periodically due"
        );
    }

    /// COALESCING + NO-OVERLAP regression (the heartbeat twin of the radar Fix #1
    /// test): a burst of kicks must collapse to a bounded number of body runs and
    /// the body must NEVER run concurrently with itself. Reuses the exact worker
    /// loop (`spawn_coalesced_signal_worker`) the heartbeat is built on, with a
    /// counting closure injected in place of the real scan.
    #[tokio::test]
    async fn habits_coalesced_worker_serializes_burst_and_never_overlaps() {
        let runs = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));

        let signal = RadarDirtySignal::new();
        let worker = {
            let runs = runs.clone();
            let in_flight = in_flight.clone();
            let max_in_flight = max_in_flight.clone();
            spawn_coalesced_signal_worker(signal.clone(), Duration::from_millis(40), move || {
                let runs = runs.clone();
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();
                async move {
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    // Simulate a non-trivial refresh body so an overlap would be observable.
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    runs.fetch_add(1, Ordering::SeqCst);
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                }
            })
        };

        // Burst: 50 rapid kicks (rapid dial-flicks + ticks landing together).
        for _ in 0..50 {
            signal.mark_dirty();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after_burst1 = runs.load(Ordering::SeqCst);
        assert!(
            (1..=2).contains(&after_burst1),
            "a 50-kick burst must collapse to 1 refresh (≤2 with a trailing edge), got {after_burst1}"
        );

        // The worker keeps serving subsequent bursts.
        for _ in 0..50 {
            signal.mark_dirty();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after_burst2 = runs.load(Ordering::SeqCst);
        assert!(
            after_burst2 > after_burst1 && after_burst2 <= after_burst1 + 2,
            "second burst also coalesces ({after_burst1} -> {after_burst2})"
        );

        // THE CAP: no two refreshes were ever in flight at once.
        assert_eq!(
            max_in_flight.load(Ordering::SeqCst),
            1,
            "habit refreshes must be strictly serialized — never two concurrent"
        );

        worker.abort();
    }

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
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(40), move |_| {
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

    /// STARTUP BOOTSTRAP: a single `mark_dirty()` with ZERO filesystem events must drive
    /// exactly one recompute. This is the contract `spawn_radar_watcher`'s startup kick
    /// relies on — without it the worker would sleep forever on the dirty signal and
    /// already-running agents would never be evaluated at launch.
    #[tokio::test]
    async fn radar_recompute_worker_bootstraps_on_initial_signal_without_fs_events() {
        let runs = Arc::new(AtomicUsize::new(0));
        let signal = RadarDirtySignal::new();
        let worker = {
            let runs = runs.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(1), move |_| {
                runs.fetch_add(1, Ordering::SeqCst);
            })
        };
        // The startup kick — no watcher has fired, this is the only signal.
        signal.mark_dirty();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the initial bootstrap signal must drive exactly one recompute at startup"
        );
        worker.abort();
    }

    #[tokio::test]
    async fn radar_recompute_worker_marks_live_refresh_only_when_requested() {
        let refresh_flags = Arc::new(Mutex::new(Vec::new()));
        let signal = RadarDirtySignal::new();
        let worker = {
            let refresh_flags = refresh_flags.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(1), move |refresh| {
                refresh_flags.lock().unwrap().push(refresh);
            })
        };

        signal.mark_dirty_with_live_refresh();
        tokio::time::sleep(Duration::from_millis(80)).await;
        signal.mark_dirty();
        tokio::time::sleep(Duration::from_millis(80)).await;

        assert_eq!(
            *refresh_flags.lock().unwrap(),
            vec![true, false],
            "startup/cold-read signals request live refresh, heartbeat-style signals do not"
        );
        worker.abort();
    }

    #[tokio::test]
    async fn radar_watch_event_requests_live_refresh_before_recompute() {
        let refresh_flags = Arc::new(Mutex::new(Vec::new()));
        let signal = RadarDirtySignal::new();
        let worker = {
            let refresh_flags = refresh_flags.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(1), move |refresh| {
                refresh_flags.lock().unwrap().push(refresh);
            })
        };

        kick_radar_after_watch_event(&signal);
        tokio::time::sleep(Duration::from_millis(80)).await;

        assert_eq!(
            *refresh_flags.lock().unwrap(),
            vec![true],
            "filesystem-triggered RADAR recomputes must ingest live transcript tails first"
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
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(10), move |_| {
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

    /// B2 — the liveness heartbeat: while at least one agent is open the tick raises a
    /// periodic recompute (so a stuck "working" globe settles and a missed termination
    /// still drops), and while the forest is EMPTY it raises NONE (the CPU invariant —
    /// an idle machine does zero recomputes).
    #[tokio::test]
    async fn radar_tick_signals_only_while_agents_present() {
        let runs = Arc::new(AtomicUsize::new(0));
        let agent_count = Arc::new(AtomicUsize::new(0));
        let signal = RadarDirtySignal::new();
        let worker = {
            let runs = runs.clone();
            spawn_radar_recompute_worker(signal.clone(), Duration::from_millis(1), move |_| {
                runs.fetch_add(1, Ordering::SeqCst);
            })
        };
        let tick = spawn_radar_tick(
            signal.clone(),
            agent_count.clone(),
            Duration::from_millis(20),
        );

        // Empty forest → the heartbeat must NOT fire any recompute.
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "no heartbeat recomputes while the forest is empty"
        );

        // Agents present → the heartbeat drives periodic recomputes.
        agent_count.store(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(120)).await;
        let active = runs.load(Ordering::SeqCst);
        assert!(
            active >= 2,
            "heartbeat should recompute while agents are open, got {active}"
        );

        // Forest empties again → ticking stops (count plateaus, allowing one in-flight).
        agent_count.store(0, Ordering::SeqCst);
        let frozen = runs.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(120)).await;
        let after = runs.load(Ordering::SeqCst);
        assert!(
            after <= frozen + 1,
            "no heartbeat after the forest empties ({frozen} -> {after})"
        );

        worker.abort();
        tick.abort();
    }

    #[test]
    fn live_ingest_with_new_events_marks_radar_dirty() {
        let signal = RadarDirtySignal::new();
        kick_radar_after_live_ingest(3, Some(&signal));

        assert!(
            signal.inner.dirty.load(Ordering::SeqCst),
            "successful live ingest must immediately wake the radar recompute worker"
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

    #[test]
    fn radar_recompute_debounce_defaults_to_zero_and_ignores_ingest_debounce() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WARDEN_WATCH_DEBOUNCE_MS", "250");
        std::env::remove_var("WARDEN_RADAR_DEBOUNCE_MS");
        assert_eq!(
            radar_recompute_debounce(),
            Duration::ZERO,
            "RADAR emit latency must not inherit the ingest debounce"
        );

        std::env::set_var("WARDEN_RADAR_DEBOUNCE_MS", "17");
        assert_eq!(radar_recompute_debounce(), Duration::from_millis(17));

        std::env::remove_var("WARDEN_WATCH_DEBOUNCE_MS");
        std::env::remove_var("WARDEN_RADAR_DEBOUNCE_MS");
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
                Event::AssistantText { text } if text == "second" => Some(e.raw_ref.offset),
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
                |(_, e)| matches!(&e.event, Event::AssistantText { text } if text == "second")
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
