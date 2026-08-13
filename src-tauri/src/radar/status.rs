//! Agent status determination: resolve a session's working/awaiting/idle/terminated
//! verdict from its conversation state (its last ingested events), falling back to
//! transcript mtime only when there are no usable events. Built on top of
//! [`super::liveness`]'s pure liveness primitives and [`super::awaiting`]'s pure
//! "is it stopped on the operator?" rules.

use super::awaiting::{self, AwaitingReason};
use super::liveness::{self, AgentStatus};
use crate::ir::{Event, EventRecord, Harness, Session};
use crate::store::Store;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// A session's status plus, when it is [`AgentStatus::Awaiting`], WHY it is waiting.
///
/// The reason is carried alongside rather than inside the enum because `AgentStatus` is
/// `Copy` and compared all over the collector; keeping it a separate field means every
/// existing `matches!(st, AgentStatus::Idle)` stays valid and nothing has to learn about
/// reasons it does not care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusVerdict {
    pub status: AgentStatus,
    /// `Some` only when `status == Awaiting`.
    pub awaiting: Option<AwaitingReason>,
}

impl StatusVerdict {
    fn plain(status: AgentStatus) -> Self {
        Self {
            status,
            awaiting: None,
        }
    }
    fn awaiting(reason: AwaitingReason) -> Self {
        Self {
            status: AgentStatus::Awaiting,
            awaiting: Some(reason),
        }
    }
    /// A finished subagent, decided by the caller's termination rules rather than by the
    /// conversation state. It is leaving the board, so it never carries a reason.
    pub(crate) fn terminated() -> Self {
        Self::plain(AgentStatus::Terminated)
    }
}

/// Status for one session, without the awaiting reason. Thin wrapper over
/// [`agent_verdict`] for the callers that only branch on working vs quiet.
pub(crate) fn agent_status(
    store: &Store,
    s: &Session,
    claude_status: &HashMap<String, AgentStatus>,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
) -> AgentStatus {
    agent_verdict(store, s, claude_status, &HashMap::new(), mtime_secs_ago, now).status
}

/// Full verdict for one session: a Claude session uses the registry partition (by
/// external id); anything else (Codex, etc., and Claude rows not in the live registry,
/// e.g. subagents, which have no PID) derives working/idle from its CONVERSATION STATE
/// (last ingested event), falling back to mtime only when it has no usable events.
/// The live collector treats store-resident sessions as open/idle and lets the
/// watcher's recompute drop a session that has left the live set.
///
/// `claude_awaiting` carries the registry's own `waitingFor` reason, already folded into
/// the closed vocabulary, keyed by external id.
///
/// AWAITING sits ON TOP of that pipeline rather than inside it, because the two things
/// it detects arrive from opposite directions:
/// * the harness's registry already says `waiting`, in which case there is nothing to
///   derive and the reason comes with it;
/// * or nothing structured says anything, and the only evidence is the shape of the
///   transcript tail (a blocking tool still in flight, or a completed turn that ends on
///   a question). See [`super::awaiting`] for why each of those is a real signal.
///
/// A registry `busy` is never second-guessed: the agent is generating, and a question it
/// wrote three seconds ago has already been superseded by the work that followed it.
pub(crate) fn agent_verdict(
    store: &Store,
    s: &Session,
    claude_status: &HashMap<String, AgentStatus>,
    claude_awaiting: &HashMap<String, AwaitingReason>,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
) -> StatusVerdict {
    // Claude's live registry is the harness's OWN statement about the session (its
    // `status` field is updated even mid-generation and during a long tool run), so it is
    // never second-guessed here. Where that registry entry had no verdict to give, the
    // conversation-state fallback it delegated to has already applied the in-flight
    // promotion itself (see `claude_conversation_status`).
    if let Some(st) = claude_status.get(&s.external_id) {
        return match st {
            // The harness says it is blocked, and normally says on what. No reason in the
            // map means this Awaiting came from the conversation-state fallback (older
            // Claude, no `status` field), so the transcript is where the reason is. If
            // even that is silent it is still honestly blocked: degrade to "input needed"
            // rather than inventing a question.
            AgentStatus::Awaiting => StatusVerdict::awaiting(
                claude_awaiting
                    .get(&s.external_id)
                    .copied()
                    .or_else(|| {
                        let events = store.session_events(&s.id).unwrap_or_default();
                        derive_awaiting_reason(&events, now)
                    })
                    .unwrap_or(AwaitingReason::Input),
            ),
            // Quiet by the registry's reckoning, which only ever tracks its own DIALOGS.
            // An agent that asked in plain prose and stopped closed its turn normally, so
            // the harness calls it idle and only the transcript knows better. This is the
            // "sometimes it asks in the output it serves" case, and it costs one events
            // read per idle root (the working ones return above and never pay it).
            AgentStatus::Idle => {
                let events = store.session_events(&s.id).unwrap_or_default();
                upgrade_quiet_to_awaiting(AgentStatus::Idle, &events, now)
            }
            other => StatusVerdict::plain(*other),
        };
    }
    let events = store.session_events(&s.id).unwrap_or_default();
    let base = base_agent_status(store, s, mtime_secs_ago, now, &events);
    let promoted = promote_if_mid_tool_call(base, &events, now);
    upgrade_quiet_to_awaiting(promoted, &events, now)
}

