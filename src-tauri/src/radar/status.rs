//! Agent status determination: resolve a session's working/idle/terminated verdict
//! from its conversation state (its last ingested events), falling back to transcript
//! mtime only when there are no usable events. Built on top of [`super::liveness`]'s
//! pure liveness primitives.

use super::liveness::{self, AgentStatus};
use crate::ir::{Event, EventRecord, Harness, Session};
use crate::store::Store;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Status for one session: a Claude session uses the registry partition (by external
/// id); anything else (Codex, etc., and Claude rows not in the live registry — e.g.
/// subagents, which have no PID) derives working/idle from its CONVERSATION STATE
/// (last ingested event), falling back to mtime only when it has no usable events.
/// The live collector treats store-resident sessions as open/idle and lets the
/// watcher's recompute drop a session that has left the live set.
pub(crate) fn agent_status(
    store: &Store,
    s: &Session,
    claude_status: &HashMap<String, AgentStatus>,
    mtime_secs_ago: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
) -> AgentStatus {
    if let Some(st) = claude_status.get(&s.external_id) {
        return *st;
    }
    // FAULT B: conversation-state first (deterministic), mtime only as a last resort.
    let stale_secs = crate::util::radar_working_stale_secs();
    let working_secs = crate::util::radar_working_ms() / 1000;
    let events = store.session_events(&s.id).unwrap_or_default();
    if matches!(s.harness, Harness::Codex) {
        let has_uningested_tail = source_has_uningested_tail(store, s);
        if let Some(st) =
            codex_status_from_last_event(&events, now, stale_secs, has_uningested_tail)
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
    if let Some(st) = liveness::status_from_last_event(&events, now, stale_secs) {
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
            liveness::status_from_last_event(&events, now, stale_secs).map(|st| (last_ts, st))
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
