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

/// Minimum spacing between two radar recomputes under a SUSTAINED event stream, in ms.
/// `WARDEN_RADAR_MIN_INTERVAL_MS` overrides (default 1000); `0` removes the floor.
///
/// This is a floor, not a delay: an isolated event still recomputes immediately (see the
/// leading edge in [`spawn_radar_recompute_worker`]), so it costs no emit latency in the
/// common case. It only bites when events arrive faster than the worker can serve them,
/// which is exactly the case that used to pin a core. One recompute is roughly 325ms of
/// CPU on a real corpus (a ~165ms live-context refresh plus a ~160ms forest recompute),
/// so a 1s floor caps the radar's steady-state cost near a third of one core.
fn radar_min_interval() -> Duration {
    let ms = std::env::var("WARDEN_RADAR_MIN_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1000);
    Duration::from_millis(ms)
}

/// The floor an URGENT recompute pays, derived from the append floor.
///
/// Urgent used to be exempt outright, and that was safe while the only urgent events
/// were a file being created or removed: rare by construction, so no floor was needed.
/// A COMPLETED TURN is now urgent too (`LiveIngest::lifecycle`), because it is the only
/// signal a subagent or teammate ever finishes by, and turns are commoner than files.
/// One eighth of the append floor, so 125ms at the 1s default:
///
/// * imperceptible on a globe arriving or leaving (well under one animation frame of
///   the 260ms implode), so the latency this whole change exists to remove stays gone;
/// * enough that a pathological fleet completing turns faster than the worker can
///   serve them degrades to a bounded rate instead of the old unbounded exemption.
///
/// Derived rather than given its own env var so `WARDEN_RADAR_MIN_INTERVAL_MS=0` still
/// means one thing: no floor anywhere.
pub(crate) fn urgent_min_interval(min_interval: Duration) -> Duration {
    min_interval / 8
}

fn kick_radar_after_watch_event(signal: &RadarDirtySignal) {
    signal.mark_dirty_with_live_refresh();
}

/// True when a filesystem event is a globe ARRIVING or LEAVING rather than one
/// growing: a subagent transcript being created, a session registry file being
/// written or removed, a Codex rollout being archived. These are the events the
/// radar is judged on, so they skip the recompute rate floor (see
/// [`RadarDirtySignal::mark_dirty_urgent`]); a plain content append does not.
fn is_structural_event(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    matches!(kind, EventKind::Create(_) | EventKind::Remove(_))
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
    // The same state, to any external host running our HUD bundle (the boring.notch
    // section). Mirrored here rather than intercepted at the Tauri event bus so the two
    // sinks are visible in one place and neither can silently outlive the other.
    if crate::bridge::has_subscribers() {
        if let Ok(v) = serde_json::to_value(&state) {
            crate::bridge::publish("radar_state", v);
        }
    }
    // An agent that has just stopped on the operator is the one state the menu bar
    // should not wait to be asked about. Fires on the TRANSITION only, and never steals
    // the keyboard: see `crate::attention`.
    if crate::attention::auto_open_enabled() {
        let waiting = crate::attention::newly_awaiting(&state);
        if !waiting.is_empty() {
            tracing::debug!(agents = ?waiting, "agent awaiting the operator; summoning the HUD");
            crate::summon_hud_for_attention(app);
        }
    }
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
    pub(crate) urgent: std::sync::atomic::AtomicBool,
    pub(crate) notify: tokio::sync::Notify,
}

impl RadarDirtySignal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RadarDirtyInner {
                dirty: std::sync::atomic::AtomicBool::new(false),
                refresh_live_context: std::sync::atomic::AtomicBool::new(false),
                urgent: std::sync::atomic::AtomicBool::new(false),
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

    /// Mark the forest dirty for a STRUCTURAL change and let it skip the rate floor.
    ///
    /// The floor exists to stop a sustained stream of transcript APPENDS from pinning
    /// a core, and against appends it is exactly right: a token count that lands a
    /// second late is invisible. A globe appearing or disappearing is not that. A
    /// subagent spawning, a subagent finishing, a session registry file being created
    /// or removed: those are the frames the whole radar exists to show, and holding
    /// them behind a one-second throttle is what read as "subagent tracking is slow".
    ///
    /// Safe to exempt because these events are RARE by construction. An append fires
    /// on every token; a file is created once and removed once. So the urgent path
    /// costs at most one extra recompute per real lifecycle event, while the sustained
    /// stream that the floor was built for still pays the floor in full.
    pub fn mark_dirty_urgent(&self) {
        self.inner
            .urgent
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // A structural change is exactly the case where the new bytes must be in the
        // store BEFORE the forest is assembled, or the new globe is one frame late.
        self.mark_dirty_with_live_refresh();
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
/// 3. hold a `min_interval` floor since the PREVIOUS recompute started, so a sustained
///    stream cannot drive them back-to-back. Leading edge: an isolated event skips this
///    entirely and runs at once;
/// 4. run `recompute` EXACTLY ONCE, on a blocking thread so it cannot starve the
///    async runtime, and `await` it so the next iteration cannot start until this
///    recompute has finished — **never two concurrent**;
/// 5. loop. A signal raised during the run left the flag set, so the latest state is
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
    min_interval: Duration,
    recompute: F,
) -> tauri::async_runtime::JoinHandle<()>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    use std::sync::atomic::Ordering;
    let recompute = Arc::new(recompute);
    tauri::async_runtime::spawn(async move {
        // Start of the previous recompute, for the leading-edge floor below.
        let mut last_start: Option<std::time::Instant> = None;
        loop {
            // The dirty flag is the SINGLE source of truth; `Notify` is only a wakeup
            // hint. Claim work by clearing the flag: if it was not set, sleep until a
            // signal and loop back to re-check (a stale/leftover `Notify` permit then
            // just causes a harmless re-check, never an extra recompute).
            if !signal.inner.dirty.swap(false, Ordering::SeqCst) {
                signal.inner.notify.notified().await;
                continue;
            }

            // Claim the urgency with the work. A structural event (a globe arriving or
            // leaving) skips the rate floor below; an append does not.
            let urgent = signal.inner.urgent.swap(false, Ordering::SeqCst);

            // Coalesce the burst: further signals during this window just re-set the
            // flag (claimed by the next iteration), they do not stack recomputes.
            if !debounce.is_zero() {
                tokio::time::sleep(debounce).await;
                // Re-claim anything that arrived during the debounce window so it is
                // folded into THIS recompute rather than triggering another.
                signal.inner.dirty.store(false, Ordering::SeqCst);
            }

            // Fix #2, the SUSTAINED-STREAM floor. Serialization caps concurrency at one
            // recompute, but not the RATE: with the default zero debounce, an event that
            // lands while a recompute is running starts the next one the instant it
            // returns, so a live transcript tail keeps a full core busy indefinitely.
            //
            // A leading edge keeps that from costing latency. The first event after a
            // quiet period runs at once (`last_start` is None, no wait), and only a
            // stream that is already saturating the worker waits out the remainder of
            // the window. Anything that arrives during the wait folds into this same
            // recompute rather than queueing another.
            //
            // An URGENT signal pays a far shorter floor. See `mark_dirty_urgent`:
            // those are the events a monitor is judged on, so they must not sit behind
            // a throttle built for token counts. See `urgent_min_interval` for why the
            // exemption is no longer total.
            let floor = if urgent {
                urgent_min_interval(min_interval)
            } else {
                min_interval
            };
            if let Some(prev) = last_start {
                let since = prev.elapsed();
                if since < floor {
                    tokio::time::sleep(floor - since).await;
                    signal.inner.dirty.store(false, Ordering::SeqCst);
                }
            }
            last_start = Some(std::time::Instant::now());

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
/// The watchers plus the two background handles (recompute worker, cadence tick)
/// and the shared dirty signal that `spawn_radar_watcher` returns to `setup()`.
type RadarWatcherHandles = (
    Vec<notify::RecommendedWatcher>,
    tauri::async_runtime::JoinHandle<()>,
    tauri::async_runtime::JoinHandle<()>,
    RadarDirtySignal,
);

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
) -> Result<RadarWatcherHandles> {
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
        spawn_radar_recompute_worker(
            signal,
            radar_recompute_debounce(),
            radar_min_interval(),
            move |refresh| {
                let n = recompute_and_emit_radar(&store, &sessions_dir, &app, &cache, refresh);
                agent_count.store(n, std::sync::atomic::Ordering::SeqCst);
            },
        )
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
            // Do NOT recompute on the watcher thread, just signal the worker. The
            // burst is coalesced + serialized there, so overlapping events across
            // roots cannot spawn overlapping recomputes. File events also request a
            // live transcript refresh so just-created subagent files are in the
            // store before the forest is assembled.
            if is_structural_event(&event.kind) {
                signal.mark_dirty_urgent();
            } else {
                kick_radar_after_watch_event(&signal);
            }
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
            // No rate-limit floor here: this case is about COALESCING a burst, which
            // must hold on its own. The floor is covered by the sustained-stream test.
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(40),
                Duration::ZERO,
                move |_| {
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    // Simulate a non-trivial recompute body.
                    std::thread::sleep(Duration::from_millis(30));
                    runs.fetch_add(1, Ordering::SeqCst);
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                },
            )
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
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(1),
                Duration::ZERO,
                move |_| {
                    runs.fetch_add(1, Ordering::SeqCst);
                },
            )
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
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(1),
                Duration::ZERO,
                move |refresh| {
                    refresh_flags.lock().unwrap().push(refresh);
                },
            )
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
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(1),
                Duration::ZERO,
                move |refresh| {
                    refresh_flags.lock().unwrap().push(refresh);
                },
            )
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
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(10),
                Duration::ZERO,
                move |_| {
                    started.notify_one();
                    std::thread::sleep(Duration::from_millis(60));
                    runs.fetch_add(1, Ordering::SeqCst);
                },
            )
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
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::from_millis(1),
                Duration::ZERO,
                move |_| {
                    runs.fetch_add(1, Ordering::SeqCst);
                },
            )
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

    /// The subagent-latency fix, and its guard rail.
    ///
    /// The rate floor is right for appends and wrong for lifecycle: a globe arriving
    /// or leaving sat behind up to a full second of throttle built for token counts,
    /// which is what "subagent tracking is slow, not on time" was. An urgent signal
    /// skips the floor. The second half of the test is the part that matters more:
    /// a plain signal must still pay it, or the pinned-core bug is back.
    #[tokio::test]
    async fn an_urgent_signal_skips_the_rate_floor_and_a_plain_one_still_pays_it() {
        let runs = Arc::new(AtomicUsize::new(0));
        let signal = RadarDirtySignal::new();
        let worker = {
            let runs = runs.clone();
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::ZERO,
                Duration::from_millis(400),
                move |_| {
                    runs.fetch_add(1, Ordering::SeqCst);
                },
            )
        };

        // First event runs at once (leading edge) and arms the floor.
        signal.mark_dirty();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // A plain follow-up inside the window waits for the floor to expire.
        signal.mark_dirty();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "an append inside the floor must wait, or the CPU cap is gone"
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 2, "and then it runs");

        // An URGENT follow-up inside the window pays only the urgent floor (400/8 =
        // 50ms here), not the 400ms append floor.
        signal.mark_dirty_urgent();
        tokio::time::sleep(Duration::from_millis(160)).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            3,
            "a globe arriving or leaving must not sit behind the append throttle"
        );

        worker.abort();
    }

    /// The urgent floor exists, and it is far enough under the append floor to be
    /// invisible on a globe arriving or leaving.
    ///
    /// Urgent used to be exempt outright, which was safe while the only urgent events
    /// were files appearing and vanishing. A COMPLETED TURN is urgent now (it is the
    /// only signal a subagent or teammate finishes by), and turns are commoner than
    /// files, so the exemption needed a bound. Both halves are asserted: too high and
    /// the latency fix is undone, too low and there is no bound at all.
    #[test]
    fn urgent_pays_a_floor_but_a_far_shorter_one() {
        let append = Duration::from_millis(1000); // the shipped default
        let urgent = urgent_min_interval(append);
        assert!(urgent < append / 4, "urgent must not inherit the append floor");
        assert!(
            urgent <= Duration::from_millis(150),
            "a globe arriving must still land inside one implode ({urgent:?})"
        );
        assert!(!urgent.is_zero(), "an unbounded exemption is what this replaced");
        // Disabling the floor disables it everywhere: one knob, one meaning.
        assert!(urgent_min_interval(Duration::ZERO).is_zero());
    }

    #[test]
    fn only_a_file_appearing_or_vanishing_counts_as_structural() {
        use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};
        use notify::EventKind;

        // A subagent transcript being written for the first time, and a session
        // registry file being removed when the process exits.
        assert!(is_structural_event(&EventKind::Create(CreateKind::File)));
        assert!(is_structural_event(&EventKind::Remove(RemoveKind::File)));
        // A transcript growing is the common case by orders of magnitude, and it is
        // exactly what the floor exists to bound.
        assert!(!is_structural_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(!is_structural_event(&EventKind::Access(
            notify::event::AccessKind::Read
        )));
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

    #[test]
    fn radar_min_interval_defaults_to_one_second_and_is_overridable() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WARDEN_RADAR_MIN_INTERVAL_MS");
        assert_eq!(
            radar_min_interval(),
            Duration::from_millis(1000),
            "the sustained-stream floor must be on by default, or the bug is back"
        );

        std::env::set_var("WARDEN_RADAR_MIN_INTERVAL_MS", "250");
        assert_eq!(radar_min_interval(), Duration::from_millis(250));

        // 0 removes the floor (the pre-fix behaviour, kept for debugging).
        std::env::set_var("WARDEN_RADAR_MIN_INTERVAL_MS", "0");
        assert_eq!(radar_min_interval(), Duration::ZERO);

        std::env::remove_var("WARDEN_RADAR_MIN_INTERVAL_MS");
    }

    /// Fix #2 (RATE LIMIT): a SUSTAINED event stream (not a burst) must not drive
    /// back-to-back recomputes. Coalescing alone does not bound this: with the default
    /// zero debounce, every event that lands after a recompute returns starts the next
    /// one immediately, so a live transcript stream pins a full core forever.
    ///
    /// The floor is a LEADING-edge throttle, so the two properties are tested together:
    /// the first event after a quiet period still runs immediately (emit latency is a
    /// product requirement), and the sustained rate stays under one per `min_interval`.
    #[tokio::test]
    async fn radar_recompute_worker_rate_limits_a_sustained_stream() {
        let runs = Arc::new(AtomicUsize::new(0));
        let first_run_at = Arc::new(Mutex::new(None::<Duration>));
        let t0 = std::time::Instant::now();

        let signal = RadarDirtySignal::new();
        let worker = {
            let runs = runs.clone();
            let first_run_at = first_run_at.clone();
            // Zero debounce == production default; a 200ms floor keeps the test quick.
            spawn_radar_recompute_worker(
                signal.clone(),
                Duration::ZERO,
                Duration::from_millis(200),
                move |_| {
                    let mut slot = first_run_at.lock().unwrap_or_else(|e| e.into_inner());
                    if slot.is_none() {
                        *slot = Some(t0.elapsed());
                    }
                    drop(slot);
                    // A recompute that costs real time, like the real one (~325ms).
                    std::thread::sleep(Duration::from_millis(50));
                    runs.fetch_add(1, Ordering::SeqCst);
                },
            )
        };

        // A SUSTAINED stream: one event every 20ms for 1s (a live transcript tail).
        for _ in 0..50 {
            signal.mark_dirty();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;

        let total = runs.load(Ordering::SeqCst);
        // ~1.25s of stream at a 200ms floor gives at most ~7 recomputes. Without the
        // floor this is ~20 (bounded only by the 50ms body), which is the bug.
        assert!(
            total <= 8,
            "a sustained stream must be rate-limited to ~one per floor, got {total} recomputes"
        );
        // ...and the stream must still be served, not starved.
        assert!(
            total >= 3,
            "the worker must keep serving the stream, got {total}"
        );

        // Leading edge: the FIRST recompute must not have waited out a floor.
        let first = first_run_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expect("worker ran at least once");
        assert!(
            first < Duration::from_millis(150),
            "the first event after a quiet period must recompute immediately, waited {first:?}"
        );

        worker.abort();
    }
}
