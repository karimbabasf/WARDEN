//! Detecting the moment an armed agent finishes, and acting on it.
//!
//! ## Why a transition and not a state
//! The Claude registry writes `<pid>.json` ON TRANSITION ONLY, never on a timer:
//! across six live sessions, file mtime age and `statusUpdatedAt` age matched to
//! the second, which means the write IS the event. So an FSEvents watch fires at
//! the instant a session goes idle, with no polling latency. It also means the
//! file is not a heartbeat: a live session can sit unwritten for hours, so
//! freshness proves nothing and liveness is `kill(pid, 0)`.
//!
//! ## Why arming does not fire on an already-idle agent
//! The tracker is SEEDED with the status observed at arm time, and only a change
//! away from that seed counts as a transition. Arming an agent that is already
//! sitting idle therefore waits for its next piece of work to end, which is the
//! literal reading of "compact this when it is done working". The alternative
//! (fire immediately on click) would look like a misfire rather than a feature.
//!
//! ## Why a debounce at all, if the write is the event
//! `busy` to `shell` to `busy` churn is normal inside one turn, and a tool call
//! that returns instantly can produce a momentary idle. Holding idle for a couple
//! of seconds costs nothing and removes that whole class of misfire.

use super::arm;
use super::deliver;
use super::model::{ArmRecord, ArmState, IdleSource};
use super::registry::{self, RegistryEntry, SessionStatus};
use crate::store::Store;
use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long idle must HOLD before it counts. `WARDEN_COMPACT_DEBOUNCE_MS` overrides.
pub fn debounce_ms() -> i64 {
    std::env::var("WARDEN_COMPACT_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(2500)
}

/// How often the armed set is re-evaluated. FSEvents supplies the precise moment
/// of a transition; this tick is what lets the debounce hold expire when no
/// further write ever arrives. `WARDEN_COMPACT_POLL_MS` overrides.
pub fn poll_ms() -> u64 {
    std::env::var("WARDEN_COMPACT_POLL_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1000)
}

/// Silence, in seconds, before a Codex rollout counts as finished.
/// `WARDEN_COMPACT_CODEX_QUIET_SECS` overrides.
pub fn codex_quiet_secs() -> u64 {
    std::env::var("WARDEN_COMPACT_CODEX_QUIET_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(20)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    status: SessionStatus,
    /// When this run of this status began.
    since: DateTime<Utc>,
    /// False for the seeded status: only a status the tracker watched CHANGE can
    /// fire. This is the whole "already idle does not count" rule.
    transitioned: bool,
    /// Set once a fire is reported, so a held idle cannot fire on every tick.
    fired: bool,
}

/// The pure idle-transition detector. No clock, no filesystem, no syscalls: it is
/// fed samples and a `now`, and returns the ids that just became eligible.
#[derive(Debug)]
pub struct IdleTracker {
    debounce: ChronoDuration,
    seen: HashMap<String, Observation>,
}

impl IdleTracker {
    pub fn new(debounce: ChronoDuration) -> Self {
        IdleTracker {
            debounce,
            seen: HashMap::new(),
        }
    }

    /// Record the status an agent had when it was armed.
    ///
    /// Seeding is what stops the first observation from looking like a
    /// transition. It is called on arm, and again for every pending record at
    /// startup (from the persisted baseline), so a restart cannot turn a
    /// long-idle session into an instant fire.
    pub fn seed(&mut self, id: &str, status: SessionStatus, now: DateTime<Utc>) {
        self.seen.insert(
            id.to_string(),
            Observation {
                status,
                since: now,
                transitioned: false,
                fired: false,
            },
        );
    }

    /// Drop an agent, on cancel or once its record is terminal.
    pub fn forget(&mut self, id: &str) {
        self.seen.remove(id);
    }

    pub fn is_tracking(&self, id: &str) -> bool {
        self.seen.contains_key(id)
    }

    /// Feed one round of samples. Returns the ids that have now transitioned INTO
    /// idle and held there for the debounce window.
    pub fn observe(&mut self, samples: &[(String, SessionStatus)], now: DateTime<Utc>) -> Vec<String> {
        let mut fires = Vec::new();
        for (id, status) in samples {
            let entry = self.seen.entry(id.clone()).or_insert(Observation {
                // An unseeded id is treated as freshly seeded, never as a
                // transition. Failing closed here means a bug that loses the seed
                // costs a missed compaction, not an unwanted one.
                status: *status,
                since: now,
                transitioned: false,
                fired: false,
            });
            if entry.status != *status {
                *entry = Observation {
                    status: *status,
                    since: now,
                    transitioned: true,
                    fired: false,
                };
            }
            let held = now.signed_duration_since(entry.since);
            if entry.status.is_idle() && entry.transitioned && !entry.fired && held >= self.debounce
            {
                entry.fired = true;
                fires.push(id.clone());
            }
        }
        fires
    }
}

/// Codex liveness as a status, from the rollout's location and age.
///
/// Weaker evidence than the Claude registry and labelled as such in the contract
/// (`idleSource`): a rollout that has not been written for the quiet window is
/// PROBABLY done, whereas a registry `idle` IS done. A rollout that has left the
/// live sessions root has been archived, which means finished, not idle.
pub fn codex_quiet_status(
    in_sessions: bool,
    mtime_secs_ago: Option<u64>,
    quiet_secs: u64,
) -> SessionStatus {
    if !in_sessions {
        return SessionStatus::Gone;
    }
    match mtime_secs_ago {
        Some(secs) if secs >= quiet_secs => SessionStatus::Idle,
        Some(_) => SessionStatus::Busy,
        None => SessionStatus::Unknown,
    }
}

/// What one evaluation pass decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassOutcome {
    /// Agent ids whose idle transition has held long enough to act on.
    pub fires: Vec<String>,
    /// Agent ids whose session went away before it ever went idle, with the
    /// sentence to record on the row.
    pub expired: Vec<(String, String)>,
}

/// Decide what to do about the pending set (pure, with injected effects).
///
/// Everything that reads the world is a closure, so the decision itself is
/// testable without live sessions: `codex_status` resolves a Codex rollout and
/// `alive` is the process check.
pub fn evaluate(
    pending: &[ArmRecord],
    registry: &[RegistryEntry],
    codex_status: &dyn Fn(&ArmRecord) -> SessionStatus,
    alive: &dyn Fn(u32) -> bool,
    tracker: &mut IdleTracker,
    now: DateTime<Utc>,
) -> PassOutcome {
    let by_session: HashMap<&str, &RegistryEntry> = registry
        .iter()
        .map(|e| (e.session_id.as_str(), e))
        .collect();

    let mut samples = Vec::with_capacity(pending.len());
    let mut out = PassOutcome::default();

    for rec in pending {
        let status = match rec.idle_source {
            IdleSource::RegistryTransition => by_session
                .get(rec.session_id.as_str())
                .map(|e| e.status)
                .unwrap_or(SessionStatus::Gone),
            IdleSource::CodexQuietWindow => codex_status(rec),
        };

        if status == SessionStatus::Gone {
            // No registry entry. For a process-backed session that is only
            // conclusive once the pid is really gone: the file could have been
            // removed while the process lives, and expiring a live agent would
            // silently disarm something the user still wants.
            let really_gone = match (rec.idle_source, rec.pid) {
                (IdleSource::RegistryTransition, Some(pid)) => !alive(pid),
                _ => true,
            };
            if really_gone {
                out.expired.push((
                    rec.agent_id.clone(),
                    "the session closed before it went idle, so nothing was sent".to_string(),
                ));
                continue;
            }
        }
        samples.push((rec.agent_id.clone(), status));
    }

    out.fires = tracker.observe(&samples, now);
    out
}

/// Age in whole seconds of a path's last write, or `None` when it cannot be read.
pub(crate) fn mtime_secs_ago(path: &Path, now: DateTime<Utc>) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let modified: DateTime<Utc> = modified.into();
    now.signed_duration_since(modified).num_seconds().try_into().ok()
}

/// Resolve a Codex thread's status by finding its rollout under the live sessions
/// root. The rollout filename embeds the thread uuid, which is the session id.
///
/// Shared with the façade so arming and watching read the thread the same way: a
/// baseline computed differently from the samples would misjudge the very first
/// transition.
pub(crate) fn codex_status_now(thread_id: &str, sessions_root: &Path) -> SessionStatus {
    codex_status_at(thread_id, sessions_root, Utc::now())
}

fn codex_status_at(thread_id: &str, sessions_root: &Path, now: DateTime<Utc>) -> SessionStatus {
    let Some(path) = find_rollout(sessions_root, thread_id) else {
        return SessionStatus::Gone;
    };
    codex_quiet_status(true, mtime_secs_ago(&path, now), codex_quiet_secs())
}

/// Locate `~/.codex/sessions/YYYY/MM/DD/rollout-<iso>-<uuid>.jsonl` for one uuid.
fn find_rollout(root: &Path, thread_id: &str) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    walkdir::WalkDir::new(root)
        .max_depth(4)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| {
            e.file_type().is_file()
                && e.file_name()
                    .to_str()
                    .map(|n| n.contains(thread_id))
                    .unwrap_or(false)
        })
        .map(|e| e.into_path())
}

