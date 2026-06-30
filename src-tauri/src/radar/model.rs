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
    pub role: Option<String>,
    pub model: Option<String>,
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
    pub fresh: u64,
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
}

/// The full live forest, emitted as event `radar_state` and returned by the
/// `get_radar_state` command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarState {
    pub generated_at: String,
    pub agents: Vec<RadarAgent>,
}
