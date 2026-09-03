//! Agent identity + naming + subagent-termination decisions.
//!
//! Pure helpers that turn a session's metadata/transcript into a display label,
//! identity quad `(label, nickname, role, origin)`, and a deterministic "this
//! subagent has terminated" verdict.

use crate::ir::{Event, Harness, Session};
use chrono::{DateTime, Utc};

/// The agent's originating task: its first non-meta user prompt, truncated to a
/// name-sized string. `None` when the session has no real user prompt yet (so the
/// label falls back to the folder). Skips `is_meta` prompts (system/tool-injected).
pub(crate) fn first_task(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> Option<String> {
    events.iter().find_map(|(_, e)| match &e.event {
        Event::UserPrompt { text, is_meta, .. } if !is_meta && !text.trim().is_empty() => {
            let name = clean_task_label(text);
            (!name.is_empty()).then_some(name)
        }
        _ => None,
    })
}

/// Clean a raw user prompt into a globe-sized agent name: collapse ALL whitespace
/// (a multi-line prompt becomes one line), drop a leading `@file`/URL token when real
/// text follows (so the name is WHAT the agent is doing, not an attachment path), and
/// truncate to a name-sized length. Returns "" only for empty/whitespace input.
fn clean_task_label(raw: &str) -> String {
    // Collapse newlines/tabs/runs-of-spaces into single spaces so a multi-line prompt
    // renders as one clean name.
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    // Drop a leading attachment/URL token when real text follows it, so the name is
    // the task — not a pasted path or link. Only the first token, only if text remains.
    let cleaned = match collapsed.split_once(' ') {
        Some((head, rest)) if is_noise_token(head) && !rest.trim().is_empty() => rest.trim(),
        _ => collapsed.as_str(),
    };
    crate::util::truncate_chars(cleaned, 60)
}

/// The radar display label.
///
/// `title` is the harness's OWN name for the session and wins whenever it exists: the
/// operator's `custom-title`, else the auto-generated `ai-title` / Codex thread name
/// (ranked in `super::agent::session_title`). It is what the operator actually recognises
/// ("build frontier website"), and without it two agents in one repo read as "WARDEN" and
/// "WARDEN ②" even when the harness has already named them distinctly.
///
/// Otherwise roots are named by their project folder and subagents by a per-parent
/// ordinal ("subagent N"). `root_dup_ordinal` is `Some(n)` only when several live roots
/// share `cwd_basename`: `n == 1` (the oldest) keeps the bare name, `n >= 2` gets a
/// circled disambiguator. `fallback` is the identity-derived label, used only for a root
/// with no project folder.
pub(crate) fn display_label(
    depth: u32,
    title: Option<&str>,
    cwd_basename: Option<&str>,
    subagent_ordinal: Option<u32>,
    root_dup_ordinal: Option<u32>,
    fallback: &str,
) -> String {
    // Subagents keep their positional naming: a title record belongs to the SESSION, and
    // a subagent sharing its parent's transcript title would make every child read as the
    // parent. Their real names come from the team roster / sidecar description upstream.
    if depth >= 1 {
        return format!("subagent {}", subagent_ordinal.unwrap_or(1));
    }
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        return title.to_string();
    }
    match cwd_basename {
        Some(name) if !name.is_empty() => match root_dup_ordinal {
            Some(n) if n >= 2 => format!("{name} {}", circled(n)),
            _ => name.to_string(),
        },
        _ => fallback.to_string(),
    }
}

/// Circled-number glyph for 2..=20 (② = U+2461 = U+2460 + (n-1)), else " (n)".
fn circled(n: u32) -> String {
    if (2..=20).contains(&n) {
        char::from_u32(0x2460 + (n - 1))
            .map(|c| c.to_string())
            .unwrap_or_else(|| format!("({n})"))
    } else {
        format!("({n})")
    }
}

/// When a subagent became terminated, or `None` if still live.
/// Primary: the parent logged an explicit COMPLETION for the subagent's `tool_use_id`
/// (see [`parent_completion_at`]) → terminated at that timestamp (a permanent transcript
/// fact ⇒ idempotent across recomputes). Backstop: no completion, but the subagent has
/// been silent longer than `terminate_ms` while its parent is alive → terminated at
/// `last + terminate_ms`.
pub(crate) fn subagent_terminated_at(
    tool_use_id: Option<&str>,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    child_last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    terminate_ms: u64,
) -> Option<DateTime<Utc>> {
    if let Some(ts) = tool_use_id.and_then(|tid| parent_completion_at(tid, parent_events)) {
        return Some(ts);
    }
    let last = child_last_activity?;
    let quiet_ms = now.signed_duration_since(last).num_milliseconds().max(0) as u64;
    (quiet_ms > terminate_ms).then(|| last + chrono::Duration::milliseconds(terminate_ms as i64))
}