/// Receives the armed set whenever it changes, so the frontend can re-render.
/// A closure rather than an `AppHandle` keeps this module free of Tauri and
/// leaves the event name a decision of the layer that owns IPC.
pub type StatusSink = Arc<dyn Fn() + Send + Sync>;

/// The watcher's handles. They must outlive `setup()`, so the caller parks them
/// in Tauri's managed state.
pub struct CompactWatcher {
    #[allow(dead_code)]
    watchers: Vec<notify::RecommendedWatcher>,
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    worker: std::thread::JoinHandle<()>,
}

impl Drop for CompactWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start watching for armed agents going idle.
///
/// The worker is a plain OS thread rather than an async task on purpose: every
/// step it takes is blocking (reading the registry, a SQLite write, and possibly
/// an `osascript` round trip that can take a hundred milliseconds), and parking
/// that on an async runtime thread would stall unrelated work for no benefit.
pub fn spawn(
    store: Store,
    sessions_dir: PathBuf,
    codex_sessions_dir: PathBuf,
    sink: StatusSink,
) -> Result<CompactWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};

    let stop = Arc::new(AtomicBool::new(false));
    // Set by FSEvents so a transition is evaluated at the instant it is written
    // rather than up to one tick later. The tick still runs, because the debounce
    // hold has to expire even when no further write ever arrives.
    let kicked = Arc::new(AtomicBool::new(false));

    let mut watchers = Vec::new();
    for root in [sessions_dir.clone(), codex_sessions_dir.clone()] {
        if !root.exists() {
            tracing::info!(root=%root.display(), "compact watch root absent; skipping");
            continue;
        }
        let kicked = kicked.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                kicked.store(true, Ordering::SeqCst);
            }
        })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        watchers.push(watcher);
    }

    let worker = {
        let stop = stop.clone();
        let interval = Duration::from_millis(poll_ms());
        let debounce = ChronoDuration::milliseconds(debounce_ms());
        std::thread::Builder::new()
            .name("warden-compact".into())
            .spawn(move || {
                let mut tracker = IdleTracker::new(debounce);
                // Re-seed anything left armed by a previous run from its stored
                // baseline, so a restart never converts a long-idle session into
                // an immediate fire.
                if let Ok(pending) = arm::pending(&store) {
                    let now = Utc::now();
                    for rec in pending {
                        tracker.seed(
                            &rec.agent_id,
                            registry::parse_status(Some(&rec.baseline_status)),
                            now,
                        );
                    }
                }
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(interval);
                    kicked.store(false, Ordering::SeqCst);
                    match run_pass(&store, &sessions_dir, &codex_sessions_dir, &mut tracker) {
                        Ok(true) => sink(),
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(error=%format!("{e:#}"), "compact pass failed")
                        }
                    }
                }
            })?
    };

    Ok(CompactWatcher {
        watchers,
        stop,
        worker,
    })
}