/// Turn a WORKING-or-QUIET verdict into `Awaiting` when the transcript shows the agent
/// stopped on the operator. Never touches `Closed`/`Terminated`: those are verdicts
/// about the agent EXISTING, and a dangling question there just means it died mid-ask.
///
/// The two rules rank differently on purpose:
/// * an unresolved BLOCKING tool call outranks `Working`, because that is exactly what a
///   `Working` verdict looks like from the outside (the call is the newest event and no
///   result has landed) while the truth is a dialog on screen;
/// * a question in prose only upgrades a QUIET verdict, because a working agent's older
///   question has already been overtaken by whatever it did next.
fn upgrade_quiet_to_awaiting(
    base: AgentStatus,
    events: &[(crate::ir::Turn, EventRecord)],
    now: DateTime<Utc>,
) -> StatusVerdict {
    if matches!(base, AgentStatus::Closed | AgentStatus::Terminated) {
        return StatusVerdict::plain(base);
    }
    if let Some(reason) = awaiting::blocking_call_in_flight(events, now) {
        return StatusVerdict::awaiting(reason);
    }
    if matches!(base, AgentStatus::Working) {
        return StatusVerdict::plain(base);
    }
    match awaiting::finished_turn_asks_question(events) {
        true => StatusVerdict::awaiting(AwaitingReason::Question),
        false => StatusVerdict::plain(base),
    }
}

/// The awaiting reason the TRANSCRIPT supports, for an agent already known to be
/// waiting. `None` when the transcript shows no reason at all, which is the honest
/// answer when the harness reported `waiting` for a dialog that leaves no record.
fn derive_awaiting_reason(
    events: &[(crate::ir::Turn, EventRecord)],
    now: DateTime<Utc>,
) -> Option<AwaitingReason> {
    awaiting::blocking_call_in_flight(events, now).or_else(|| {
        awaiting::finished_turn_asks_question(events).then_some(AwaitingReason::Question)
    })
}

/// A tool call the harness STARTED and has not finished is direct evidence the agent is
/// mid-step, so it promotes an otherwise-idle verdict to `Working`.
///
/// This inverts the old order, which gated the in-flight call behind the status. Every
/// status rule below decides from the age of the LAST ingested event against a staleness
/// window (default 180s), and while a long `Read`, `Edit`, or build is running the tool
/// call IS the last line in the transcript: nothing else arrives until it returns. So a
/// slow call aged the session into `Idle` and took the live action off the panel with it,
/// which is the "does not catch edits or reads being made live" complaint. A started call
/// with no end is a fact, not a timer, and it now outranks the timer.
///
/// `Closed` and `Terminated` are never promoted: those are verdicts about the agent
/// EXISTING (a dead PID, a subagent whose parent logged its result), and a dangling call
/// there just means the session died mid-call.
fn promote_if_mid_tool_call(
    base: AgentStatus,
    events: &[(crate::ir::Turn, EventRecord)],
    now: DateTime<Utc>,
) -> AgentStatus {
    if !matches!(base, AgentStatus::Idle) {
        return base;
    }
    match super::agent::in_flight_tool_call(events, now) {
        Some(_) => AgentStatus::Working,
        None => base,
    }
}

