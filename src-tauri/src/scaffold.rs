//! Forward seams for M3-M7. These modules deliberately expose real contracts and return
//! explicit "not built in this slice" errors rather than pretending to operate.
use crate::ir::{Harness, ProjectRef, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentInstance {
    pub id: String,
    pub session_id: Option<SessionId>,
    pub harness: Harness,
    pub pid: i32,
    pub location: PhysicalLocation,
    pub status: AgentStatus,
    pub confidence: f32,
    pub project: Option<ProjectRef>,
    pub last_activity: String,
    pub bound_via: BindSource,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhysicalLocation {
    TerminalTab {
        app: String,
        term_session_id: Option<String>,
        tty: String,
    },
    TmuxPane {
        client_tty: String,
        target: String,
    },
    GuiWindow {
        app: String,
        window_id: u32,
        title: String,
    },
    Cloud {
        provider: String,
        url: Option<String>,
    },
    Unknown {
        tty: Option<String>,
        cwd: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Working,
    AwaitingInput,
    AwaitingPermission,
    Idle,
    Errored,
    Exited,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindSource {
    HookSelfRegister,
    EnvSessionId,
    TtyMatch,
    CwdHeuristic,
    FuguDisambig,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeArtifactRequest {
    pub finding_id: String,
    pub kind: String,
    pub target_path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HookEvent {
    pub harness: Harness,
    pub session_id: Option<SessionId>,
    pub payload: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoiceIntent {
    pub utterance: String,
    pub mode: String,
}

pub fn not_in_slice(feature: &str) -> String {
    format!("{feature} is an M3-M7 seam in this build. M0-M2 are complete; this boundary is typed but intentionally not fake-implemented.")
}
