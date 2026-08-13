//! AWAITING detection: is this agent stopped on the OPERATOR?
//!
//! Working and finished are both states the agent owns. Awaiting is the third one, and
//! it is the only state the agent cannot leave by itself: it asked something and nothing
//! moves until a human answers. That is why it is worth its own verdict rather than a
//! flavour of idle. An idle agent needs nothing from you; an awaiting one is the only
//! thing on the board that does.
//!
//! Three sources, strongest first. Each is a REAL signal (honest viz); none of them
//! infers a question from silence:
//!
//! 1. **The harness says so.** Claude Code's session registry carries
//!    `status: "waiting"` plus a `waitingFor` reason (`permission prompt`, `input
//!    needed`, `dialog open`, `sandbox request`, ...). It is the harness's own statement
//!    about its own UI, so it outranks everything here. Read by
//!    [`super::liveness::partition_claude`]; folded into the closed vocabulary by
//!    [`AwaitingReason::from_registry`].
//!
//! 2. **A blocking tool is in flight.** `AskUserQuestion` and `ExitPlanMode` do not
//!    return until the operator answers. Verified on real transcripts: an
//!    `AskUserQuestion` `tool_use` and its `tool_result` 74s apart, the gap being the
//!    human. An unresolved call to one of these is not a slow tool, it is a question on
//!    screen, so it BEATS a Working verdict rather than being promoted from Idle.
//!    Claude-only by nature; Codex has no such tool and simply never matches.
//!
//! 3. **The finished turn ends in a question.** The case with no structured signal at
//!    all: the agent just wrote "Which of these do you want?" as ordinary output and
//!    stopped. This is the ONLY heuristic here, so it is deliberately narrow. It needs a
//!    turn that actually ENDED (never a mid-turn preamble), and a question mark closing
//!    the last real line of prose. It is also the only awaiting signal available to
//!    Codex, whose rollouts record no approval or dialog event of any kind (checked
//!    across 195 real rollouts: zero approval-request records).
//!
//! Pure: no store, no filesystem, no clock beyond what the caller injects, so every rule
//! is unit-tested directly.

use crate::ir::{Event, EventRecord, Turn};

/// Why an agent is waiting on the operator: a CLOSED vocabulary.
///
/// Deliberately three coarse values rather than the harness's own phrasing. `waitingFor`
/// is free text produced by a dialog (it can carry a tool name, a path, an MCP server
/// name), and `RadarAgent` is the struct the observer projection reads from, so letting
/// raw harness text through here would be a slow leak of local detail toward the wire.
/// Folding it to a fixed set keeps the readout honest and keeps the redaction boundary
/// something you can reason about by reading this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwaitingReason {
    /// It asked a question and wants an answer.
    Question,
    /// It wants a yes/no: a permission prompt, a plan to approve, a sandbox request.
    Approval,
    /// A prompt is open that is neither of the above (a dialog, an elicitation).
    Input,
}

impl AwaitingReason {
    /// snake_case wire value for the `radar_state` contract (`awaitingReason`).
    pub fn as_str(&self) -> &'static str {
        match self {
            AwaitingReason::Question => "question",
            AwaitingReason::Approval => "approval",
            AwaitingReason::Input => "input",
        }
    }

    /// Fold Claude's registry `waitingFor` text into the closed vocabulary.
    ///
    /// The harness's own strings (v2.1.229): `permission prompt`, `sandbox request`,
    /// `worker request`, `goal proposal`, `input needed`, `dialog open`, `inline`. An
    /// unrecognised or absent value degrades to [`AwaitingReason::Input`]: we know it is
    /// blocked, we just do not know on what, and saying "input needed" is honest where
    /// guessing "question" would not be.
    pub fn from_registry(waiting_for: Option<&str>) -> Self {
        let raw = waiting_for.unwrap_or_default().to_ascii_lowercase();
        if raw.contains("permission")
            || raw.contains("approval")
            || raw.contains("approve")
            || raw.contains("sandbox")
            || raw.contains("worker request")
            || raw.contains("proposal")
            || raw.contains("plan")
        {
            return AwaitingReason::Approval;
        }
        if raw.contains("question") {
            return AwaitingReason::Question;
        }
        AwaitingReason::Input
    }
}