fn base_agent_status(
    store: &Store,
    s: &Session,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
    events: &[(crate::ir::Turn, EventRecord)],
) -> AgentStatus {
    // FAULT B: conversation-state first (deterministic), mtime only as a last resort.
    let stale_secs = crate::util::radar_working_stale_secs();
    let working_secs = crate::util::radar_working_ms() / 1000;
    if matches!(s.harness, Harness::Codex) {
        let has_uningested_tail = source_has_uningested_tail(store, s);
        if let Some(st) = codex_status_from_last_event(events, now, stale_secs, has_uningested_tail)
        {
            return st;
        }
        if has_uningested_tail {
            return match mtime_secs_ago(&s.external_id) {
                Some(secs) if secs < working_secs => AgentStatus::Working,
                _ => AgentStatus::Idle,
            };
        }
    }
    if let Some(st) = liveness::status_from_last_event(events, now, stale_secs) {
        return st;
    }
    // No usable events at all → fall back to the old transcript-mtime heuristic.
    match mtime_secs_ago(&s.external_id) {
        Some(secs) if secs < working_secs => AgentStatus::Working,
        _ => AgentStatus::Idle,
    }
}

fn codex_status_from_last_event(
    events: &[(crate::ir::Turn, EventRecord)],
    now: DateTime<Utc>,
    stale_secs: u64,
    has_uningested_tail: bool,
) -> Option<AgentStatus> {
    let last = latest_codex_liveness_event(events)?;
    let fresh = codex_liveness_event_is_fresh(last, now, stale_secs);
    if matches!(last.event, Event::AssistantText { .. })
        || matches!(&last.event, Event::UserPrompt { text, .. } if is_completed_task_notification(text))
    {
        return Some(if has_uningested_tail && fresh {
            AgentStatus::Working
        } else {
            AgentStatus::Idle
        });
    }
    Some(if fresh {
        AgentStatus::Working
    } else {
        AgentStatus::Idle
    })
}

/// The timestamp at which a Codex session's task last COMPLETED, if that completion is
/// its most recent activity. Returns `Some(ts)` only when the newest action-or-
/// completion event is a `task_complete` marker (nothing ran after it). A later action
/// (a new orchestrator message, a tool call, a follow-up turn) means the agent resumed,
/// and this yields `None`. This is the honest "the task is done" signal: it fires on the
/// real `task_complete` record, so it is not fooled by a mid-task commentary message
/// (which is always followed by more action events before the task truly completes).
/// The RADAR uses it to retire a finished Codex subagent instead of leaving it nested.
pub(crate) fn codex_subagent_completed_at(
    events: &[(crate::ir::Turn, EventRecord)],
) -> Option<DateTime<Utc>> {
    let newest = events
        .iter()
        .filter(|(_, e)| {
            codex_liveness_priority(&e.event).is_some() || is_codex_task_complete(&e.event)
        })
        .max_by(|(_, a), (_, b)| a.ts.cmp(&b.ts).then(a.raw_ref.offset.cmp(&b.raw_ref.offset)))
        .map(|(_, e)| e)?;
    is_codex_task_complete(&newest.event).then_some(newest.ts)
}

/// The `task_complete` marker the Codex adapter emits when a task finishes (a bookkeeping
/// `SystemNotice`, so it stays out of the working/idle rule).
fn is_codex_task_complete(event: &Event) -> bool {
    matches!(event, Event::SystemNotice { subtype, .. } if subtype == "codex_task_complete")
}

fn codex_liveness_event_is_fresh(event: &EventRecord, now: DateTime<Utc>, stale_secs: u64) -> bool {
    let age_secs = now.signed_duration_since(event.ts).num_seconds().max(0) as u64;
    age_secs <= stale_secs
}

fn latest_codex_liveness_event(events: &[(crate::ir::Turn, EventRecord)]) -> Option<&EventRecord> {
    events
        .iter()
        .filter(|(_, e)| codex_liveness_priority(&e.event).is_some())
        .max_by(|(_, a), (_, b)| {
            a.ts.cmp(&b.ts)
                .then(a.raw_ref.offset.cmp(&b.raw_ref.offset))
                .then(codex_liveness_priority(&a.event).cmp(&codex_liveness_priority(&b.event)))
                .then(a.id.cmp(&b.id))
        })
        .map(|(_, e)| e)
}