/// The explicit COMPLETION the parent logged for a subagent dispatch `tool_use_id`, or
/// `None` if it has logged neither yet (the dispatch is still open). Two shapes, newest
/// wins: a completed `task-notification` naming the call (a background/async agent, whose
/// immediate tool-result is only the launch ack), else a tool-result for the call (a
/// foreground dispatch returns its result there).
///
/// This carries NO file-silence backstop. That timer is the caller's separate policy and
/// is wrong for an agent that legitimately goes quiet while still assigned (an in-process
/// teammate between turns), so the pure "the parent said this call is done" signal is
/// kept on its own for callers that must not fall back to silence.
pub(crate) fn parent_completion_at(
    tool_use_id: &str,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Option<DateTime<Utc>> {
    if let Some(ts) = parent_events
        .iter()
        .filter_map(|(_, e)| match &e.event {
            Event::UserPrompt { text, .. } if task_notification_completed_for(text, tool_use_id) => {
                Some(e.ts)
            }
            _ => None,
        })
        .max()
    {
        return Some(ts);
    }
    parent_events
        .iter()
        .filter_map(|(_, e)| match &e.event {
            Event::ToolResult {
                call_id, summary, ..
            } if call_id == tool_use_id && !is_async_agent_launch_summary(summary.as_deref()) => {
                Some(e.ts)
            }
            _ => None,
        })
        .max()
}

/// When every `Agent`/`Task` dispatch that could have launched a child starting at
/// `child_started_at` had returned, or `None` while any of them is still open.
///
/// The fast path for a subagent carrying no `toolUseId`. Without one there is nothing
/// to match against the parent, so the only rule left was the file-silence backstop:
/// wait 90 seconds of no writes and call it finished. That is the whole of the "WARDEN
/// is late to notice an agent ended" complaint for this class of subagent, and the wait
/// is not needed, because the parent states the answer directly. A subagent exists
/// because the parent made a dispatch call, and the parent logs that call's result when
/// the subagent returns. So if no dispatch the parent made before this child appeared is
/// still open, this child is not running either.
///
/// Two deliberate conservatisms, both chosen so this can only ever be late, never wrong:
///
/// * only calls at or before `child_started_at` count. The parent logs the call and the
///   child's transcript appears after it, so a later call belongs to a different child.
/// * ALL of those calls must have returned, and the answer is the LAST of their
///   completions. A parent that fans out three agents at once gives all three the same
///   candidate set, so each waits for the slowest. Seconds of extra patience against a
///   90 second timer, and it removes any chance of retiring a sibling that is still out.
///
/// Completion is [`parent_completion_at`], so an async agent's launch acknowledgement
/// does not count as a return. `None` when the parent made no such call at all: no
/// evidence is not the same as evidence of finishing.
pub(crate) fn dispatches_settled_at(
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    child_started_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let candidates: Vec<&str> = parent_events
        .iter()
        .filter_map(|(_, e)| match &e.event {
            Event::ToolCall { tool, call_id, .. }
                if (tool == "Agent" || tool == "Task") && e.ts <= child_started_at =>
            {
                Some(call_id.as_str())
            }
            _ => None,
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let mut latest: Option<DateTime<Utc>> = None;
    for call_id in candidates {
        let done = parent_completion_at(call_id, parent_events)?;
        latest = Some(latest.map_or(done, |cur: DateTime<Utc>| cur.max(done)));
    }
    latest
}

/// Recover the `Agent`/`Task` tool-call id that dispatched an in-process TEAMMATE, by
/// matching the member's roster `name` to the call's `name` argument in the LEAD's events
/// (newest match wins, so a re-dispatched member keys off its latest run).
///
/// Claude writes a teammate's sidecar WITHOUT a `toolUseId` (only ~1 in 3 sidecars carry
/// one), yet the lead spawned each member through an `Agent` call whose `input.name` is
/// the member name, and logs a result for that call when the member finishes. Recovering
/// the id here is what lets a finished teammate retire on its own completion via
/// [`parent_completion_at`] instead of riding the lead's whole lifetime.
pub(crate) fn teammate_dispatch_call_id(
    member_name: &str,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Option<String> {
    if member_name.is_empty() {
        return None;
    }
    parent_events
        .iter()
        .filter_map(|(_, e)| match &e.event {
            Event::ToolCall {
                tool,
                input,
                call_id,
                ..
            } if (tool == "Agent" || tool == "Task")
                && input.get("name").and_then(|v| v.as_str()) == Some(member_name) =>
            {
                Some((e.ts, call_id.clone()))
            }
            _ => None,
        })
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, id)| id)
}

fn task_notification_completed_for(text: &str, tool_use_id: &str) -> bool {
    text.contains("<task-notification>")
        && text.contains(&format!("<tool-use-id>{tool_use_id}</tool-use-id>"))
        && text.contains("<status>completed</status>")
}

fn is_async_agent_launch_summary(summary: Option<&str>) -> bool {
    let Some(summary) = summary else {
        return false;
    };
    summary.contains("Async agent launched successfully")
        || summary.contains("The agent is working in the background")
}

/// A leading prompt token that is an attachment path (`@…`) or a bare URL — noise to
/// drop from an agent name when the real prompt text follows it.
fn is_noise_token(tok: &str) -> bool {
    tok.starts_with('@') || tok.starts_with("http://") || tok.starts_with("https://")
}

/// Identity quad: `(label, nickname, role, origin)`.
/// * Claude subagent → label = its sidecar `description`, role = its `agentType`
///   (persisted onto the child `meta` when the parent linkage is recorded);
/// * Claude root → label = its originating `task` (so several live sessions in the
///   same repo are differentiated by WHAT each is doing), falling back to cwd basename;
/// * Codex → nickname/role/origin from `session_meta`; label = nickname when set;
/// * final fallback for any harness = cwd basename → nickname → external id.
pub(crate) fn identity(
    s: &Session,
    task: Option<String>,
) -> (String, Option<String>, Option<String>, Option<String>) {
    let nickname = s
        .meta
        .get("agent_nickname")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // Which SURFACE this agent is being driven from. Codex records it as `originator`
    // ("Codex Desktop", "codex_vscode"); Claude records the same idea per envelope as
    // `entrypoint` (`cli`, `claude-vscode`, `claude-desktop`, `sdk-*`), which the adapter
    // lifts onto meta. Both land here so the face can say terminal vs IDE panel with one
    // field, whichever harness it is. Values pass through verbatim rather than being
    // mapped to a house vocabulary: the harness's own word is the honest one.
    let origin = s
        .meta
        .get("originator")
        .or_else(|| s.meta.get("entrypoint"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // A Claude subagent carries its sidecar `description`/`agentType` on its meta
    // (written when the parent link is persisted). When present they win the label
    // and role — these keys never appear on a Codex session or a root.
    let claude_description = s
        .meta
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    let claude_agent_type = s
        .meta
        .get("agentType")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);

    // Role: Claude subagent `agentType`, else the Codex `agent_role`.
    let role = claude_agent_type.clone().or_else(|| {
        s.meta
            .get("agent_role")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });

    // Task-first naming for Claude: a session in a shared repo is named by WHAT it
    // is doing (its originating prompt), not just the folder — so several live
    // sessions in the same cwd are differentiated. Codex keeps its existing
    // nickname/cwd naming (its sessions already carry good `session_meta` names).
    let task_label = if matches!(s.harness, Harness::Codex) {
        None
    } else {
        task
    };

    // Label precedence: Claude subagent description → Claude root task → cwd basename
    // → Codex nickname → external id.
    let label = claude_description
        .or(task_label)
        .or_else(|| {
            s.project
                .as_ref()
                .and_then(|p| p.cwd.file_name())
                .map(|n| n.to_string_lossy().to_string())
        })
        .or_else(|| nickname.clone())
        .unwrap_or_else(|| s.external_id.clone());

    (label, nickname, role, origin)
}

#[cfg(test)]
mod naming_tests {
    use super::clean_task_label;

    #[test]
    fn collapses_internal_whitespace_and_newlines() {
        assert_eq!(
            clean_task_label("  fix   the\n\nradar  glow "),
            "fix the radar glow"
        );
    }

    #[test]
    fn strips_leading_at_file_mention() {
        assert_eq!(
            clean_task_label("@/Users/k/Desktop/MOBIUS-intro.mp4 turn this into a launch video"),
            "turn this into a launch video"
        );
    }

    #[test]
    fn strips_leading_quoted_at_file_mention() {
        assert_eq!(
            clean_task_label("@\"/Users/k/clip.mp4\" This video needs captions"),
            "This video needs captions"
        );
    }

    #[test]
    fn strips_leading_bare_url() {
        assert_eq!(
            clean_task_label("https://github.com/foo/bar Can you review this repo"),
            "Can you review this repo"
        );
    }

    #[test]
    fn keeps_leading_token_when_it_is_the_whole_prompt() {
        // Nothing meaningful follows → keep the original rather than an empty name.
        assert_eq!(
            clean_task_label("@/Users/k/only-a-path.txt"),
            "@/Users/k/only-a-path.txt"
        );
    }

    #[test]
    fn truncates_to_name_size_with_ellipsis() {
        let long = "design a comprehensive multi agent orchestration radar with glow and tethers and side panels";
        let out = clean_task_label(long);
        assert!(
            out.chars().count() <= 60,
            "got {} chars: {out:?}",
            out.chars().count()
        );
        assert!(
            out.ends_with('…'),
            "long label should be ellipsized: {out:?}"
        );
    }

    #[test]
    fn empty_or_whitespace_is_empty() {
        assert_eq!(clean_task_label("   \n  "), "");
    }
}
