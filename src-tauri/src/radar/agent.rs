//! Per-agent construction: join a stored session's size/composition/labels/activity
//! into a [`RadarAgent`], and tail its events into a recent-activity feed.

use super::composition::{
    self, claude_context_size, codex_context_size, exact_composition, tokenize_len, ContextSize,
};
use super::context::{context_breakdown, est_cost_usd, estimate_for_session};
use super::identity::{first_task, identity};
use super::liveness::AgentStatus;
use super::model::{RadarAction, RadarActivity, RadarAgent, RadarComposition, RadarExact};
use crate::ir::{Event, Harness, Session};
use crate::store::Store;

/// Build one [`RadarAgent`] from a stored session, joining size/composition
/// (Tasks 7/8), labels/identity (per harness), recent activity, and est cost.
pub(crate) fn build_agent(
    store: &Store,
    s: &Session,
    parent_id: Option<String>,
    depth: u32,
    child_count: u32,
    status: AgentStatus,
) -> RadarAgent {
    let events = store.session_events(&s.id).unwrap_or_default();

    // Last TokenUsage drives live occupancy + exact composition.
    let last_usage = events
        .iter()
        .rev()
        .find(|(_, e)| matches!(e.event, Event::TokenUsage { .. }))
        .map(|(_, e)| e.event.clone());
    let model = last_usage
        .as_ref()
        .and_then(|e| match e {
            Event::TokenUsage { model, .. } if !model.trim().is_empty() => Some(model.clone()),
            _ => None,
        })
        .or_else(|| first_non_empty_model_id(s));

    let (base_size, exact) = match &last_usage {
        Some(u) => {
            let m = model.clone().unwrap_or_default();
            let size = match s.harness {
                Harness::Codex => {
                    // Codex resident size = input_tokens; window from the transcript
                    // metadata when present, falling back to the provider/model table.
                    let input = match u {
                        Event::TokenUsage { input, .. } => *input as u64,
                        _ => 0,
                    };
                    codex_context_size(input, codex_context_window(s, &m))
                }
                _ => claude_context_size(u, &m),
            };
            (size, exact_composition(u))
        }
        None => (
            ContextSize {
                context_tokens: 0,
                max_tokens: composition::max_window_for_model(&model.clone().unwrap_or_default()),
                fill_pct: 0.0,
            },
            composition::ExactComposition {
                cache_read: 0,
                fresh: 0,
                output: 0,
            },
        ),
    };
    let pending_tail_tokens = if last_usage.is_some() {
        pending_context_after_latest_usage(&events)
    } else {
        0
    };
    let size = with_pending_context(base_size, pending_tail_tokens);

    let estimated = estimate_for_session(store, s, &events, base_size.context_tokens);
    let context_breakdown = context_breakdown(
        s.harness.clone(),
        size,
        estimated.clone(),
        &events,
        pending_tail_tokens,
    );

    let recent_activity = recent_activity(&events);
    let current_action = current_action(&events, &status, chrono::Utc::now());
    // The harness's own title for this session, recorded by whichever adapter found one.
    let title = s
        .meta
        .get("session_title")
        .and_then(|v| v.as_str())
        .map(|t| crate::util::truncate_chars(t.trim(), 120))
        .filter(|t| !t.is_empty());
    let est_cost_usd = est_cost_usd(&model, &exact);
    let task = first_task(&events);
    let (label, nickname, role, origin) = identity(s, task);
    let cwd = s
        .project
        .as_ref()
        .and_then(|p| p.cwd.file_name())
        .map(|n| n.to_string_lossy().to_string());

    RadarAgent {
        id: s.id.clone(),
        harness: s.harness.as_str().to_string(),
        origin,
        parent_id,
        depth,
        label,
        nickname,
        cwd,
        role,
        model,
        title,
        current_action,
        // Team membership is joined in `assemble`, which reads the roster once per
        // recompute instead of once per agent.
        team: None,
        status: status.as_str().to_string(),
        context_tokens: size.context_tokens,
        max_tokens: size.max_tokens,
        fill_pct: size.fill_pct,
        context_breakdown,
        composition: RadarComposition {
            exact: RadarExact {
                cache_read: exact.cache_read,
                fresh: exact.fresh,
                output: exact.output,
            },
            estimated,
        },
        recent_activity,
        child_count,
        started_at: s.started_at.to_rfc3339(),
        est_cost_usd,
    }
}