fn codex_liveness_priority(event: &Event) -> Option<u8> {
    match event {
        Event::UserPrompt { .. } => Some(3),
        Event::ToolCall { .. } => Some(3),
        Event::FileSnapshot { .. } => Some(2),
        Event::ToolResult { .. } => Some(2),
        Event::AssistantText { .. } => Some(1),
        _ => None,
    }
}

fn is_completed_task_notification(text: &str) -> bool {
    text.contains("<task-notification>") && text.contains("<status>completed</status>")
}

fn source_has_uningested_tail(store: &Store, s: &Session) -> bool {
    let Ok(watermark) = store.watermark_offset(&s.source_path) else {
        return false;
    };
    std::fs::metadata(&s.source_path)
        .map(|m| m.len() > watermark)
        .unwrap_or(false)
}

/// Decide a Claude session's working/idle status from its CONVERSATION STATE (Fault B):
/// resolve the registry's external `sessionId` → the store row → its last ingested
/// event via [`super::liveness::status_from_last_event`]. Falls back to the
/// transcript-mtime heuristic ONLY when the session has no usable events (or is not in
/// the store). This is deterministic across reads — the property that removes the
/// working↔idle flicker.
pub(crate) fn claude_conversation_status(
    store: &Store,
    sessions: &[Session],
    external_id: &str,
    now: DateTime<Utc>,
    stale_secs: u64,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
) -> AgentStatus {
    // The registry keys by external `sessionId`; the store keys events by the internal
    // id. A long Claude session is re-ingested as SEVERAL store rows sharing one external
    // id (one row per compaction segment), and — contrary to an earlier assumption — the
    // conversational tail is NOT replicated across them: each row holds only its segment's
    // events. So `find()`-first (which, given `sessions()` orders by `started_at DESC`,
    // returns the row with the latest START, not the freshest TAIL) can read a stale
    // segment and mislabel a live agent idle. Evaluate the row whose LAST event is the
    // most recent — that segment carries the agent's true current state.
    let working_secs = crate::util::radar_working_ms() / 1000;
    let freshest = sessions
        .iter()
        .filter(|s| s.external_id == external_id)
        .filter_map(|s| {
            let events = store.session_events(&s.id).unwrap_or_default();
            let last_ts = events
                .iter()
                .rev()
                .find(|(_, e)| {
                    matches!(
                        e.event,
                        Event::UserPrompt { .. }
                            | Event::ToolCall { .. }
                            | Event::ToolResult { .. }
                            | Event::AssistantText { .. }
                            | Event::TokenUsage { .. }
                    )
                })
                .map(|(_, e)| e.ts)?;
            liveness::status_from_last_event(&events, now, stale_secs).map(|st| {
                let promoted = promote_if_mid_tool_call(st, &events, now);
                // Same third state as everywhere else: a session with no registry
                // `status` still has a transcript, and a dangling `AskUserQuestion` or a
                // turn that ended on a question is blocked on the operator whether or not
                // this Claude build reports its dialogs. The REASON is re-derived in
                // `agent_verdict` (this function can only return a bare status).
                (last_ts, upgrade_quiet_to_awaiting(promoted, &events, now).status)
            })
        })
        .max_by_key(|(ts, _)| *ts);
    if let Some((_, st)) = freshest {
        return st;
    }
    // No row / no usable events → old transcript-mtime heuristic (last resort).
    match mtime_secs_ago(external_id) {
        Some(secs) if secs < working_secs => AgentStatus::Working,
        _ => AgentStatus::Idle,
    }
}

/// Seconds since the session's transcript was last modified, by `external_id`.
/// Returns `None` when the session/file is unknown or its mtime is unreadable.
pub(crate) fn transcript_mtime_secs_ago(
    sessions: &[Session],
    external_id: &str,
    now: DateTime<Utc>,
) -> Option<u64> {
    let session = sessions.iter().find(|s| s.external_id == external_id)?;
    let modified = std::fs::metadata(&session.source_path)
        .and_then(|m| m.modified())
        .ok()?;
    let modified: DateTime<Utc> = modified.into();
    let secs = now.signed_duration_since(modified).num_seconds();
    Some(secs.max(0) as u64)
}
