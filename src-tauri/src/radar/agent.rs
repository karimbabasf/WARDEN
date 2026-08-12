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

/// The radar's `(folder, repo)` pair for a session: the cwd basename, plus the repo
/// basename when the two DIFFER (a worktree, or a subdirectory of the repo). Working
/// in the repo root itself leaves `repo` `None` rather than repeating the folder.
///
/// Basenames only. `RadarAgent` is the object the observer projection reads from, so
/// absolute paths are deliberately kept out of it.
fn folder_and_repo(project: Option<&crate::ir::ProjectRef>) -> (Option<String>, Option<String>) {
    let base = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().to_string());
    let Some(project) = project else {
        return (None, None);
    };
    let cwd = base(&project.cwd);
    let repo = project
        .repo_root
        .as_deref()
        .and_then(base)
        .filter(|r| Some(r) != cwd.as_ref());
    (cwd, repo)
}

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
                cache_write: 0,
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
    let title = session_title(s);
    let est_cost_usd = est_cost_usd(&model, &exact);
    let task = first_task(&events);
    let (label, nickname, role, origin) = identity(s, task);
    let (cwd, repo) = folder_and_repo(s.project.as_ref());

    RadarAgent {
        id: s.id.clone(),
        harness: s.harness.as_str().to_string(),
        origin,
        parent_id,
        depth,
        label,
        nickname,
        cwd,
        repo,
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
                cache_write: exact.cache_write,
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

/// The harness's OWN name for this session, recorded by whichever adapter found one.
///
/// Precedence is by AUTHORITY, not recency: a title the operator typed
/// (`session_title_custom`, Claude's `custom-title` record) outranks the one the
/// harness generated for itself (`session_title_ai`, Claude's `ai-title` record, and
/// Codex's thread name), which outranks the legacy single-slot key. The two Claude
/// sources are stored under separate meta keys precisely so this order survives
/// incremental ingest: a tail slice that carries only a fresh `ai-title` merges into
/// meta on its own key and cannot clobber a custom title set in an earlier slice.
pub(crate) fn session_title(s: &Session) -> Option<String> {
    ["session_title_custom", "session_title_ai", "session_title"]
        .iter()
        .filter_map(|k| s.meta.get(*k).and_then(|v| v.as_str()))
        .map(|t| crate::util::truncate_chars(t.trim(), 120))
        .find(|t| !t.is_empty())
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
            Event::UserPrompt { text, .. } | Event::AssistantText { text, .. } => tokenize_len(text),
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
            Event::AssistantText { text, .. } => ("message", crate::util::truncate_chars(text, 80)),
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
    // Codex has NO read tool. It reads a file by shelling out (`sed -n '1,240p' <path>`,
    // `cat <path>`) and searches with `rg`/`grep`, so the only place the file being read
    // is written down is the command string. Without this, every Codex read and search
    // renders as "reading" with no target, which is the "it does not catch what it is
    // reading" gap. Restricted to the verbs `classify_shell_command` already calls read
    // or search: a `run` command's arguments are not a file it is opening.
    if let Some(cmd) = input.get("cmd").and_then(|v| v.as_str()) {
        return shell_command_read_path(cmd);
    }
    None
}

/// The file a read/search shell command targets, as an absolute path, or `None`.
///
/// Takes the LAST absolute-looking token, which is where the path sits in every real
/// form of these commands (`sed -n '1,240p' /a/b.rs`, `rg -n pattern /a/src`), and only
/// for commands whose verb is already classified read or search. Redirections and pipes
/// end the search: in `cat a.txt > /tmp/out` the trailing path is a destination, not the
/// file being read, and guessing wrong here means revealing the wrong file.
fn shell_command_read_path(cmd: &str) -> Option<String> {
    let kind = classify_shell_command(cmd).map(|(k, _)| k)?;
    if kind != "read" && kind != "search" {
        return None;
    }
    cmd.split_whitespace()
        .take_while(|tok| !matches!(*tok, "|" | ">" | ">>" | "2>" | "&&" | ";"))
        .filter(|tok| !tok.starts_with('-'))
        .filter_map(|tok| absolute_or_none(tok.trim_matches(['\'', '"', '`'])))
        .last()
}

/// Keep only absolute paths. A bare basename or a relative fragment cannot be revealed
/// in Finder without guessing a base directory, and a wrong guess opens the wrong file.
fn absolute_or_none(p: &str) -> Option<String> {
    let p = p.trim();
    (!p.is_empty() && p.starts_with('/')).then(|| p.to_string())
}

/// Past this an in-flight call is presented as LONG RUNNING (its label carries an age,
/// e.g. `Read big.log (12m)`) instead of vanishing. A slow build, a big `Bash`, or a
/// slow MCP call reading as idle is exactly the "it does not catch what the agent is
/// doing" failure, so a slow call must degrade to a duration, never to silence.
const CURRENT_ACTION_LONG_RUNNING_MS: u64 = 10 * 60 * 1000;

/// How long a call may sit as the NEWEST thing in a transcript before the radar stops
/// treating it as in flight. Silence this long is better explained by a wedged harness
/// than by a call still running, and claiming "reading X" for an agent that is gone is
/// the fabricated signal the honest-viz rule forbids.
///
/// Deliberately hours, not minutes. It is only reached when NOTHING was written after
/// the call (a long call inside a busy session is anchored by the events around it), and
/// the one call that legitimately runs that long is a synchronous subagent spawn: the
/// parent transcript goes silent for the child's whole run, and "Agent build the radar
/// (1h20m)" is true for every minute of it. The dead-session case this guards against is
/// already covered upstream, where a dead PID and an archived rollout both leave the
/// live set entirely.
const CURRENT_ACTION_WEDGED_MS: u64 = 4 * 60 * 60 * 1000;

/// The tool call this agent has started and not yet finished, if any: the newest
/// `ToolCall` whose `call_id` never got a matching `ToolResult`, and which the turn has
/// not visibly moved past.
///
/// This is a FACT read off the transcript, not an inference from timers, so it is the
/// primary liveness evidence rather than something gated behind a liveness verdict
/// (`super::status::agent_status` promotes an otherwise-idle session on the strength of
/// it). Two guards keep it honest:
/// * a later `AssistantText`/`UserPrompt` means the turn moved on without the call ever
///   returning (an aborted turn, an unparsed record shape) so the call is not current.
///   A later `ToolResult` for a DIFFERENT call is fine: Claude issues parallel calls in
///   one assistant record and resolves them independently;
/// * past [`CURRENT_ACTION_WEDGED_MS`] of total transcript silence the call is wedged.
pub(crate) fn in_flight_tool_call(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&crate::ir::EventRecord> {
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
    // The newest point at which the conversation visibly moved past a tool call: a
    // final assistant message or a fresh operator prompt. A call older than this never
    // returned and is not what the agent is doing now.
    // A mid-turn preamble does NOT advance the turn: the agent narrating a step before
    // it acts is the same turn still running, so only an assistant text that ENDED the
    // turn (or a fresh operator prompt) retires a call. Counting every text block here
    // dropped the live action off the panel while the agent was still mid-step.
    let turn_advanced_at = events
        .iter()
        .filter(|(_, e)| match &e.event {
            Event::AssistantText { turn_complete, .. } => *turn_complete != Some(false),
            Event::UserPrompt { .. } => true,
            _ => false,
        })
        .map(|(_, e)| e.ts)
        .max();

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
        let Event::ToolCall { call_id, .. } = &e.event else {
            continue;
        };
        if resolved.contains(call_id.as_str()) {
            continue;
        }
        if turn_advanced_at.is_some_and(|ts| ts > e.ts) {
            return None;
        }
        let elapsed_ms = (now - e.ts).num_milliseconds().max(0) as u64;
        if elapsed_ms > CURRENT_ACTION_WEDGED_MS {
            return None;
        }
        return Some(e);
    }
    None
}

/// The in-flight call rendered for the detail panel.
///
/// Gated on the agent still being present: a closed or terminated session's last call is
/// unfinished only because the session died mid-call, and rendering "editing foo.rs" for
/// an agent that is gone is fabricated signal. `Idle` is deliberately NOT gated out: an
/// unresolved call is itself the evidence the agent is mid-step (it is what
/// `agent_status` promotes on), so refusing to render one here would hide the live read
/// or edit this whole path exists to show.
pub(crate) fn current_action(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    status: &AgentStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RadarAction> {
    if matches!(status, AgentStatus::Closed | AgentStatus::Terminated) {
        return None;
    }
    let e = in_flight_tool_call(events, now)?;
    let Event::ToolCall { tool, input, .. } = &e.event else {
        return None;
    };
    // Unlike the activity feed, an in-flight apply_patch is NOT deduped away: the
    // FileSnapshot that would replace it has not arrived yet, so this is the only
    // evidence the write is happening.
    let (kind, label) = tool_activity(tool, input)
        .unwrap_or_else(|| ("write", format!("Edit {}", short_tool_name(tool))));
    let elapsed_ms = (now - e.ts).num_milliseconds().max(0) as u64;
    Some(RadarAction {
        kind: kind.to_string(),
        tool: short_tool_name(tool),
        label: long_running_label(label, elapsed_ms),
        target: tool_call_abs_path(tool, input)
            .as_deref()
            .map(crate::util::display_path),
        started_at: e.ts.to_rfc3339(),
        elapsed_ms,
    })
}

/// Append a coarse age to a call that has outlived [`CURRENT_ACTION_LONG_RUNNING_MS`],
/// so `Read big.log` becomes `Read big.log (12m)`. Shorter calls are left alone: an age
/// on every row is noise, and `RadarAction.elapsed_ms` already carries the exact number
/// for a face that wants to render its own timer.
fn long_running_label(label: String, elapsed_ms: u64) -> String {
    if elapsed_ms <= CURRENT_ACTION_LONG_RUNNING_MS {
        return label;
    }
    let minutes = elapsed_ms / 60_000;
    if minutes < 60 {
        format!("{label} ({minutes}m)")
    } else {
        format!("{label} ({}h{}m)", minutes / 60, minutes % 60)
    }
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
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "Artifact" => "write",
        // WebSearch/WebFetch/ToolSearch are lookups, the same act as a Grep over the
        // repo, so they share the search glyph rather than the nondescript tool bucket.
        "Grep" | "Glob" | "LS" | "WebSearch" | "WebFetch" | "ToolSearch" => "search",
        "Bash" | "BashOutput" => "run",
        // Launching a subagent is a STRUCTURAL event (a new node in the forest), not a
        // nondescript tool call, so it gets its own kind. `Task` is the older spelling.
        "Agent" | "Task" => "spawn",
        "Skill" => "skill",
        _ => "tool",
    };
    (kind, named_tool_label(tool, input))
}

/// A label for the tool names whose target does not live in `file_path`/`command`/
/// `pattern`: a subagent spawn is named by WHAT it was asked to do, a skill by which
/// skill, a web lookup by its query or URL. Everything else falls through to the
/// generic file/command/pattern label.
fn named_tool_label(tool: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let named = match tool {
        // `description` is the 3-to-5 word task the operator gave the subagent, and
        // `name` is the handle it is addressable by: both beat "Agent".
        "Agent" | "Task" => s("description").or_else(|| s("name")).or_else(|| s("subagent_type")),
        "Skill" => s("skill"),
        "ToolSearch" => s("query"),
        "WebSearch" => s("query"),
        "WebFetch" => s("url"),
        _ => None,
    };
    match named {
        Some(t) => format!("{} {}", short_tool_name(tool), crate::util::truncate_chars(t, 64)),
        None => tool_target_label(tool, input),
    }
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
    use crate::ir::{EventRecord, FileEdit, ProjectRef, RawRef, Role, ToolKind, Turn};
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
    fn a_wedged_unresolved_call_is_dropped_rather_than_shown_forever() {
        let events = vec![tool_call(1, "Edit", "c1")];
        // Comfortably past the cap: `ev` stamps its own `now`, so a one-millisecond
        // margin would land exactly on the boundary.
        let long_after =
            Utc::now() + chrono::Duration::milliseconds(CURRENT_ACTION_WEDGED_MS as i64 + 60_000);
        assert!(
            current_action(&events, &AgentStatus::Working, long_after).is_none(),
            "an abandoned call must not haunt the panel"
        );
    }

    /// The old behaviour dropped a call the moment it passed 10 minutes, so a slow build
    /// or a big Read read as idle. It now degrades to an age instead of to silence.
    #[test]
    fn a_long_running_call_is_shown_with_its_age_not_dropped() {
        let events = vec![tool_call(1, "Bash", "c1")];
        // `ev` stamps its own `now`, so add a second of slack to land inside the minute
        // rather than on its boundary.
        let later = Utc::now() + chrono::Duration::milliseconds(12 * 60 * 1000 + 1_000);
        let a = current_action(&events, &AgentStatus::Working, later)
            .expect("a slow call must still be reported");
        assert!(a.label.ends_with("(12m)"), "got {:?}", a.label);
        assert!(a.elapsed_ms >= 12 * 60 * 1000);

        let much_later = Utc::now() + chrono::Duration::milliseconds(95 * 60 * 1000 + 1_000);
        let a = current_action(&events, &AgentStatus::Working, much_later).expect("still current");
        assert!(a.label.ends_with("(1h35m)"), "got {:?}", a.label);
    }

    /// An unresolved call is the evidence the agent is mid-step, so it must render for an
    /// `Idle` verdict too (`agent_status` promotes on exactly this). A CLOSED or
    /// TERMINATED agent is gone, and its dangling call must never claim it is editing.
    #[test]
    fn only_a_present_agent_has_a_current_action() {
        let events = vec![tool_call(1, "Edit", "c1")];
        assert!(
            current_action(&events, &AgentStatus::Idle, Utc::now()).is_some(),
            "an in-flight call must survive a stale idle verdict"
        );
        for s in [AgentStatus::Closed, AgentStatus::Terminated] {
            assert!(
                current_action(&events, &s, Utc::now()).is_none(),
                "an agent that is gone must not claim to be mid-edit"
            );
        }
    }

    /// A tool call the turn already moved past (a final assistant message landed after
    /// it) never returned. Reporting it would pin a dead action to the panel.
    #[test]
    fn a_call_the_turn_moved_past_is_not_current() {
        let events = vec![
            tool_call(1, "Edit", "c1"),
            ev(
                2,
                Event::AssistantText {
                    text: "done".into(),
                    turn_complete: None,
                },
            ),
        ];
        assert!(current_action(&events, &AgentStatus::Working, Utc::now()).is_none());
    }

    /// Claude issues parallel calls in ONE assistant record and resolves them
    /// independently, so a result for a sibling call must not retire the one still open.
    #[test]
    fn a_sibling_result_does_not_retire_a_parallel_call() {
        let events = vec![
            tool_call(1, "Read", "c1"),
            tool_call(1, "Read", "c2"),
            ev(
                3,
                Event::ToolResult {
                    call_id: "c1".into(),
                    status: crate::ir::ToolStatus::Ok,
                    bytes: 0,
                    summary: None,
                },
            ),
        ];
        let a = current_action(&events, &AgentStatus::Working, Utc::now())
            .expect("the unresolved sibling is still in flight");
        assert_eq!(a.tool, "Read");
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

    // ── folder_and_repo ───────────────────────────────────────────────────────

    fn project_at(cwd: &str, repo_root: Option<&str>) -> ProjectRef {
        ProjectRef {
            cwd: PathBuf::from(cwd),
            repo_root: repo_root.map(PathBuf::from),
            git_branch: None,
        }
    }

    #[test]
    fn a_session_in_the_repo_root_reports_no_separate_repo() {
        let p = project_at("/Users/k/Developer/Apps/WARDEN", Some("/Users/k/Developer/Apps/WARDEN"));
        assert_eq!(
            folder_and_repo(Some(&p)),
            (Some("WARDEN".into()), None),
            "the ordinary case must not grow a redundant second name"
        );
    }

    #[test]
    fn a_session_in_a_worktree_reports_the_repo_it_belongs_to() {
        let p = project_at(
            "/Users/k/Developer/Apps/WARDEN-feature",
            Some("/Users/k/Developer/Apps/WARDEN"),
        );
        assert_eq!(
            folder_and_repo(Some(&p)),
            (Some("WARDEN-feature".into()), Some("WARDEN".into()))
        );
    }

    #[test]
    fn a_session_in_a_subdirectory_reports_the_repo_it_belongs_to() {
        let p = project_at(
            "/Users/k/Developer/Apps/WARDEN/src-tauri",
            Some("/Users/k/Developer/Apps/WARDEN"),
        );
        assert_eq!(
            folder_and_repo(Some(&p)),
            (Some("src-tauri".into()), Some("WARDEN".into()))
        );
    }

    #[test]
    fn an_unknown_repo_leaves_the_folder_alone() {
        let p = project_at("/tmp/scratch", None);
        assert_eq!(folder_and_repo(Some(&p)), (Some("scratch".into()), None));
        assert_eq!(folder_and_repo(None), (None, None));
    }
}