fn first_non_empty_model_id(s: &Session) -> Option<String> {
    s.model_ids.iter().find(|m| !m.trim().is_empty()).cloned()
}

fn codex_context_window(s: &Session, model: &str) -> u64 {
    s.meta
        .get("model_context_window")
        .and_then(serde_json::Value::as_u64)
        .filter(|n| *n > 0)
        .unwrap_or_else(|| composition::max_window_for_model(model))
}

fn with_pending_context(mut size: ContextSize, pending_tail_tokens: u64) -> ContextSize {
    if pending_tail_tokens == 0 {
        return size;
    }
    size.context_tokens = size.context_tokens.saturating_add(pending_tail_tokens);
    size.fill_pct = if size.max_tokens == 0 {
        0.0
    } else {
        (size.context_tokens as f64 / size.max_tokens as f64).clamp(0.0, 1.0)
    };
    size
}

fn pending_context_after_latest_usage(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> u64 {
    let Some(last_usage_idx) = events
        .iter()
        .rposition(|(_, e)| matches!(e.event, Event::TokenUsage { .. }))
    else {
        return 0;
    };
    events
        .iter()
        .skip(last_usage_idx + 1)
        .map(|(_, e)| match &e.event {
            Event::UserPrompt { text, .. } | Event::AssistantText { text } => tokenize_len(text),
            Event::Thinking { tokens } => *tokens as u64,
            Event::ToolCall { tool, input, .. } => tokenize_len(&format!("{tool} {input}")),
            Event::ToolResult { bytes, .. } => bytes / 4,
            _ => 0,
        })
        .sum()
}

/// Every action event as a recent-activity row (newest first): a kind glyph-friendly
/// `kind` plus a short label. No cap — the detail panel shows ~10 rows in a
/// scrollable feed and lets you scroll back to the very first action.
pub(crate) fn recent_activity(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Vec<RadarActivity> {
    let mut out: Vec<RadarActivity> = Vec::new();
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by(|(_, a), (_, b)| {
        b.ts.cmp(&a.ts)
            .then(b.raw_ref.offset.cmp(&a.raw_ref.offset))
            .then(b.id.cmp(&a.id))
    });
    for (_, e) in ordered {
        let (kind, label) = match &e.event {
            // The "what is it doing" signal: name the file touched / command run, and
            // classify it (read / write / search / run) so the feed is not a wall of
            // identical "tool" rows.
            Event::ToolCall { tool, input, .. } => match tool_activity(tool, input) {
                Some(kl) => kl,
                // Deduped: a Codex apply_patch is shown as its FileSnapshot write below.
                None => continue,
            },
            // File writes: Codex `patch_apply_end` and Claude edits arrive here as real
            // paths. Previously dropped (no "writing files" ever showed on the radar).
            Event::FileSnapshot { files } => ("write", file_snapshot_label(files)),
            Event::AssistantText { text } => ("message", crate::util::truncate_chars(text, 80)),
            Event::UserPrompt { text, .. } => ("message", crate::util::truncate_chars(text, 80)),
            Event::Thinking { .. } => ("thinking", "thinking".to_string()),
            // ToolResult (and the rest) is not a distinct action; its bare
            // `result <call_id>` row was pure noise, so it is dropped here.
            _ => continue,
        };
        let target = match &e.event {
            Event::ToolCall { tool, input, .. } => {
                tool_call_abs_path(tool, input).as_deref().map(crate::util::display_path)
            }
            // A single-file snapshot has an unambiguous target; a multi-file apply does not.
            Event::FileSnapshot { files } => match files.as_slice() {
                [one] => absolute_or_none(&one.path).as_deref().map(crate::util::display_path),
                _ => None,
            },
            _ => None,
        };
        out.push(RadarActivity {
            ts: e.ts.to_rfc3339(),
            kind: kind.to_string(),
            label,
            target,
        });
    }
    out
}

/// The single file a tool call targets, as an ABSOLUTE path, or `None` when the call
/// touches no file or touches several.
///
/// Both harnesses record absolute paths verbatim: Claude in `input.file_path` (and the
/// `path` / `notebook_path` spellings), Codex in the `*** Update File:` headers of an
/// `apply_patch` DSL body. Neither is joined against the session cwd anywhere upstream,
/// so what is stored is already absolute; a relative path here is a harness quirk we do
/// not guess about, and returning `None` is the honest answer.
pub(crate) fn tool_call_abs_path(tool: &str, input: &serde_json::Value) -> Option<String> {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    if let Some(p) = s("file_path").or_else(|| s("path")).or_else(|| s("notebook_path")) {
        return absolute_or_none(p);
    }
    // Codex `apply_patch` carries raw patch DSL as a string rather than JSON. One header
    // line per file; a multi-file patch has no single target, so it yields None.
    if tool == "apply_patch" || input.get("codex_tool").and_then(|v| v.as_str()) == Some("apply_patch") {
        let body = input
            .as_str()
            .or_else(|| s("patch"))
            .or_else(|| s("input"))
            .unwrap_or_default();
        let mut found: Option<&str> = None;
        for line in body.lines() {
            let line = line.trim();
            for tag in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
                if let Some(rest) = line.strip_prefix(tag) {
                    if found.is_some() {
                        return None; // several files touched: no single target
                    }
                    found = Some(rest.trim());
                }
            }
        }
        return found.and_then(absolute_or_none);
    }
    None
}

/// Keep only absolute paths. A bare basename or a relative fragment cannot be revealed
/// in Finder without guessing a base directory, and a wrong guess opens the wrong file.
fn absolute_or_none(p: &str) -> Option<String> {
    let p = p.trim();
    (!p.is_empty() && p.starts_with('/')).then(|| p.to_string())
}

/// How long an unresolved tool call may sit before the radar stops calling it current.
/// Generous enough for a genuinely slow build or test run, short enough that an abandoned
/// call does not haunt the panel.
const CURRENT_ACTION_MAX_MS: u64 = 10 * 60 * 1000;

/// The tool call this agent has started and not yet finished, if any.
///
/// A `ToolCall` whose `call_id` never gets a matching `ToolResult` is in flight. Gated on
/// `Working`: a closed or terminated session's last call is unfinished only because the
/// session died mid-call, and rendering "editing foo.rs" for an agent that is gone would
/// be exactly the fabricated signal the honest-viz rule forbids.
pub(crate) fn current_action(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    status: &AgentStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RadarAction> {
    if !matches!(status, AgentStatus::Working) {
        return None;
    }
    let mut resolved: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (_, e) in events {
        match &e.event {
            Event::ToolResult { call_id, .. } => {
                resolved.insert(call_id.as_str());
            }
            // Codex's native `apply_patch` never produces a ToolResult: its ONLY
            // completion signal is the patch snapshot carrying the same call id.
            // Verified against real rollouts (0 of 26 calls had a function_call_output,
            // 24 had a patch_apply_end). Without this arm every finished Codex patch
            // would read as permanently in flight.
            Event::FileSnapshot { files } => {
                for f in files {
                    if let Some(cid) = f.call_id.as_deref() {
                        resolved.insert(cid);
                    }
                }
            }
            _ => {}
        }
    }
    // A Codex `apply_patch` is completed by its `patch_apply_end` FileSnapshot, which
    // shares the call_id, so it is already covered by the ToolResult sweep above where
    // the pair exists. Walk newest-first and take the first unresolved call.
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by(|(_, a), (_, b)| {
        b.ts.cmp(&a.ts)
            .then(b.raw_ref.offset.cmp(&a.raw_ref.offset))
            .then(b.id.cmp(&a.id))
    });
    for (_, e) in ordered {
        let Event::ToolCall {
            tool, input, call_id, ..
        } = &e.event
        else {
            continue;
        };
        if resolved.contains(call_id.as_str()) {
            continue;
        }
        // Unlike the activity feed, an in-flight apply_patch is NOT deduped away: the
        // FileSnapshot that would replace it has not arrived yet, so this is the only
        // evidence the write is happening.
        let (kind, label) = tool_activity(tool, input)
            .unwrap_or_else(|| ("write", format!("Edit {}", short_tool_name(tool))));
        let abs = tool_call_abs_path(tool, input);
        let elapsed_ms = (now - e.ts).num_milliseconds().max(0) as u64;
        // A call with no completion record at all (an aborted turn, a harness crash, a
        // record shape we do not parse) would otherwise sit on screen as "editing X"
        // forever. Past the cap we report nothing rather than something false: an absent
        // current action means idle, and that promise is what makes it safe to render big.
        if elapsed_ms > CURRENT_ACTION_MAX_MS {
            return None;
        }
        return Some(RadarAction {
            kind: kind.to_string(),
            tool: short_tool_name(tool),
            label,
            target: abs.as_deref().map(crate::util::display_path),
            started_at: e.ts.to_rfc3339(),
            elapsed_ms,
        });
    }
    None
}

/// Classify a tool call into `(kind, label)`. Codex funnels every action through the
/// `exec` meta-tool, so the real action lives in the normalized input (`cmd` for a shell
/// command, `codex_tool` for an inner tool like `web__run`); a named tool (Claude
/// built-ins, MCP) classifies by its name. Returns `None` to drop a row another event
/// already represents (a Codex `apply_patch`, surfaced by its `FileSnapshot`).
fn tool_activity(tool: &str, input: &serde_json::Value) -> Option<(&'static str, String)> {
    if let Some(cmd) = input.get("cmd").and_then(|v| v.as_str()) {
        return classify_shell_command(cmd);
    }
    if let Some(inner) = input.get("codex_tool").and_then(|v| v.as_str()) {
        return classify_codex_inner_tool(inner);
    }
    Some(classify_named_tool(tool, input))
}

/// Classify a shell command (Codex `exec`) by its leading program into a read / search /
/// write / run kind, keeping the literal command as the label so the feed shows exactly
/// what ran. Returns `None` for `apply_patch` (its write is surfaced by the FileSnapshot).
fn classify_shell_command(cmd: &str) -> Option<(&'static str, String)> {
    let cmd = cmd.trim();
    let verb = shell_verb(cmd);
    if verb == "apply_patch" {
        return None;
    }
    let kind = match verb.as_str() {
        "cat" | "bat" | "head" | "tail" | "less" | "more" | "nl" | "view" => "read",
        "sed" if !cmd.contains(" -i") => "read",
        "grep" | "rg" | "ag" | "ack" | "find" | "fd" | "ls" | "tree" => "search",
        "tee" | "touch" | "mkdir" | "mv" | "cp" | "rm" | "chmod" | "dd" => "write",
        _ => "run",
    };
    Some((kind, crate::util::truncate_chars(cmd, 72)))
}

/// The invoked program of a shell command: skip leading `NAME=value` env assignments and
/// common wrappers (`sudo`, `env`, `time`, `command`), then take the program and strip any
/// directory prefix.
fn shell_verb(cmd: &str) -> String {
    for tok in cmd.split_whitespace() {
        if tok == "sudo" || tok == "env" || tok == "time" || tok == "command" {
            continue;
        }
        if tok.contains('=') && !tok.starts_with('-') && !tok.contains('/') {
            continue; // FOO=bar env assignment
        }
        return tok.rsplit('/').next().unwrap_or(tok).to_string();
    }
    String::new()
}

/// Classify a Codex inner tool (the JS `tools.<name>(...)` behind an `exec`). `apply_patch`
/// returns `None`: its write is surfaced by the FileSnapshot, so the row would be a dup.
/// Web browsing is a search, a bare `exec_command` (dynamic command) is a run, and any
/// other inner tool (MCP, playwright, update_plan) is a generic tool named after it.
fn classify_codex_inner_tool(inner: &str) -> Option<(&'static str, String)> {
    match inner {
        "apply_patch" => None,
        "web__run" | "web_search" => Some(("search", "web search".to_string())),
        "exec_command" => Some(("run", "exec_command".to_string())),
        other => Some(("tool", short_tool_name(other))),
    }
}

/// Classify a named tool call (Claude built-ins, MCP) by tool name, with a target-rich
/// label (the file it touches or the command it runs).
fn classify_named_tool(tool: &str, input: &serde_json::Value) -> (&'static str, String) {
    let kind = match tool {
        "Read" | "NotebookRead" => "read",
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "write",
        "Grep" | "Glob" | "LS" => "search",
        "Bash" | "BashOutput" => "run",
        _ => "tool",
    };
    (kind, tool_target_label(tool, input))
}

/// A short label for a file-write snapshot: the edited file, plus a count when several
/// files changed in one apply.
fn file_snapshot_label(files: &[crate::ir::FileEdit]) -> String {
    match files {
        [] => "edit files".to_string(),
        [one] => format!("Edit {}", path_basename(&one.path)),
        [first, rest @ ..] => {
            format!("Edit {} (+{} more)", path_basename(&first.path), rest.len())
        }
    }
}

/// A target-rich label for a named tool call: the file it touches or the command it runs,
/// prefixed by a compact tool name (e.g. `Read orbLayout.ts`, `Bash cargo test`). Falls
/// back to the tool name alone when the input carries no obvious target. Mirrors the real
/// `Event::ToolCall.input` shapes for Claude (`file_path`/`command`/`pattern`).
fn tool_target_label(tool: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    let short = short_tool_name(tool);
    let target = if let Some(f) = s("file_path")
        .or_else(|| s("path"))
        .or_else(|| s("notebook_path"))
    {
        Some(path_basename(f))
    } else if let Some(c) = s("command").or_else(|| s("cmd")) {
        Some(crate::util::truncate_chars(c.trim(), 64))
    } else {
        s("pattern").map(|p| crate::util::truncate_chars(p, 48))
    };
    match target {
        Some(t) if !t.is_empty() => format!("{short} {t}"),
        _ => short,
    }
}

/// A compact tool name: an MCP tool (`mcp__server__tool`) collapses to its final
/// segment (`tool`); everything else passes through unchanged.
fn short_tool_name(tool: &str) -> String {
    tool.rsplit("__").next().unwrap_or(tool).to_string()
}

/// The last component of a slash/backslash path (keeps the filename, drops the long
/// directory prefix). Returns the input unchanged when it has no separators.
fn path_basename(p: &str) -> String {
    p.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(p)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{EventRecord, FileEdit, RawRef, Role, ToolKind, Turn};
    use chrono::Utc;
    use std::path::PathBuf;

    #[test]
    fn shell_verb_skips_env_and_wrappers() {
        assert_eq!(shell_verb("FOO=bar npm test"), "npm");
        assert_eq!(shell_verb("sudo rm -rf x"), "rm");
        assert_eq!(shell_verb("/usr/bin/sed -n '1,5p' f"), "sed");
        assert_eq!(shell_verb("cargo build"), "cargo");
    }

    #[test]
    fn classify_shell_command_buckets_actions() {
        let kind = |c: &str| classify_shell_command(c).map(|(k, _)| k);
        assert_eq!(kind("sed -n '1,240p' foo.md"), Some("read"));
        assert_eq!(kind("cat notes.txt"), Some("read"));
        assert_eq!(kind("nl file"), Some("read"));
        assert_eq!(kind("rg pattern src/"), Some("search"));
        assert_eq!(kind("grep -n foo bar"), Some("search"));
        assert_eq!(kind("find . -name '*.rs'"), Some("search"));
        assert_eq!(kind("npm test"), Some("run"));
        assert_eq!(kind("git status"), Some("run"));
        assert_eq!(kind("mv a b"), Some("write"));
        // The literal command is kept as the label so the feed shows exactly what ran.
        assert_eq!(classify_shell_command("cat notes.txt").unwrap().1, "cat notes.txt");
        // apply_patch is deduped: its write is surfaced by the FileSnapshot.
        assert!(classify_shell_command("apply_patch <<'EOF'\n*** Begin").is_none());
    }

    #[test]
    fn classify_named_tool_maps_claude_builtins() {
        let jf = |k: &str, v: &str| serde_json::json!({ k: v });
        assert_eq!(
            classify_named_tool("Read", &jf("file_path", "/a/b/orb.ts")),
            ("read", "Read orb.ts".to_string())
        );
        assert_eq!(classify_named_tool("Edit", &jf("file_path", "/a/x.rs")).0, "write");
        assert_eq!(classify_named_tool("Grep", &jf("pattern", "todo")).0, "search");
        assert_eq!(
            classify_named_tool("Bash", &jf("command", "cargo test")),
            ("run", "Bash cargo test".to_string())
        );
        assert_eq!(classify_named_tool("mcp__x__y", &serde_json::Value::Null).0, "tool");
    }

    #[test]
    fn tool_activity_reads_codex_normalized_input() {
        let cmd = serde_json::json!({ "cmd": "rg TODO src/" });
        assert_eq!(
            tool_activity("exec", &cmd),
            Some(("search", "rg TODO src/".to_string()))
        );
        let web = serde_json::json!({ "codex_tool": "web__run" });
        assert_eq!(
            tool_activity("exec", &web),
            Some(("search", "web search".to_string()))
        );
        // apply_patch is deduped: the write is surfaced by its FileSnapshot instead.
        let patch = serde_json::json!({ "codex_tool": "apply_patch" });
        assert_eq!(tool_activity("exec", &patch), None);
    }

    fn ev(offset: u64, event: Event) -> (Turn, EventRecord) {
        let now = Utc::now();
        let turn = Turn {
            id: "t1".into(),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 1,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let rec = EventRecord {
            id: format!("e{offset}"),
            turn_id: "t1".into(),
            session_id: "s".into(),
            ts: now + chrono::Duration::milliseconds(offset as i64),
            event,
            raw_ref: RawRef {
                source_path: PathBuf::from("/x.jsonl"),
                offset,
                line: offset as u32,
            },
        };
        (turn, rec)
    }

    fn tool_call(offset: u64, tool: &str, call_id: &str) -> (Turn, EventRecord) {
        ev(
            offset,
            Event::ToolCall {
                tool: tool.into(),
                input: serde_json::json!({ "file_path": "/Users/x/proj/main.rs" }),
                call_id: call_id.into(),
                kind: ToolKind::Unknown,
            },
        )
    }

    #[test]
    fn current_action_reports_an_unresolved_call() {
        let events = vec![tool_call(1, "Edit", "c1")];
        let a = current_action(&events, &AgentStatus::Working, Utc::now())
            .expect("an unresolved call is current");
        assert_eq!(a.kind, "write");
        assert_eq!(a.tool, "Edit");
        assert_eq!(a.target.as_deref(), Some("/Users/x/proj/main.rs"));
    }

    #[test]
    fn current_action_is_none_once_a_tool_result_lands() {
        let events = vec![
            tool_call(1, "Edit", "c1"),
            ev(
                2,
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: crate::ir::ToolStatus::Ok,
                    bytes: 0,
                    summary: None,
                },
            ),
        ];
        assert!(current_action(&events, &AgentStatus::Working, Utc::now()).is_none());
    }

    /// The Codex case: apply_patch emits NO ToolResult, only a snapshot carrying the same
    /// call id. Verified against real rollouts (0 of 26 had a function_call_output).
    #[test]
    fn a_codex_patch_snapshot_resolves_its_apply_patch_call() {
        let events = vec![
            tool_call(1, "apply_patch", "c9"),
            ev(
                2,
                Event::FileSnapshot {
                    files: vec![FileEdit {
                        path: "/Users/x/proj/main.rs".into(),
                        call_id: Some("c9".into()),
                        ..Default::default()
                    }],
                },
            ),
        ];
        assert!(
            current_action(&events, &AgentStatus::Working, Utc::now()).is_none(),
            "a finished Codex patch must not read as in flight"
        );
    }

    #[test]
    fn a_snapshot_for_a_different_call_does_not_resolve_this_one() {
        let events = vec![
            tool_call(1, "apply_patch", "c9"),
            ev(
                2,
                Event::FileSnapshot {
                    files: vec![FileEdit {
                        path: "/Users/x/other.rs".into(),
                        call_id: Some("SOMETHING_ELSE".into()),
                        ..Default::default()
                    }],
                },
            ),
        ];
        assert!(current_action(&events, &AgentStatus::Working, Utc::now()).is_some());
    }

    #[test]
    fn a_stale_unresolved_call_is_dropped_rather_than_shown_forever() {
        let events = vec![tool_call(1, "Edit", "c1")];
        // Comfortably past the cap: `ev` stamps its own `now`, so a one-millisecond
        // margin would land exactly on the boundary.
        let long_after =
            Utc::now() + chrono::Duration::milliseconds(CURRENT_ACTION_MAX_MS as i64 + 60_000);
        assert!(
            current_action(&events, &AgentStatus::Working, long_after).is_none(),
            "an abandoned call must not haunt the panel"
        );
    }

    #[test]
    fn only_a_working_agent_has_a_current_action() {
        let events = vec![tool_call(1, "Edit", "c1")];
        for s in [
            AgentStatus::Idle,
            AgentStatus::Closed,
            AgentStatus::Terminated,
        ] {
            assert!(
                current_action(&events, &s, Utc::now()).is_none(),
                "a non-working agent must not claim to be mid-edit"
            );
        }
    }

    #[test]
    fn current_action_picks_the_newest_unresolved_call() {
        let events = vec![tool_call(1, "Read", "c1"), tool_call(5, "Edit", "c2")];
        let a = current_action(&events, &AgentStatus::Working, Utc::now()).expect("current");
        assert_eq!(a.tool, "Edit");
    }

    #[test]
    fn apply_patch_target_is_extracted_from_the_patch_dsl() {
        let body = serde_json::json!(
            "*** Begin Patch\n*** Update File: /Users/x/proj/a.rs\n@@\n-old\n+new\n*** End Patch"
        );
        assert_eq!(
            tool_call_abs_path("apply_patch", &body).as_deref(),
            Some("/Users/x/proj/a.rs")
        );
    }

    #[test]
    fn a_multi_file_patch_has_no_single_target() {
        let body = serde_json::json!(
            "*** Update File: /Users/x/a.rs\n*** Update File: /Users/x/b.rs\n"
        );
        assert_eq!(tool_call_abs_path("apply_patch", &body), None);
    }

    #[test]
    fn relative_paths_are_refused_rather_than_guessed_at() {
        // Revealing the wrong file is worse than revealing none.
        let rel = serde_json::json!({ "file_path": "src/main.rs" });
        assert_eq!(tool_call_abs_path("Edit", &rel), None);
    }

    #[test]
    fn recent_activity_surfaces_writes_reads_and_runs() {
        let events = vec![
            ev(1, Event::Thinking { tokens: 10 }),
            ev(
                2,
                Event::ToolCall {
                    tool: "exec".into(),
                    input: serde_json::json!({ "cmd": "sed -n '1,20p' foo.md" }),
                    call_id: "c1".into(),
                    kind: ToolKind::Unknown,
                },
            ),
            ev(
                3,
                Event::FileSnapshot {
                    files: vec![FileEdit {
                        path: "/a/b/notes.md".into(),
                        old_hash: None,
                        new_hash: None,
                        lines_changed: None,
                        call_id: None,
                    }],
                },
            ),
            ev(
                4,
                Event::ToolCall {
                    tool: "exec".into(),
                    input: serde_json::json!({ "cmd": "npm test" }),
                    call_id: "c2".into(),
                    kind: ToolKind::Unknown,
                },
            ),
        ];
        let feed = recent_activity(&events);
        // Newest-first: run, write, read, thinking (was previously all "tool" + no write).
        let kinds: Vec<&str> = feed.iter().map(|a| a.kind.as_str()).collect();
        assert_eq!(kinds, vec!["run", "write", "read", "thinking"]);
        assert!(feed.iter().any(|a| a.kind == "write" && a.label == "Edit notes.md"));
        assert!(feed.iter().any(|a| a.kind == "read" && a.label.contains("sed -n")));
    }
}