/// One full pass: evaluate, expire what is gone, fire what is ready. Returns
/// whether anything changed, so the sink only fires on real transitions.
fn run_pass(
    store: &Store,
    sessions_dir: &Path,
    codex_sessions_dir: &Path,
    tracker: &mut IdleTracker,
) -> Result<bool> {
    let pending = arm::pending(store)?;
    if pending.is_empty() {
        return Ok(false);
    }
    let now = Utc::now();
    let entries = registry::snapshot(sessions_dir);
    let codex = |rec: &ArmRecord| codex_status_at(&rec.session_id, codex_sessions_dir, now);
    let outcome = evaluate(
        &pending,
        &entries,
        &codex,
        &crate::platform::process_alive,
        tracker,
        now,
    );

    let mut changed = false;
    for (agent_id, reason) in &outcome.expired {
        if arm::settle(store, agent_id, ArmState::Expired, Some(reason), now)? {
            tracker.forget(agent_id);
            changed = true;
        }
    }

    for agent_id in &outcome.fires {
        // Claim the record first. The UPDATE is conditional on it still being
        // armed, so a cancel that landed a moment ago wins and nothing is sent.
        if !arm::settle(store, agent_id, ArmState::Firing, None, now)? {
            tracker.forget(agent_id);
            continue;
        }
        changed = true;
        let Some(rec) = arm::get(store, agent_id)? else {
            continue;
        };
        let result = deliver::fire(&rec);
        arm::force_state(store, agent_id, result.state, result.detail.as_deref())?;
        tracker.forget(agent_id);
        if result.automation_denied {
            super::set_automation_denied();
        }
        tracing::info!(
            agent = %agent_id,
            state = result.state.as_str(),
            detail = result.detail.as_deref().unwrap_or(""),
            "compact arm fired"
        );
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_785_183_890 + secs, 0).expect("valid timestamp")
    }

    fn tracker() -> IdleTracker {
        IdleTracker::new(ChronoDuration::milliseconds(2500))
    }

    fn sample(status: SessionStatus) -> Vec<(String, SessionStatus)> {
        vec![("a".to_string(), status)]
    }

    #[test]
    fn a_busy_session_that_goes_idle_fires_once_the_hold_expires() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        assert!(t.observe(&sample(SessionStatus::Busy), at(1)).is_empty());
        // The transition itself is not enough: the hold has not elapsed.
        assert!(t.observe(&sample(SessionStatus::Idle), at(10)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Idle), at(11)).is_empty());
        assert_eq!(t.observe(&sample(SessionStatus::Idle), at(13)), vec!["a"]);
        // And never twice for the same transition.
        assert!(t.observe(&sample(SessionStatus::Idle), at(20)).is_empty());
    }

    /// The single most dangerous misfire: `shell` means a foreground command is
    /// running, and one real session held it for about three hours.
    #[test]
    fn shell_never_fires_no_matter_how_long_it_is_held() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        for secs in [1, 60, 3600, 10_800] {
            assert!(
                t.observe(&sample(SessionStatus::Shell), at(secs)).is_empty(),
                "shell must never fire (t+{secs}s)"
            );
        }
        // And it still fires correctly once the command really ends.
        assert!(t.observe(&sample(SessionStatus::Idle), at(10_801)).is_empty());
        assert_eq!(
            t.observe(&sample(SessionStatus::Idle), at(10_805)),
            vec!["a"]
        );
    }

    #[test]
    fn an_already_idle_agent_waits_for_its_next_piece_of_work() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Idle, at(0));
        // Seeded idle is not a transition, however long it holds.
        assert!(t.observe(&sample(SessionStatus::Idle), at(30)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Idle), at(600)).is_empty());
        // It works, then finishes: now it counts.
        assert!(t.observe(&sample(SessionStatus::Busy), at(700)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Idle), at(800)).is_empty());
        assert_eq!(t.observe(&sample(SessionStatus::Idle), at(805)), vec!["a"]);
    }

    #[test]
    fn a_flicker_back_to_busy_restarts_the_hold() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        assert!(t.observe(&sample(SessionStatus::Idle), at(10)).is_empty());
        // Idle for a moment, then work resumes before the hold expires.
        assert!(t.observe(&sample(SessionStatus::Busy), at(11)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Idle), at(12)).is_empty());
        // The clock restarted at t+12, so t+13 is still too early.
        assert!(t.observe(&sample(SessionStatus::Idle), at(13)).is_empty());
        assert_eq!(t.observe(&sample(SessionStatus::Idle), at(15)), vec!["a"]);
    }

    #[test]
    fn an_unknown_status_is_never_treated_as_idle() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        assert!(t.observe(&sample(SessionStatus::Unknown), at(10)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Unknown), at(60)).is_empty());
    }

    #[test]
    fn cancelling_stops_the_tracker_watching_it() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        assert!(t.is_tracking("a"));
        t.forget("a");
        assert!(!t.is_tracking("a"));
        // Re-observing an unknown id seeds it rather than firing.
        assert!(t.observe(&sample(SessionStatus::Idle), at(10)).is_empty());
        assert!(t.observe(&sample(SessionStatus::Idle), at(20)).is_empty());
    }

    #[test]
    fn codex_quiet_status_reads_location_first_then_age() {
        let quiet = 20;
        assert_eq!(
            codex_quiet_status(false, Some(999), quiet),
            SessionStatus::Gone,
            "an archived rollout is finished, not idle"
        );
        assert_eq!(codex_quiet_status(true, Some(5), quiet), SessionStatus::Busy);
        assert_eq!(codex_quiet_status(true, Some(20), quiet), SessionStatus::Idle);
        assert_eq!(
            codex_quiet_status(true, None, quiet),
            SessionStatus::Unknown,
            "an unreadable mtime must not read as idle"
        );
    }

    // ---- evaluate(): the pass decision, with the world injected ----

    fn rec(agent: &str, session: &str, pid: Option<u32>, source: IdleSource) -> ArmRecord {
        use super::super::model::{ArmState, DeliveryMode};
        ArmRecord {
            agent_id: agent.to_string(),
            session_id: session.to_string(),
            pid,
            harness: "claude_code".to_string(),
            mode: DeliveryMode::TerminalAppleScript,
            mode_reason: "Terminal tab on /dev/ttys001".to_string(),
            idle_source: source,
            label: "machine-89".to_string(),
            baseline_status: "busy".to_string(),
            armed_at: at(0),
            state: ArmState::Armed,
            fired_at: None,
            detail: None,
        }
    }

    fn entry(session: &str, status: SessionStatus) -> RegistryEntry {
        RegistryEntry {
            pid: 71015,
            session_id: session.to_string(),
            cwd: "/Users/dev".to_string(),
            entrypoint: "cli".to_string(),
            name: "machine-89".to_string(),
            status,
        }
    }

    #[test]
    fn evaluate_fires_a_registry_session_that_settles_into_idle() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        let pending = vec![rec("a", "sess-1", Some(71015), IdleSource::RegistryTransition)];
        let no_codex = |_: &ArmRecord| SessionStatus::Gone;
        let alive = |_: u32| true;

        let first = evaluate(
            &pending,
            &[entry("sess-1", SessionStatus::Idle)],
            &no_codex,
            &alive,
            &mut t,
            at(10),
        );
        assert!(first.fires.is_empty(), "the hold has not elapsed yet");

        let second = evaluate(
            &pending,
            &[entry("sess-1", SessionStatus::Idle)],
            &no_codex,
            &alive,
            &mut t,
            at(14),
        );
        assert_eq!(second.fires, vec!["a"]);
        assert!(second.expired.is_empty());
    }

    #[test]
    fn a_session_that_exits_before_going_idle_expires_and_sends_nothing() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        let pending = vec![rec("a", "sess-1", Some(71015), IdleSource::RegistryTransition)];
        let out = evaluate(
            &pending,
            &[], // the registry file is gone
            &|_| SessionStatus::Gone,
            &|_| false, // and so is the process
            &mut t,
            at(10),
        );
        assert!(out.fires.is_empty());
        assert_eq!(out.expired.len(), 1);
        assert_eq!(out.expired[0].0, "a");
        assert!(out.expired[0].1.contains("nothing was sent"));
    }

    /// A missing registry file while the process is still alive is not proof of
    /// anything: expiring there would silently disarm a live agent.
    #[test]
    fn a_missing_registry_file_with_a_live_pid_keeps_waiting() {
        let mut t = tracker();
        t.seed("a", SessionStatus::Busy, at(0));
        let pending = vec![rec("a", "sess-1", Some(71015), IdleSource::RegistryTransition)];
        let out = evaluate(&pending, &[], &|_| SessionStatus::Gone, &|_| true, &mut t, at(10));
        assert!(out.expired.is_empty());
        assert!(out.fires.is_empty());
    }

    #[test]
    fn a_codex_record_uses_the_quiet_window_and_ignores_the_registry() {
        let mut t = tracker();
        t.seed("c", SessionStatus::Busy, at(0));
        let pending = vec![rec("c", "thread-1", None, IdleSource::CodexQuietWindow)];
        // The registry says nothing about Codex; the injected resolver decides.
        let busy = evaluate(
            &pending,
            &[],
            &|_| SessionStatus::Busy,
            &|_| true,
            &mut t,
            at(5),
        );
        assert!(busy.fires.is_empty() && busy.expired.is_empty());

        assert!(evaluate(&pending, &[], &|_| SessionStatus::Idle, &|_| true, &mut t, at(10))
            .fires
            .is_empty());
        let fired = evaluate(&pending, &[], &|_| SessionStatus::Idle, &|_| true, &mut t, at(14));
        assert_eq!(fired.fires, vec!["c"]);
    }

    #[test]
    fn an_archived_codex_thread_expires_without_a_pid_check() {
        let mut t = tracker();
        t.seed("c", SessionStatus::Busy, at(0));
        let pending = vec![rec("c", "thread-1", None, IdleSource::CodexQuietWindow)];
        let out = evaluate(&pending, &[], &|_| SessionStatus::Gone, &|_| true, &mut t, at(10));
        assert_eq!(out.expired.len(), 1);
        assert_eq!(out.expired[0].0, "c");
    }
}
