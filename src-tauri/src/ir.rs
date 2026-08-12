use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub type SessionId = String;
pub type TurnId = String;
pub type EventId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    ClaudeCode,
    Codex,
    Cursor,
    Hermes,
    Generic(String),
}
impl Harness {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Hermes => "hermes",
            Self::Generic(s) => s.as_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ProjectRef {
    pub cwd: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub git_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Session {
    pub id: SessionId,
    pub harness: Harness,
    pub external_id: String,
    pub project: Option<ProjectRef>,
    pub model_ids: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub source_path: PathBuf,
    pub raw_hash: u64,
    pub ingested_at: DateTime<Utc>,
    pub meta: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Turn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub parent_id: Option<TurnId>,
    pub role: Role,
    pub index: u32,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub is_sidechain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    pub kind: String,
    pub name: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Orchestration {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Builtin,
    Mcp,
    SubagentTask,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Error,
    /// Started and not finished. Every harness models a tool call as a start record
    /// and a separate end record joined by an id, so "in flight" was previously only
    /// derivable at render time by subtracting the ends from the starts (see
    /// `radar::agent::in_flight_tool_call`). This makes it a first-class value of the
    /// IR instead, so a harness that reports the state DIRECTLY has somewhere to put
    /// it: Cursor mutates one row in place and carries `toolFormerData.status`, whose
    /// `loading` maps here with no join at all. No adapter emits it today, since
    /// neither Claude nor Codex writes an in-flight record.
    Running,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Running => "running",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct FileEdit {
    pub path: String,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub lines_changed: Option<u32>,
    /// The tool call this edit completes, when the harness reports one.
    ///
    /// Codex's native `apply_patch` never emits a `function_call_output`: verified against
    /// real rollouts, 0 of 26 calls had one while 24 had a `patch_apply_end` sharing this
    /// call id. Without this field a finished patch looks like a tool call that never
    /// returned, so the radar would render it as forever in-flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum Event {
    UserPrompt {
        text: String,
        attachments: Vec<Attachment>,
        is_meta: bool,
    },
    AssistantText {
        text: String,
        /// Did the assistant message carrying this text END the turn?
        ///
        /// The harness writes ONE transcript line per content block, so a mid-turn
        /// preamble ("Let me check the config.") and a final answer are both a lone
        /// text block and cannot be told apart by shape. The message's `stop_reason`
        /// is what separates them, and it is the only honest source:
        /// * `Some(true)`: `end_turn`/`stop_sequence`/`max_tokens`, so the agent
        ///   stopped and is waiting on the operator.
        /// * `Some(false)`: `tool_use`, or a null (still streaming) reason, so the
        ///   turn CONTINUES and a tool call is coming. Measured on this machine, 79%
        ///   of text-only assistant lines are this case.
        /// * `None`: no information. Rows ingested before this field existed, and
        ///   harnesses that do not report a stop reason (Codex). Readers must treat
        ///   it as the pre-existing "a trailing text ends the turn" assumption, so
        ///   old stored events keep their old verdict.
        #[serde(default)]
        turn_complete: Option<bool>,
    },
    Thinking {
        tokens: u32,
    },
    ToolCall {
        tool: String,
        input: Value,
        call_id: String,
        kind: ToolKind,
    },
    ToolResult {
        call_id: String,
        status: ToolStatus,
        bytes: u64,
        summary: Option<String>,
    },
    TokenUsage {
        input: u32,
        output: u32,
        cache_creation: u32,
        cache_read: u32,
        model: String,
        orchestration: Option<Orchestration>,
    },
    FileSnapshot {
        files: Vec<FileEdit>,
    },
    SubagentSpawn {
        source_assistant_uuid: String,
        child_session: Option<SessionId>,
    },
    ModeChange {
        mode: String,
    },
    Error {
        source: String,
        message: String,
    },
    SystemNotice {
        subtype: String,
        data: Value,
    },
}
impl Event {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Event::UserPrompt { .. } => "user_prompt",
            Event::AssistantText { .. } => "assistant_text",
            Event::Thinking { .. } => "thinking",
            Event::ToolCall { .. } => "tool_call",
            Event::ToolResult { .. } => "tool_result",
            Event::TokenUsage { .. } => "token_usage",
            Event::FileSnapshot { .. } => "file_snapshot",
            Event::SubagentSpawn { .. } => "subagent_spawn",
            Event::ModeChange { .. } => "mode_change",
            Event::Error { .. } => "error",
            Event::SystemNotice { .. } => "system_notice",
        }
    }
    pub fn searchable_text(&self) -> String {
        match self {
            Event::UserPrompt { text, .. } | Event::AssistantText { text, .. } => text.clone(),
            Event::ToolCall { tool, input, .. } => format!("{tool} {input}"),
            Event::ToolResult { summary, .. } => summary.clone().unwrap_or_default(),
            Event::Error { source, message } => format!("{source} {message}"),
            Event::SystemNotice { subtype, data } => format!("{subtype} {data}"),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RawRef {
    pub source_path: PathBuf,
    pub offset: u64,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EventRecord {
    pub id: EventId,
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub ts: DateTime<Utc>,
    pub event: Event,
    pub raw_ref: RawRef,
}
