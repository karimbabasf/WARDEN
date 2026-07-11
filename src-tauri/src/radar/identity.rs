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

/// The radar display label. Roots are named by their project folder; subagents by a
/// per-parent ordinal ("subagent N"). `root_dup_ordinal` is `Some(n)` only when
/// several live roots share `cwd_basename` — `n == 1` (the oldest) keeps the bare
/// name, `n >= 2` gets a circled disambiguator. `fallback` is the identity-derived
/// label, used only for a root with no project folder.
pub(crate) fn display_label(
    depth: u32,
    cwd_basename: Option<&str>,
    subagent_ordinal: Option<u32>,
    root_dup_ordinal: Option<u32>,
    fallback: &str,
) -> String {
    if depth >= 1 {
        return format!("subagent {}", subagent_ordinal.unwrap_or(1));
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
/// Primary: the parent logged a tool RESULT for the subagent's `tool_use_id` →
/// terminated at the result's timestamp (a permanent transcript fact ⇒ idempotent
/// across recomputes). Backstop: no result, but the subagent has been silent longer
/// than `terminate_ms` while its parent is alive → terminated at `last + terminate_ms`.
pub(crate) fn subagent_terminated_at(
    tool_use_id: Option<&str>,
    parent_events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    child_last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    terminate_ms: u64,
) -> Option<DateTime<Utc>> {
    if let Some(tid) = tool_use_id {
        if let Some(ts) = parent_events
            .iter()
            .filter_map(|(_, e)| match &e.event {
                Event::UserPrompt { text, .. } if task_notification_completed_for(text, tid) => {
                    Some(e.ts)
                }
                _ => None,
            })
            .max()
        {
            return Some(ts);
        }
        if let Some(ts) = parent_events
            .iter()
            .filter_map(|(_, e)| match &e.event {
                Event::ToolResult {
                    call_id, summary, ..
                } if call_id == tid && !is_async_agent_launch_summary(summary.as_deref()) => {
                    Some(e.ts)
                }
                _ => None,
            })
            .max()
        {
            return Some(ts);
        }
    }
    let last = child_last_activity?;
    let quiet_ms = now.signed_duration_since(last).num_milliseconds().max(0) as u64;
    (quiet_ms > terminate_ms).then(|| last + chrono::Duration::milliseconds(terminate_ms as i64))
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
    let origin = s
        .meta
        .get("originator")
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
