//! RADAR recompute task (the WHEN of the live forest): a single coalesced
//! worker drains a dirty signal and recomputes the forest strictly
//! one-at-a-time, plus a liveness heartbeat and the byte-cheap dirty signal
//! the filesystem watchers raise.
//!
//! This is `crate::scheduler::radar` — the *task driver*. The forest
//! computation it invokes lives in the `crate::radar` DOMAIN module; all such
//! calls stay fully-qualified (`crate::radar::...`) so the two are never
//! confused.

use crate::store::Store;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Cached snapshot of the last fully-recomputed RADAR forest, shared between
/// the recompute worker (writer) and the FACE command handlers (readers).
///
/// Invalidation invariant: the forest is marked stale by a
/// [`RadarDirtySignal`] `mark_dirty` (raised by a live ingest, an FS watch
/// event, or the liveness heartbeat), never by mutating this value in place.
/// Until the single recompute worker finishes and SWAPS in a fresh
/// `RadarState`, readers keep observing the PREVIOUS snapshot — a stale read,
/// never a torn or empty one. The `RwLock` is held only for the brief swap in
/// [`cache_radar_state`] and the clone-out in [`latest_cached_radar_state`],
/// never across an `.await`, so a slow recompute can never block readers.
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

fn kick_radar_after_watch_event(signal: &RadarDirtySignal) {
    signal.mark_dirty_with_live_refresh();
}

/// RADAR (Task 9): recompute the live forest and emit it as `radar_state`. Thin
/// wrapper over [`crate::radar::recompute_radar_state`] + the Tauri emit, so the
/// watcher closure stays small.
pub(crate) fn recompute_and_emit_radar(
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
    pub(crate) inner: Arc<RadarDirtyInner>,
}

pub(crate) struct RadarDirtyInner {
    pub(crate) dirty: std::sync::atomic::AtomicBool,
    refresh_live_context: std::sync::atomic::AtomicBool,
    pub(crate) notify: tokio::sync::Notify,
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
pub(crate) fn spawn_radar_recompute_worker<F>(
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
pub(crate) fn spawn_radar_tick(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

}
