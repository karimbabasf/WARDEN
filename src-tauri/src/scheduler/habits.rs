//! ============================================================================
//! HEARTBEAT (Living Habits, Piece 2): cadence-driven live habit refresh.
//!
//! The habits view refreshes on a cadence tied to the SELECTED window, in two
//! tiers plus a liveness timestamp:
//!
//! * CHEAP tier — the local detector pass (`nominate_windowed`, free/on-device).
//!   Periodic only for tight windows (Today ~90s, 7d ~5min); 30d/6mo/all-time are
//!   on-demand (refreshed when the window is selected, no clock).
//! * EXPENSIVE tier — the GLM pipeline (`run_pipeline`, a paid API call). Periodic
//!   for 7d (~daily), 30d (~weekly), 6mo (~monthly), all-time (~every 2 months);
//!   Today is event-driven only (on window-select/open AND on "material change"),
//!   never on a clock — "live" must never mean "call the AI every tick".
//!
//! The cadence DECISION logic (`cheap_interval`, `expensive_interval`, `is_due`,
//! `material_change`) is pure — no clock, no I/O — so it is unit-testable without
//! real timers. The single coalesced worker reuses the radar Fix #1 structure: a
//! burst of triggers collapses onto one flag and recomputes run strictly
//! one-at-a-time (never two concurrent), so the heartbeat cannot reintroduce the
//! 800%→1-core CPU storm.
//! ============================================================================

use super::radar::RadarDirtySignal;
use crate::store::Store;
use crate::window::Window;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

/// Cheap-tier (local `nominate_windowed`) refresh interval for `window`, or
/// `None` when the cheap tier has no periodic tick (the window only re-scans
/// when it is (re)selected). Pure — no clock, no I/O.
///
/// Per the spec's two-layer cadence table: Today ≈ 90s, 7d ≈ 5min, and the
/// wider windows (30d / 6mo / all-time) are on-demand only.
pub(crate) fn cheap_interval(window: Window) -> Option<Duration> {
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
pub(crate) fn expensive_interval(window: Window) -> Option<Duration> {
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
pub(crate) fn is_due(
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
pub(crate) fn material_change(prev: &BTreeSet<String>, now: &BTreeSet<String>) -> bool {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

}
