//! Living-Habits Piece 3 — "implode-when-fixed": the resolution model.
//!
//! A habit (a `pattern_id` raised in a window) is **easy to flag, hard to clear**.
//! Clearing is a TIME-GATED clean streak (spaced repetition): you earn `K` clean
//! "credits", and a credit only counts if at least `S` has elapsed since the last
//! counted credit — you cannot cram `K` clean sessions into one afternoon. A single
//! slip resets the streak to 0. `K` and `S` scale with the window: a wider window
//! demands more, spread further apart, before the habit implodes (resolves).
//!
//! Everything here is PURE and testable with passed-in timestamps — no real clock,
//! no sleeps. The streak core `streak_state` is the heart of the piece. The only
//! impure leaf is [`durable_erase_fixed`], which (gated on the AllTime window only)
//! erases a resolved habit's CLAUDE.md guardrail via the Piece-4 forge apply-path.

use crate::detectors::session_trips_pattern;
use crate::ir::FeatureVector;
use crate::store::Store;
use crate::window::Window;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// `K` — the number of time-gated clean credits a habit must earn in `window`
/// before it implodes (resolves). Wider windows demand a longer clean streak.
/// Pure config; mirrors the spec table exactly.
pub fn streak_target(window: Window) -> u32 {
    match window {
        Window::Today => 1,
        Window::D7 => 3,
        Window::D30 => 5,
        Window::M6 => 8,
        Window::AllTime => 10,
    }
}

/// `S` — the minimum gap that must elapse between two counted clean credits for
/// `window`. A clean session inside the gap is clean-but-too-soon and does NOT
/// advance the streak (anti-cram). `Today` has no gap (S = 0). Pure config.
pub fn credit_spacing(window: Window) -> Duration {
    match window {
        Window::Today => Duration::zero(),
        Window::D7 => Duration::days(1),
        Window::D30 => Duration::days(3),
        Window::M6 => Duration::days(7),
        Window::AllTime => Duration::days(14),
    }
}

/// The evolving state of one habit's clean streak after walking its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreakState {
    /// Counted clean credits so far (time-gated; reset to 0 by any slip).
    pub credits: u32,
    /// The target `K` this streak is measured against (carried for the FACE arc).
    pub k: u32,
    /// `credits >= k`: the habit has earned enough spaced-out clean proof.
    pub fixed: bool,
    /// Timestamp of the most recent COUNTED credit (the gate anchor). `None` after
    /// a slip or before the first credit.
    pub last_credit_at: Option<DateTime<Utc>>,
}

/// THE HEART. Walk `sessions` in ASCENDING time, computing the time-gated clean
/// streak toward `k` with minimum gap `s`:
///
/// * a **slip** (`is_slip == true`) resets `credits = 0` and `last_credit_at = None`;
/// * a **clean** session counts a credit IFF `last_credit_at` is `None` (first
///   credit) OR `ts - last_credit_at >= s` (spaced far enough); then `credits += 1`
///   and `last_credit_at = Some(ts)`;
/// * a clean session that is too soon (`ts - last_credit_at < s`) is clean-but-not-
///   counted — it neither advances `credits` nor moves the anchor.
///
/// `fixed = credits >= k`. The caller is responsible for passing sessions already
/// sorted ascending; [`session_slip_series`] does that from the store.
pub fn streak_state(
    sessions: &[(DateTime<Utc>, bool)],
    k: u32,
    s: Duration,
) -> StreakState {
    let mut credits: u32 = 0;
    let mut last_credit_at: Option<DateTime<Utc>> = None;

    for &(ts, is_slip) in sessions {
        if is_slip {
            // A slip wipes the streak: durability means an unbroken clean run.
            credits = 0;
            last_credit_at = None;
            continue;
        }
        // Clean session: count it only if it is the first credit OR it is spaced at
        // least `s` past the previous counted credit. Otherwise it is too soon and
        // is ignored (anti-cram) — the anchor does NOT move.
        let counts = match last_credit_at {
            None => true,
            Some(prev) => ts - prev >= s,
        };
        if counts {
            credits += 1;
            last_credit_at = Some(ts);
        }
    }

    StreakState {
        credits,
        k,
        fixed: credits >= k,
        last_credit_at,
    }
}

