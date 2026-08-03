//! Shared RADAR data types: the frozen `radar_state` contract.
//!
//! These are the camelCase-serialized structs returned by `get_radar_state` and
//! emitted on the `radar_state` event. They are a LEAF module so every other radar
//! submodule can depend on them without forming a parent↔child cycle.

use serde::{Deserialize, Serialize};

/// One agent (root or subagent) in the live forest — the frozen `radar_state`
/// contract, serialized camelCase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarAgent {
    pub id: String,
    pub harness: String,
    pub origin: Option<String>,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub label: String,
    pub nickname: Option<String>,
    /// The agent's project-folder basename (root only), e.g. `WARDEN`. Carried
    /// separately from `label` so the FACE can render a "folder · model" subtitle
    /// even when `label` is the agent's task. `None` when there is no project cwd.
    pub cwd: Option<String>,
    /// The basename of the REPO that `cwd` belongs to, e.g. `WARDEN` for a session
    /// running in the `WARDEN-feature` worktree. Present only when it DIFFERS from
    /// `cwd`, so it reads as "this folder is a worktree of that repo" and is `None`
    /// for the ordinary case of working in the repo root itself.
    ///
    /// This is what groups sessions the harnesses now scatter across one worktree
    /// each. A basename, never a path: `RadarAgent` is what the observer projection
    /// reads from, and absolute paths are deliberately kept out of it.
    pub repo: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    /// The harness's OWN human-readable session title, when it has one: Claude's
    /// `custom-title` record, or the H1 of a Codex plan. Distinct from `label` (which
    /// the radar derives) and from `nickname`. `None` when the harness never named it.
    pub title: Option<String>,
    /// What this agent is doing RIGHT NOW: the newest tool call with no result yet.
    /// `None` when the agent is between calls, which is what makes it honest to render
    /// large: an absent value means genuinely idle, not "we could not tell".
    pub current_action: Option<RadarAction>,
    /// Agent-team membership, when the harness groups agents into a named team.
    pub team: Option<RadarTeam>,
    pub status: String,
    pub context_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
    pub context_breakdown: RadarContextBreakdown,
    pub composition: RadarComposition,
    pub recent_activity: Vec<RadarActivity>,
    pub child_count: u32,
    pub started_at: String,
    pub est_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarExact {
    pub cache_read: u64,
    /// Genuinely new input tokens, billed at the plain input rate.
    pub fresh: u64,
    /// `cache_creation` tokens, billed at a premium over the input rate. Split
    /// out from `fresh` because the two bill differently; summing them made the
    /// premium impossible to apply.
    pub cache_write: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarEstimated {
    pub preamble: u64,
    pub conversation: u64,
    pub tool_output: u64,
    pub thinking: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarComposition {
    pub exact: RadarExact,
    /// `None` (serialized `null`) when there is no turn-1 baseline to estimate from.
    pub estimated: Option<RadarEstimated>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarContextBreakdown {
    pub used_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
    pub rows: Vec<RadarContextRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarContextRow {
    pub key: String,
    pub label: String,
    pub tokens: u64,
    pub percent: f64,
    pub count: Option<u32>,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarActivity {
    pub ts: String,
    pub kind: String,
    pub label: String,
    /// The file this row touched, as a DISPLAY path with `$HOME` folded to `~`
    /// (e.g. `~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs`). `None` for rows
    /// with no single file target (a shell run, a message, thinking).
    ///
    /// Deliberately NOT the absolute path: this struct is the exact shape a remote
    /// observer receives, and `$HOME` carries the account name. The absolute path is
    /// re-derived host-side by `reveal_activity_path` when the user clicks it, so it
    /// never has to be transmitted at all.
    pub target: Option<String>,
}

/// The single in-flight action: a tool call the harness has started and not yet
/// returned a result for. Rendered as the hero of the detail panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarAction {
    /// read / write / search / run / tool, the same closed vocabulary as `RadarActivity.kind`.
    pub kind: String,
    /// The tool as the harness named it (`Edit`, `Bash`, `exec_command`).
    pub tool: String,
    /// A short human label, e.g. `Edit agent.rs`.
    pub label: String,
    /// Display path (`~`-folded) of the file being touched, when there is exactly one.
    pub target: Option<String>,
    pub started_at: String,
    /// Milliseconds this call has been outstanding as of `RadarState.generated_at`.
    /// Lets the FACE age a long-running action without re-deriving clock skew.
    pub elapsed_ms: u64,
}

/// Membership in a named agent team. Claude Code writes a roster per team; Codex has
/// no team concept, so this stays `None` there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadarTeam {
    /// Stable team key, e.g. `session-f3e4ef77`.
    pub id: String,
    /// Human team name from the roster.
    pub name: String,
    /// This agent's own name within the team (`ClaudeFormat`), which is what the
    /// operator actually recognises, unlike a positional `subagent 3`.
    pub member_name: Option<String>,
    /// The agent type the team recorded for this member (`Explore`, `general-purpose`).
    pub member_type: Option<String>,
    /// Total roster size, including the lead.
    pub member_count: u32,
    /// True when this agent is the team lead.
    pub is_lead: bool,
}

/// The full live forest, emitted as event `radar_state` and returned by the
/// `get_radar_state` command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarState {
    pub generated_at: String,
    pub agents: Vec<RadarAgent>,
}
