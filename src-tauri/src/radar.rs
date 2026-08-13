//! RADAR: the live agent-forest collector.
//!
//! M3 assembles an ephemeral constellation of currently-open Claude/Codex agents
//! and their subagents from local files, computes per-agent context size + honest
//! composition, and emits a `radar_state` event. The forest is recomputed from
//! files on each FS event — no heavy persistence (see the M3 design spec).
//!
//! This module is a slim façade over cohesive submodules:
//! * [`model`] — the shared `radar_state` data types (leaf; everything depends on it).
//! * [`hierarchy`] — pure resolvers that link subagents to their parents
//!   (Claude `subagents/` + `toolUseId`; Codex `parent_thread_id`).
//! * [`liveness`] — open/working/awaiting/idle/closed partition (pure core + thin syscall).
//! * [`awaiting`] — is the agent stopped on the OPERATOR? (pure; the third globe state).
//! * [`composition`] — exact + estimated context composition (pure).
//! * [`status`] — per-session working/awaiting/idle/terminated verdict from conversation state.
//! * [`context`] — context-window breakdown + cost estimation.
//! * `pricing` — model price/context-window ground truth (a data table, no
//!   `radar_state` types of its own; used by `context` and `composition`).
//! * [`identity`] — agent naming/identity + subagent-termination decisions.
//! * [`agent`] — per-agent construction + recent-activity tailing.
//! * [`teams`] — Claude agent-team rosters, the source of real member names.
//! * [`assemble`] — the pure top-level forest join ([`assemble()`]).
//! * [`live`] — live transcript refresh + open-session scanning ([`recompute_radar_state`]).

pub mod awaiting;
pub mod composition;
pub mod hierarchy;
pub mod liveness;

mod agent;
mod assemble;
mod context;
mod identity;
mod live;
mod model;
mod pricing;
mod status;
mod teams;

// ── public API (kept byte-identical to the pre-split `crate::radar` surface) ──
pub use assemble::assemble;
pub use awaiting::AwaitingReason;
pub use live::{recompute_radar_state, refresh_live_context};
pub use liveness::{AgentStatus, LiveSession};
pub use model::{
    RadarAction, RadarActivity, RadarAgent, RadarComposition, RadarContextBreakdown,
    RadarContextRow, RadarEstimated, RadarExact, RadarState, RadarTeam,
};