/// Streak progress for one habit, ready to attach to the FACE habits payload so the
/// UI can draw a progress arc (`credits` / `k`) and an implode state (`fixed`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreakProgress {
    pub credits: u32,
    pub k: u32,
    pub fixed: bool,
    /// ISO-8601 (RFC3339) of the last counted credit, or `None`.
    pub last_credit_at: Option<String>,
}

impl From<StreakState> for StreakProgress {
    fn from(s: StreakState) -> Self {
        StreakProgress {
            credits: s.credits,
            k: s.k,
            fixed: s.fixed,
            last_credit_at: s.last_credit_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Build the ascending `(session_ts, is_slip)` series for `pattern_id` over the
/// in-window features. A session is a SLIP iff [`session_trips_pattern`] fires on
/// its features (the SAME predicate `detect()` uses → no drift); otherwise it is
/// CLEAN. Sessions with no timestamp are skipped (a credit cannot be time-gated
/// without a clock). The result is sorted ascending by timestamp — ready for
/// [`streak_state`].
pub fn session_slip_series(
    features: &[FeatureVector],
    pattern_id: &str,
) -> Vec<(DateTime<Utc>, bool)> {
    let mut series: Vec<(DateTime<Utc>, bool)> = features
        .iter()
        .filter_map(|fv| fv.started_at.map(|ts| (ts, session_trips_pattern(pattern_id, fv))))
        .collect();
    series.sort_by_key(|(ts, _)| *ts);
    series
}

/// Compute `StreakProgress` for `pattern_id` over the in-window `features`, scaled
/// to `window` (K and S come from the window). The single place that ties the
/// per-session clean/slip series to the streak core for a given window.
pub fn streak_progress_for(
    features: &[FeatureVector],
    pattern_id: &str,
    window: Window,
) -> StreakProgress {
    let series = session_slip_series(features, pattern_id);
    let state = streak_state(&series, streak_target(window), credit_spacing(window));
    state.into()
}

/// The window at which a `fixed` habit triggers the DURABLE guardrail erase (not a
/// merely-visual implode). Env-tunable via `WARDEN_HABITS_ERASE_WINDOW` (wire
/// strings `today|7d|30d|6mo|all`, same vocabulary as [`Window::from_str`]); an
/// unknown/absent value degrades to `AllTime` — strong proof only. Mirrors the
/// `WARDEN_*` env-helper convention (see `util.rs`).
pub fn erase_window_from_env() -> Window {
    std::env::var("WARDEN_HABITS_ERASE_WINDOW")
        .ok()
        .map(|s| Window::from_str(&s))
        .unwrap_or(Window::AllTime)
}

/// Erase one resolved habit's guardrail from `target` and record the resolution,
/// IDEMPOTENTLY. Returns `Ok(true)` only when this call actually removed a block
/// (the first time); `Ok(false)` when there was nothing to do — either the
/// resolution was already recorded, or the block was already absent. The pure,
/// target-injected core of [`durable_erase_fixed`] so it is testable against a temp
/// file with no env or clock.
///
/// Two layers of idempotence:
///  1. If `store` already has a recorded resolution for `pattern_id`, skip entirely.
///  2. Otherwise call the Piece-4 [`crate::forge::remove_block_from_target`], which
///     is itself a no-op (`changed == false`, no backup) when the block is absent.
/// The resolution row is recorded whenever the block was present-and-removed; if the
/// block was already gone we still record it so the habit reads "resolved" without
/// re-attempting the file every scan.
pub fn erase_one(
    store: &Store,
    pattern_id: &str,
    target: &Path,
    now: DateTime<Utc>,
) -> Result<bool> {
    // Layer 1: already resolved → nothing to do (the common steady state).
    if store.habit_resolution(pattern_id)?.is_some() {
        return Ok(false);
    }

    // Back the erase up beside the target, exactly like forge's apply/revert path
    // (`<target dir>/.warden-bak/<pattern>.implode.bak`) so the removal is as
    // reversible as a write.
    let backup_path = crate::util::backup_dir(target).join(format!("{pattern_id}.implode.bak"));
    let outcome = crate::forge::remove_block_from_target(target, pattern_id, &backup_path)?;

    // Record the resolution once (whether we removed a present block or it was
    // already absent) so the UI marks it resolved and we never re-scan the file.
    store.record_habit_resolution(
        pattern_id,
        now,
        &target.to_string_lossy(),
    )?;
    Ok(outcome.changed)
}

/// DURABLE guardrail erase, gated on STRONG proof. If `window` is the erase window
/// ([`erase_window_from_env`], default `AllTime`), every `fixed` issue in `issues`
/// has its CLAUDE.md guardrail erased via [`erase_one`] (which resolves nothing on
/// its own — the target is forge-resolved here so the erase hits the exact file
/// apply wrote to). On any OTHER window this is a no-op: short-window fixes are
/// visual-only implodes in the habits view, never durable writes.
///
/// NOT permanent: a later slip re-flags the pattern; the resolution is cleared by
/// [`clear_resolutions_for_active`] and the normal apply flow re-adds the guardrail.
/// Returns the number of guardrails actually erased this call (0 when re-run —
/// idempotent). Best-effort per pattern: one erase failing is logged, not fatal.
pub fn durable_erase_fixed(
    store: &Store,
    window: Window,
    issues: &[crate::commands::OrbIssue],
    now: DateTime<Utc>,
) -> usize {
    if window != erase_window_from_env() {
        return 0; // short-window fixes implode visually only.
    }
    let mut erased = 0usize;
    for issue in issues.iter().filter(|i| i.fixed) {
        let target = crate::forge::target_for_pattern(&issue.pattern_id);
        match erase_one(store, &issue.pattern_id, &target, now) {
            Ok(true) => erased += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(
                error = ?e,
                pattern = %issue.pattern_id,
                "durable habit erase failed"
            ),
        }
    }
    erased
}

/// Clear any recorded resolution whose pattern is once again being raised — a slip
/// re-flagged it, so it is no longer resolved and the guardrail should come back via
/// the normal apply flow. `active_patterns` is the set of pattern ids currently
/// raised (e.g. from the latest scan's issues). Returns how many resolutions were
/// cleared. Keeps the `habit_resolutions` table honest across the implode↔re-flag
/// cycle without ever touching the user's file (re-adding is apply's job).
pub fn clear_resolutions_for_active(
    store: &Store,
    active_patterns: &std::collections::BTreeSet<String>,
) -> Result<usize> {
    let mut cleared = 0usize;
    for resolved in store.all_habit_resolutions()? {
        if active_patterns.contains(&resolved) {
            store.clear_habit_resolution(&resolved)?;
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Attach streak progress to each windowed `issue`, keyed by `pattern_id`, scaled to
/// `window`. Computes the clean/slip series for each raised pattern from the
/// in-window `features` and writes `credits`/`streak_k`/`fixed`/`last_credit_at`
/// onto the matching `OrbIssue`s (additive — leaves every other field untouched).
/// This is the bridge from the pure streak core to the FACE habits payload.
pub fn attach_streak_progress(
    issues: &mut [crate::commands::OrbIssue],
    features: &[FeatureVector],
    window: Window,
) {
    // Compute once per distinct pattern (several issues can share a pattern_id
    // across harnesses; the streak is per pattern, harness-independent).
    let mut by_pattern: BTreeMap<String, StreakProgress> = BTreeMap::new();
    for issue in issues.iter() {
        by_pattern
            .entry(issue.pattern_id.clone())
            .or_insert_with(|| streak_progress_for(features, &issue.pattern_id, window));
    }
    for issue in issues.iter_mut() {
        if let Some(p) = by_pattern.get(&issue.pattern_id) {
            issue.credits = p.credits;
            issue.streak_k = p.k;
            issue.fixed = p.fixed;
            issue.last_credit_at = p.last_credit_at.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(day: i64) -> DateTime<Utc> {
        // A fixed anchor + `day` days, so spacing math is obvious in the tests.
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(day)
    }

    // ---- streak_target / credit_spacing: exact table ----

    #[test]
    fn streak_target_matches_spec_table() {
        assert_eq!(streak_target(Window::Today), 1);
        assert_eq!(streak_target(Window::D7), 3);
        assert_eq!(streak_target(Window::D30), 5);
        assert_eq!(streak_target(Window::M6), 8);
        assert_eq!(streak_target(Window::AllTime), 10);
    }

    #[test]
    fn credit_spacing_matches_spec_table() {
        assert_eq!(credit_spacing(Window::Today), Duration::zero());
        assert_eq!(credit_spacing(Window::D7), Duration::days(1));
        assert_eq!(credit_spacing(Window::D30), Duration::days(3));
        assert_eq!(credit_spacing(Window::M6), Duration::days(7));
        assert_eq!(credit_spacing(Window::AllTime), Duration::days(14));
    }

    // ---- streak_state: the heart ----

    #[test]
    fn first_clean_always_counts() {
        let s = streak_state(&[(t(0), false)], 3, Duration::days(1));
        assert_eq!(s.credits, 1);
        assert_eq!(s.last_credit_at, Some(t(0)));
        assert!(!s.fixed);
    }

    #[test]
    fn clean_too_soon_does_not_count() {
        // Second clean only 12h after the first, with S = 1 day → not counted.
        let s = streak_state(
            &[(t(0), false), (t(0) + Duration::hours(12), false)],
            3,
            Duration::days(1),
        );
        assert_eq!(s.credits, 1, "the too-soon second clean must not advance");
        assert_eq!(
            s.last_credit_at,
            Some(t(0)),
            "the anchor must NOT move to a non-counting session"
        );
    }

    #[test]
    fn clean_exactly_at_spacing_counts() {
        // Boundary: exactly S apart counts (>=, not >).
        let s = streak_state(
            &[(t(0), false), (t(1), false)],
            3,
            Duration::days(1),
        );
        assert_eq!(s.credits, 2);
        assert_eq!(s.last_credit_at, Some(t(1)));
    }

    #[test]
    fn a_slip_resets_to_zero() {
        let s = streak_state(
            &[(t(0), false), (t(1), false), (t(2), true)],
            3,
            Duration::days(1),
        );
        assert_eq!(s.credits, 0, "a slip wipes the streak");
        assert_eq!(s.last_credit_at, None, "a slip clears the anchor");
        assert!(!s.fixed);
    }

    #[test]
    fn reaching_k_sets_fixed() {
        // 3 clean sessions, each 1 day apart, K=3, S=1 day → fixed.
        let s = streak_state(
            &[(t(0), false), (t(1), false), (t(2), false)],
            3,
            Duration::days(1),
        );
        assert_eq!(s.credits, 3);
        assert!(s.fixed, "K properly-spaced clean sessions must fix the habit");
    }

    /// CORE ANTI-CRAM TEST: K well-spaced cleans → fixed; but K cleans all crammed
    /// within < S of each other → credits stays 1, NOT fixed.
    #[test]
    fn anti_cram_spacing_is_enforced() {
        let k = 3;
        let s = Duration::days(1);

        // Properly spaced (0d, 1d, 2d) → fixed.
        let spaced = streak_state(
            &[(t(0), false), (t(1), false), (t(2), false)],
            k,
            s,
        );
        assert!(spaced.fixed, "spaced cleans must fix");
        assert_eq!(spaced.credits, 3);

        // Crammed: 3 cleans all within the same afternoon (0h, 1h, 2h) → only the
        // first counts; the rest are clean-but-too-soon.
        let crammed = streak_state(
            &[
                (t(0), false),
                (t(0) + Duration::hours(1), false),
                (t(0) + Duration::hours(2), false),
            ],
            k,
            s,
        );
        assert_eq!(crammed.credits, 1, "cramming must NOT earn extra credits");
        assert!(!crammed.fixed, "you cannot cram K in one afternoon");
    }

    #[test]
    fn slip_midway_then_respaced_streak_recovers() {
        // clean(0) clean(1) slip(2) clean(3) clean(4) clean(5) with K=3,S=1day.
        // After the slip the streak restarts and the three later spaced cleans fix.
        let s = streak_state(
            &[
                (t(0), false),
                (t(1), false),
                (t(2), true),
                (t(3), false),
                (t(4), false),
                (t(5), false),
            ],
            3,
            Duration::days(1),
        );
        assert_eq!(s.credits, 3);
        assert!(s.fixed);
        assert_eq!(s.last_credit_at, Some(t(5)));
    }

    #[test]
    fn today_zero_spacing_counts_every_clean() {
        // S = 0 (Today): even same-instant cleans all count; K=1 fixes immediately.
        let s = streak_state(&[(t(0), false)], 1, Duration::zero());
        assert_eq!(s.credits, 1);
        assert!(s.fixed);

        let many = streak_state(
            &[(t(0), false), (t(0), false), (t(0), false)],
            1,
            Duration::zero(),
        );
        assert_eq!(many.credits, 3, "S=0 counts every clean regardless of gap");
        assert!(many.fixed);
    }

    #[test]
    fn empty_series_is_not_fixed() {
        let s = streak_state(&[], 1, Duration::zero());
        assert_eq!(s.credits, 0);
        assert!(!s.fixed);
        assert_eq!(s.last_credit_at, None);
    }

    // ---- session_slip_series: labeling via the shared predicate ----

    #[test]
    fn slip_series_labels_and_sorts_by_time() {
        // Three sessions out of order; UNVERIFIED_COMPLETION trips on tool>=4 & !verified.
        let f_slip = FeatureVector {
            session_id: "slip".into(),
            started_at: Some(t(2)),
            tool_call_count: 6,
            verification_present: false,
            ..Default::default()
        };
        let f_clean = FeatureVector {
            session_id: "clean".into(),
            started_at: Some(t(0)),
            tool_call_count: 6,
            verification_present: true, // verified → CLEAN
            ..Default::default()
        };
        let f_noclock = FeatureVector {
            session_id: "noclock".into(),
            started_at: None, // skipped (cannot time-gate)
            tool_call_count: 6,
            verification_present: false,
            ..Default::default()
        };
        let series = session_slip_series(
            &[f_slip, f_clean, f_noclock],
            "UNVERIFIED_COMPLETION",
        );
        assert_eq!(series.len(), 2, "the no-timestamp session is skipped");
        assert_eq!(series[0], (t(0), false), "clean, earliest first");
        assert_eq!(series[1], (t(2), true), "slip, later");
    }

    #[test]
    fn streak_progress_for_round_trips_to_wire_shape() {
        // One clean (verified) session at t(0): AllTime needs K=10 → not fixed; the
        // wire shape carries the RFC3339 last-credit stamp.
        let f = FeatureVector {
            session_id: "s".into(),
            started_at: Some(t(0)),
            tool_call_count: 6,
            verification_present: true,
            ..Default::default()
        };
        let p = streak_progress_for(&[f], "UNVERIFIED_COMPLETION", Window::AllTime);
        assert_eq!(p.credits, 1);
        assert_eq!(p.k, 10);
        assert!(!p.fixed);
        assert_eq!(p.last_credit_at, Some(t(0).to_rfc3339()));
    }

    // ---- durable erase: gated on AllTime, idempotent ----

    fn write_target_with_guardrail(dir: &Path, pattern: &str) -> std::path::PathBuf {
        // Reproduce a CLAUDE.md that already carries the pattern's guardrail block,
        // so the erase has something real to remove.
        let target = dir.join("CLAUDE.md");
        let block = crate::forge::block_for_finding(&crate::ir::Finding {
            id: "f".into(),
            pattern_id: pattern.into(),
            title: "t".into(),
            severity: 5,
            frequency: 1.0,
            est_cost_tokens: 0,
            est_cost_minutes: 0,
            confidence: 1.0,
            rationale: String::new(),
            evidence: vec![],
            status: "candidate".into(),
            verifier_verdict: None,
        });
        std::fs::write(&target, format!("# Project\n\nkeep me\n{block}")).unwrap();
        target
    }

    #[test]
    fn erase_one_removes_block_then_is_idempotent() {
        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pattern = "UNVERIFIED_COMPLETION";
        let target = write_target_with_guardrail(dir.path(), pattern);
        let before = std::fs::read_to_string(&target).unwrap();
        assert!(before.contains("WARDEN guardrail"), "precondition: block present");

        // First erase removes the block and records the resolution.
        let first = erase_one(&store, pattern, &target, t(0)).unwrap();
        assert!(first, "first erase must remove a present block");
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(!after.contains("WARDEN guardrail"), "block must be gone");
        assert!(after.contains("keep me"), "user content survives");
        assert!(store.habit_resolution(pattern).unwrap().is_some(), "resolution recorded");

        // Second erase is a no-op (resolution already recorded) and leaves the file.
        let second = erase_one(&store, pattern, &target, t(1)).unwrap();
        assert!(!second, "second erase must be a no-op");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            after,
            "idempotent: file unchanged on re-erase"
        );
    }

    #[test]
    fn durable_erase_fires_at_alltime_fixed_not_at_today() {
        // Acquire the crate-wide env lock BEFORE touching any env, so neither the
        // erase-window nor WARDEN_CLAUDE_MD can race a concurrent forge/util test.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_md = std::env::var_os("WARDEN_CLAUDE_MD");
        let prev_win = std::env::var_os("WARDEN_HABITS_ERASE_WINDOW");
        std::env::remove_var("WARDEN_HABITS_ERASE_WINDOW"); // default → AllTime

        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pattern = "UNVERIFIED_COMPLETION";
        let target = write_target_with_guardrail(dir.path(), pattern);
        // Point forge's resolver at our temp CLAUDE.md so the erase hits it.
        std::env::set_var("WARDEN_CLAUDE_MD", target.to_string_lossy().to_string());

        let mut fixed_issue = sample_issue(pattern);
        fixed_issue.fixed = true;

        // Today-fixed → NO durable erase (visual implode only); file untouched.
        let n_today = durable_erase_fixed(&store, Window::Today, &[fixed_issue.clone()], t(0));
        assert_eq!(n_today, 0, "Today-fixed must not erase");
        assert!(
            std::fs::read_to_string(&target).unwrap().contains("WARDEN guardrail"),
            "Today erase must leave the guardrail in place"
        );
        assert!(store.habit_resolution(pattern).unwrap().is_none());

        // AllTime-fixed → durable erase fires once.
        let n_all = durable_erase_fixed(&store, Window::AllTime, &[fixed_issue.clone()], t(0));
        assert_eq!(n_all, 1, "AllTime-fixed must erase exactly one guardrail");
        assert!(!std::fs::read_to_string(&target).unwrap().contains("WARDEN guardrail"));

        // Re-running AllTime erase is idempotent → 0 more erased.
        let n_again = durable_erase_fixed(&store, Window::AllTime, &[fixed_issue], t(1));
        assert_eq!(n_again, 0, "second AllTime erase is a no-op");

        restore_env("WARDEN_CLAUDE_MD", prev_md);
        restore_env("WARDEN_HABITS_ERASE_WINDOW", prev_win);
    }

    #[test]
    fn durable_erase_skips_unfixed_issues() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_md = std::env::var_os("WARDEN_CLAUDE_MD");
        let prev_win = std::env::var_os("WARDEN_HABITS_ERASE_WINDOW");
        std::env::remove_var("WARDEN_HABITS_ERASE_WINDOW");

        let store = Store::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pattern = "UNVERIFIED_COMPLETION";
        let target = write_target_with_guardrail(dir.path(), pattern);
        std::env::set_var("WARDEN_CLAUDE_MD", target.to_string_lossy().to_string());

        let unfixed = sample_issue(pattern); // fixed defaults to false
        let n = durable_erase_fixed(&store, Window::AllTime, &[unfixed], t(0));
        assert_eq!(n, 0, "an unfixed habit is never erased");
        assert!(std::fs::read_to_string(&target).unwrap().contains("WARDEN guardrail"));

        restore_env("WARDEN_CLAUDE_MD", prev_md);
        restore_env("WARDEN_HABITS_ERASE_WINDOW", prev_win);
    }

    #[test]
    fn erase_window_env_override_and_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("WARDEN_HABITS_ERASE_WINDOW");
        std::env::remove_var("WARDEN_HABITS_ERASE_WINDOW");
        assert_eq!(erase_window_from_env(), Window::AllTime, "default is AllTime");
        std::env::set_var("WARDEN_HABITS_ERASE_WINDOW", "30d");
        assert_eq!(erase_window_from_env(), Window::D30);
        std::env::set_var("WARDEN_HABITS_ERASE_WINDOW", "garbage");
        assert_eq!(erase_window_from_env(), Window::AllTime, "unknown degrades to AllTime");
        restore_env("WARDEN_HABITS_ERASE_WINDOW", prev);
    }

    #[test]
    fn clear_resolutions_for_active_undoes_resolution_on_reflag() {
        let store = Store::memory().unwrap();
        store.record_habit_resolution("UNVERIFIED_COMPLETION", t(0), "/tmp/x").unwrap();
        store.record_habit_resolution("WHACK_A_MOLE", t(0), "/tmp/y").unwrap();

        // Only UNVERIFIED is raised again → its resolution clears, WHACK survives.
        let mut active = std::collections::BTreeSet::new();
        active.insert("UNVERIFIED_COMPLETION".to_string());
        let cleared = clear_resolutions_for_active(&store, &active).unwrap();
        assert_eq!(cleared, 1);
        assert!(store.habit_resolution("UNVERIFIED_COMPLETION").unwrap().is_none());
        assert!(store.habit_resolution("WHACK_A_MOLE").unwrap().is_some());
    }

    // --- test helpers ---

    // Serialize tests that mutate process-wide env (WARDEN_CLAUDE_MD /
    // WARDEN_HABITS_ERASE_WINDOW) on the SAME crate-wide lock the forge command
    // tests use, so a habits test pointing WARDEN_CLAUDE_MD at its temp file can
    // never race a concurrent forge apply that reads the same var.
    use crate::util::TEST_ENV_LOCK as ENV_LOCK;

    /// Restore an env var to a previously-captured value (or remove it if it was
    /// unset), so an env-mutating test leaves the process exactly as it found it.
    fn restore_env(key: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    fn sample_issue(pattern: &str) -> crate::commands::OrbIssue {
        crate::commands::OrbIssue {
            id: format!("claude:{pattern}"),
            agent_id: "claude".into(),
            harness: "claude".into(),
            pattern_id: pattern.into(),
            title: "t".into(),
            count: 1,
            severity: 5,
            rationale: String::new(),
            est_cost_tokens: 0,
            est_cost_minutes: 0,
            frequency: 1.0,
            confidence: 1.0,
            session_ids: vec![],
            evidence: vec![],
            finding_id: None,
            verifier_verdict: None,
            status: None,
            credits: 0,
            streak_k: 0,
            fixed: false,
            last_credit_at: None,
        }
    }
}