/// Tool calls that do not return until a human answers them.
///
/// Not "tools that are slow": these are the harness's own operator prompts wearing a
/// tool call's clothes, so an unresolved one is a question sitting on screen. Anything
/// not listed falls through to the ordinary in-flight-tool rule and reads as Working,
/// which is the safe direction to be wrong in (a slow tool mislabelled as a question
/// would send the operator to a session that needs nothing).
const BLOCKING_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode", "exit_plan_mode"];

/// Does this tool block on the operator? Matches the bare name and the `mcp__x__y` tail
/// so a namespaced re-export is still recognised.
pub fn is_blocking_tool(tool: &str) -> bool {
    let bare = tool.rsplit("__").next().unwrap_or(tool);
    BLOCKING_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tool) || t.eq_ignore_ascii_case(bare))
}

/// The reason an in-flight blocking tool is waiting: a plan needs approving, everything
/// else on the list is a question.
fn blocking_tool_reason(tool: &str) -> AwaitingReason {
    let bare = tool.rsplit("__").next().unwrap_or(tool).to_ascii_lowercase();
    if bare.contains("plan") {
        AwaitingReason::Approval
    } else {
        AwaitingReason::Question
    }
}

/// Is a BLOCKING tool call currently in flight (started, never resolved)?
///
/// Reuses [`super::agent::in_flight_tool_call`] so there is exactly one definition of
/// "in flight" in the codebase: the same resolution sweep (tool results, plus Codex's
/// patch snapshots) and the same turn-advance cutoff. Returns the reason, so the caller
/// does not re-derive it.
pub fn blocking_call_in_flight(
    events: &[(Turn, EventRecord)],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<AwaitingReason> {
    let e = super::agent::in_flight_tool_call(events, now)?;
    let Event::ToolCall { tool, .. } = &e.event else {
        return None;
    };
    is_blocking_tool(tool).then(|| blocking_tool_reason(tool))
}

/// Did the agent's LAST completed turn end by asking the operator something?
///
/// Only ever consulted for an agent that has already been judged quiet, and only fires
/// on a turn the agent actually ENDED. A mid-turn preamble that happens to contain a
/// question ("Should I check the config? Let me look.") is still a working agent, and
/// `turn_complete == Some(false)` keeps it that way.
///
/// `turn_complete == None` (Codex, and Claude rows ingested before the field existed)
/// keeps the module-wide convention: no information means the pre-existing assumption
/// that a trailing text ended the turn. For Codex that is the intended path, since its
/// rollouts carry no stop reason at all and a trailing `agent_message` is genuinely the
/// end of its turn.
pub fn finished_turn_asks_question(events: &[(Turn, EventRecord)]) -> bool {
    let last = events
        .iter()
        .filter(|(_, e)| tail_priority(&e.event).is_some())
        .max_by(|(_, a), (_, b)| {
            a.ts.cmp(&b.ts)
                .then(a.raw_ref.offset.cmp(&b.raw_ref.offset))
                .then(tail_priority(&a.event).cmp(&tail_priority(&b.event)))
                .then(a.id.cmp(&b.id))
        });
    match last.map(|(_, e)| &e.event) {
        Some(Event::AssistantText {
            text,
            turn_complete,
        }) => *turn_complete != Some(false) && text_asks_question(text),
        _ => false,
    }
}

/// The events that can be the TAIL of a turn. Same set as the liveness rule's actions:
/// if a tool call or a tool result is newer than the text, the agent is mid-step and the
/// text is not the last word.
fn tail_priority(event: &Event) -> Option<u8> {
    match event {
        Event::UserPrompt { .. } | Event::ToolCall { .. } => Some(3),
        Event::ToolResult { .. } => Some(2),
        Event::AssistantText { .. } => Some(1),
        _ => None,
    }
}

/// How many trailing lines to look through for the question before giving up. A question
/// is very often followed by the options it offers ("Which one? / 1. ... / 2. ..."), so
/// the last line of the message is regularly the last OPTION, not the question.
const TRAILING_LINES_SCANNED: usize = 8;

/// Does this message end by asking the operator a question?
///
/// The rule, and why each part of it is there:
/// * fenced code is stripped first, because a `?` inside a regex or a URL is not a
///   question;
/// * lines are walked from the BOTTOM, skipping list items, table rows, quotes and
///   headings, because a question is usually followed by the options it offers;
/// * the first line of real prose found decides it, and only if it CLOSES on a question
///   mark (trailing emphasis, backticks, brackets and quotes stripped first).
///
/// Walking up past the options rather than reading the whole message is what keeps this
/// narrow: a message that mentions a question in passing and then reports a result does
/// not match, because the result is the trailing prose.
pub fn text_asks_question(text: &str) -> bool {
    let stripped = strip_code_fences(text);
    let mut scanned = 0usize;
    for line in stripped.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if scanned >= TRAILING_LINES_SCANNED {
            return false;
        }
        scanned += 1;
        if is_option_line(line) {
            continue;
        }
        return closes_on_question_mark(line);
    }
    false
}

