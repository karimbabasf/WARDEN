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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct FileEdit {
    pub path: String,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub lines_changed: Option<u32>,
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
            Event::UserPrompt { text, .. } | Event::AssistantText { text } => text.clone(),
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct FeatureVector {
    pub session_id: SessionId,
    pub token_burn_total: u64,
    pub context_saturation_peak: f64,
    pub cache_read_ratio: f64,
    pub search_in_main_context: u32,
    pub subagent_spawn_count: u32,
    pub subagent_delegation_rate: f64,
    pub tool_call_count: u32,
    pub tool_error_rate: f64,
    pub ignored_error_count: u32,
    pub reprompt_count: u32,
    pub prompt_specificity: f64,
    pub file_churn: f64,
    pub thrash_index: f64,
    pub planning_ratio: f64,
    pub verification_present: bool,
    pub permission_friction: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct CompetenceProfile {
    pub session_count: u32,
    pub event_count: u64,
    pub finding_count: u32,
    pub token_burn_total: u64,
    pub avg_prompt_specificity: f64,
    pub avg_cache_read_ratio: f64,
    pub avg_tool_error_rate: f64,
    pub no_delegation_sessions: u32,
    pub context_bloat_sessions: u32,
    pub unverified_sessions: u32,
    pub repeated_explanation_clusters: Vec<RepeatedCluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct RepeatedCluster {
    pub phrase: String,
    pub count: u32,
    pub session_ids: Vec<SessionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceRef {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub event_id: Option<EventId>,
    pub quote: Option<String>,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    pub id: String,
    pub pattern_id: String,
    pub title: String,
    pub severity: u8,
    pub frequency: f64,
    pub est_cost_tokens: u64,
    pub est_cost_minutes: u64,
    pub confidence: f64,
    pub rationale: String,
    pub evidence: Vec<EvidenceRef>,
    pub status: String,
    pub verifier_verdict: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Diagnosis {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub ranked_findings: Vec<Finding>,
    pub do_items: Vec<String>,
    pub stop_items: Vec<String>,
    pub narrative: String,
    pub detector_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunScope {
    pub harness: Option<String>,
    pub query: Option<String>,
    pub force: Option<bool>,
    pub max_files: Option<usize>,
}