/// A line that carries an OPTION rather than the question itself: a bullet, a numbered
/// choice, a table row, a quote, a heading, or a horizontal rule.
fn is_option_line(line: &str) -> bool {
    let b = line.as_bytes();
    // A bullet marker only counts when a SPACE follows it. `**Should I deploy?**` opens
    // with `*` and is prose in bold, not a list item, and swallowing it as an option was
    // enough to make a bolded question read as no question at all.
    let mut chars = line.chars();
    if let (Some(first), Some(second)) = (chars.next(), chars.next()) {
        if matches!(first, '-' | '*' | '+' | '#') && second == ' ' {
            return true;
        }
    }
    if line.starts_with('>') || line.starts_with('|') {
        return true;
    }
    // `1. ` / `2) ` / `3 -` is a numbered choice.
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && digits < 3
        && matches!(b.get(digits), Some(b'.') | Some(b')') | Some(b':') | Some(b'-'))
}

/// Does this line CLOSE on a question mark, ignoring trailing markdown furniture?
///
/// Requires two real WORD characters in front of the mark, counted after the furniture
/// is discounted, so a bare "?" and a bolded "**?**" both fail. Counting raw characters
/// instead let the asterisks themselves clear the bar.
fn closes_on_question_mark(line: &str) -> bool {
    let trimmed = line.trim_end_matches(|c: char| {
        matches!(c, '*' | '_' | '`' | '"' | '\'' | ')' | ']' | '}' | '>' | ' ')
    });
    match trimmed.strip_suffix('?') {
        Some(before) => before.chars().filter(|c| c.is_alphanumeric()).count() >= 2,
        None => false,
    }
}

/// Remove fenced code blocks (``` and ~~~), including an unterminated trailing fence.
///
/// A `?` inside code is punctuation, not a question, and an agent that ends its turn on
/// a code block has not asked anything. An unterminated fence swallows the rest of the
/// message on purpose: the operator is looking at code, not a prompt.
fn strip_code_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{RawRef, Role, ToolKind, ToolStatus};
    use chrono::{DateTime, TimeZone, Utc};

    fn ts(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + sec, 0)
            .single()
            .expect("valid ts")
    }

    fn rec(id: &str, at: i64, event: Event) -> (Turn, EventRecord) {
        let turn = Turn {
            id: format!("t-{id}"),
            session_id: "s".into(),
            parent_id: None,
            role: Role::Assistant,
            index: 0,
            started_at: ts(at),
            duration_ms: None,
            is_sidechain: false,
        };
        let record = EventRecord {
            id: id.into(),
            turn_id: format!("t-{id}"),
            session_id: "s".into(),
            ts: ts(at),
            event,
            raw_ref: RawRef {
                source_path: "/tmp/s.jsonl".into(),
                offset: at as u64,
                line: at as u32,
            },
        };
        (turn, record)
    }

    fn text(t: &str, complete: Option<bool>) -> Event {
        Event::AssistantText {
            text: t.into(),
            turn_complete: complete,
        }
    }

    fn call(tool: &str, id: &str) -> Event {
        Event::ToolCall {
            tool: tool.into(),
            input: serde_json::json!({}),
            call_id: id.into(),
            kind: ToolKind::Builtin,
        }
    }

    fn result(id: &str) -> Event {
        Event::ToolResult {
            call_id: id.into(),
            status: ToolStatus::Ok,
            bytes: 0,
            summary: None,
        }
    }

    #[test]
    fn plain_question_at_the_end_counts() {
        assert!(text_asks_question(
            "Done with the parser. Which backend do you want next?"
        ));
    }

    #[test]
    fn question_above_its_options_counts() {
        let msg = "I can take this two ways.\n\nWhich do you prefer?\n\n1. Ship the quick fix\n2. Rewrite the adapter\n";
        assert!(text_asks_question(msg));
        let bullets = "Which do you prefer?\n\n- Ship the quick fix\n- Rewrite the adapter";
        assert!(text_asks_question(bullets));
    }

    #[test]
    fn a_finished_report_is_not_a_question() {
        assert!(!text_asks_question(
            "Fixed the flaky test. cargo test passes: 214 passed, 0 failed."
        ));
        // A question asked in passing, then a result: the trailing prose decides.
        assert!(!text_asks_question(
            "You asked whether the cache was warm?\n\nIt was. Build is green."
        ));
    }

    #[test]
    fn a_question_mark_inside_code_is_not_a_question() {
        let msg = "Here is the regex:\n\n```\n^/api/v1/(.*)\\?debug=1$\n```\n";
        assert!(!text_asks_question(msg));
    }

    #[test]
    fn trailing_markdown_furniture_does_not_hide_the_mark() {
        assert!(text_asks_question("**Should I deploy this now?**"));
        assert!(text_asks_question("Do you want the `--force` variant?  "));
    }

    #[test]
    fn a_bare_question_mark_is_not_a_question() {
        assert!(!text_asks_question("?"));
        assert!(!text_asks_question("**?**"));
    }

    #[test]
    fn a_url_query_string_is_not_a_question() {
        assert!(!text_asks_question(
            "Deployed. Logs: https://vercel.com/x/y?filter=error"
        ));
    }

    #[test]
    fn only_a_completed_turn_asks() {
        let mid = vec![rec("a", 1, text("Should I check the config?", Some(false)))];
        assert!(
            !finished_turn_asks_question(&mid),
            "a mid-turn preamble is still a working agent"
        );
        let done = vec![rec("a", 1, text("Should I check the config?", Some(true)))];
        assert!(finished_turn_asks_question(&done));
        // Codex and legacy rows carry no stop reason, so the old trailing-text assumption
        // holds.
        let unknown = vec![rec("a", 1, text("Should I check the config?", None))];
        assert!(finished_turn_asks_question(&unknown));
    }

    #[test]
    fn a_newer_tool_call_means_the_text_was_not_the_last_word() {
        let events = vec![
            rec("a", 1, text("Which one do you want?", Some(true))),
            rec("b", 2, call("Read", "c1")),
        ];
        assert!(!finished_turn_asks_question(&events));
    }

    #[test]
    fn an_unresolved_blocking_call_is_awaiting_and_a_resolved_one_is_not() {
        let pending = vec![
            rec("a", 1, text("Let me ask.", Some(false))),
            rec("b", 2, call("AskUserQuestion", "c1")),
        ];
        assert_eq!(
            blocking_call_in_flight(&pending, ts(3)),
            Some(AwaitingReason::Question)
        );
        let answered = vec![
            rec("a", 1, text("Let me ask.", Some(false))),
            rec("b", 2, call("AskUserQuestion", "c1")),
            rec("c", 3, result("c1")),
        ];
        assert_eq!(blocking_call_in_flight(&answered, ts(4)), None);
    }

    #[test]
    fn a_plan_awaits_approval_and_an_ordinary_tool_awaits_nothing() {
        let plan = vec![rec("a", 1, call("ExitPlanMode", "c1"))];
        assert_eq!(
            blocking_call_in_flight(&plan, ts(2)),
            Some(AwaitingReason::Approval)
        );
        let slow_read = vec![rec("a", 1, call("Read", "c1"))];
        assert_eq!(
            blocking_call_in_flight(&slow_read, ts(2)),
            None,
            "a slow tool is Working, never a question"
        );
    }

    #[test]
    fn registry_reasons_fold_into_the_closed_vocabulary() {
        for s in [
            "permission prompt",
            "sandbox request",
            "worker request",
            "goal proposal",
        ] {
            assert_eq!(
                AwaitingReason::from_registry(Some(s)),
                AwaitingReason::Approval,
                "{s}"
            );
        }
        for s in ["input needed", "dialog open", "inline", "something new"] {
            assert_eq!(
                AwaitingReason::from_registry(Some(s)),
                AwaitingReason::Input,
                "{s}"
            );
        }
        assert_eq!(
            AwaitingReason::from_registry(None),
            AwaitingReason::Input,
            "no reason given is still honestly 'blocked on input'"
        );
    }

    #[test]
    fn blocking_tools_are_recognised_namespaced_or_bare() {
        assert!(is_blocking_tool("AskUserQuestion"));
        assert!(is_blocking_tool("mcp__somewhere__AskUserQuestion"));
        assert!(!is_blocking_tool("Read"));
        assert!(
            !is_blocking_tool("EnterPlanMode"),
            "entering a plan blocks nobody"
        );
    }
}
