//! The redaction boundary for remote observation.
//!
//! A remote observer never receives a [`RadarAgent`]. It receives an [`ObservedAgent`],
//! built only by [`project`], and the two types share no fields by accident.
//!
//! This is an ALLOWLIST PROJECTION, not a scrubber, and the distinction is the whole
//! point. A scrubber walks the real struct removing known-bad fields, so a field added to
//! `RadarAgent` next quarter ships to observers until someone remembers to scrub it. A
//! projection copies named fields onto a separate type, so a new field is invisible by
//! default and the compiler stays silent because `project` simply never mentions it.
//! Safe-by-default is therefore a property of the shape, not of anyone's diligence.
//!
//! What the local radar actually carries, and why each item below is redacted:
//! * `label` is the session's FIRST USER PROMPT (or a subagent's task brief).
//! * `title` is a human-written session name.
//! * `cwd` is the project folder basename, which is often a client or codename.
//! * `recent_activity[].label` is literal shell command text, including paths.
//! * `recent_activity[].target` and `current_action.target` are file paths.
//! * `est_cost_usd` is the host's spend.
//!
//! None of those have a field on `ObservedAgent` at all. An observer sees the SHAPE of
//! the work (how many agents, nested how, how busy, how full, what kind of action just
//! fired) and nothing that names it.

use crate::radar::{RadarAgent, RadarState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// How much a grant is allowed to reveal.
///
/// v1 ships exactly one variant. Widening what crosses the wire therefore requires
/// adding a variant here, which is a visible, reviewable diff rather than a config flag
/// someone can flip without review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Structure, status and numbers. No names, no paths, no prompts, no costs.
    #[default]
    Shapes,
}

/// Global caps applied regardless of profile, so a pathological host state cannot turn
/// into a pathological frame.
const MAX_AGENTS: usize = 64;
const MAX_ACTIVITY: usize = 12;

/// `role` values safe to send verbatim. These are harness-defined agent types, not user
/// content. Anything unrecognised is dropped rather than guessed at, because a custom
/// agent name is chosen by the user and can name the project.
const ROLE_ALLOWLIST: &[&str] = &[
    "Explore",
    "Plan",
    "general-purpose",
    "code-reviewer",
    "statusline-setup",
    "claude",
    "claude-code-guide",
];

/// `contextBreakdown.rows[].key` values that exist in the radar's own fixed vocabulary
/// (see `radar/context.rs`). A row whose key is not here is dropped entirely, so a future
/// content-derived row cannot ride out on an unknown key.
const CONTEXT_KEY_ALLOWLIST: &[&str] = &[
    "context",
    "messages",
    "reasoning",
    "skills",
    "base_instructions",
    "custom_agents",
    "custom_tools",
    "free_space",
    "function_tools",
    "mcp_tools",
    "mcp_tools_deferred",
    "memory_files",
    "pending_tail",
    "system_prompt",
    "system_tools",
    "system_tools_deferred",
];

/// Tool names safe to send. A tool name is usually a harness builtin, but an MCP tool is
/// named by whoever wrote the server (`mcp__acme_internal__deploy`), so the default is to
/// report the CATEGORY rather than the name.
const TOOL_ALLOWLIST: &[&str] = &[
    "Read", "Write", "Edit", "MultiEdit", "NotebookEdit", "NotebookRead", "Grep", "Glob", "LS",
    "Bash", "BashOutput", "Task", "WebFetch", "WebSearch", "TodoWrite", "exec_command",
    "apply_patch", "web_search", "update_plan",
];

/// One agent as a remote observer sees it. Every field here is either numeric, drawn from
/// a closed vocabulary, or a salted hash. There is deliberately no field capable of
/// carrying free text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedAgent {
    /// Salted hash of the real id. Needed for node identity and frame diffing; the raw id
    /// maps to a transcript path on disk. The per-grant salt stops two observers
    /// correlating the same session across grants.
    pub id: String,
    pub parent_id: Option<String>,
    pub harness: String,
    pub depth: u32,
    pub status: String,
    /// Stable per-grant pseudonym for the project folder (`project A`), which preserves
    /// the "three agents in one repo" grouping the radar needs without naming the repo.
    pub project: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub context_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
    pub context_rows: Vec<ObservedContextRow>,
    pub child_count: u32,
    /// Seconds since this agent started, as of the frame. An absolute wall-clock start
    /// fingerprints the host's working day; an age animates just as well.
    pub age_secs: u64,
    /// Kind only (read / write / search / run / tool) plus category and elapsed time.
    pub current_action: Option<ObservedAction>,
    /// Kinds and relative offsets. No labels, no paths.
    pub recent_activity: Vec<ObservedActivity>,
    pub team: Option<ObservedTeam>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedContextRow {
    pub key: String,
    pub tokens: u64,
    pub percent_x100: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedAction {
    pub kind: String,
    /// The tool name when it is a known harness builtin, otherwise `"tool"`.
    pub tool: String,
    pub elapsed_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedActivity {
    pub kind: String,
    /// Seconds before the frame timestamp, not an absolute time.
    pub secs_ago: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedTeam {
    /// Salted hash of the team id, so team membership groups without naming the team.
    pub id: String,
    pub member_count: u32,
    pub is_lead: bool,
    pub member_type: Option<String>,
}

/// A whole frame as an observer sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedState {
    pub generated_at: String,
    pub agents: Vec<ObservedAgent>,
    /// True when agents were dropped by [`MAX_AGENTS`], so the observer can say "showing
    /// 64 of 91" instead of silently under-reporting.
    pub truncated: bool,
}

/// Salted, truncated hash used for every id that crosses the wire.
///
/// Uses sha2, already a dependency for content hashing, rather than adding a crate purely
/// for this. The security property needed is preimage resistance on a 16-byte random
/// salt, which sha256 provides comfortably; speed is irrelevant at 64 agents per second.
fn salted_id(raw: &str, salt: &[u8; 16]) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(raw.as_bytes());
    let out = h.finalize();
    hex::encode(&out[..8])
}

/// Seconds between two rfc3339 stamps, clamped at zero. A malformed stamp yields 0 rather
/// than failing the frame: a wrong age is a cosmetic bug, a dropped frame is an outage.
fn secs_between(later: &str, earlier: &str) -> u64 {
    let (Ok(l), Ok(e)) = (
        chrono::DateTime::parse_from_rfc3339(later),
        chrono::DateTime::parse_from_rfc3339(earlier),
    ) else {
        return 0;
    };
    (l - e).num_seconds().max(0) as u64
}

fn allowlisted(value: &str, list: &[&str]) -> bool {
    list.contains(&value)
}

/// Project a whole radar frame for one grant.
///
/// `salt` is per-grant: the same session hashes differently for different observers, so
/// two observers cannot compare notes to correlate the host's sessions.
pub fn project_state(state: &RadarState, profile: Profile, salt: &[u8; 16]) -> ObservedState {
    // Assign project pseudonyms in first-appearance order. Deterministic for a given
    // frame, and stable across frames because the radar iterates sessions in source order.
    let mut project_names: HashMap<String, String> = HashMap::new();
    let truncated = state.agents.len() > MAX_AGENTS;
    let agents = state
        .agents
        .iter()
        .take(MAX_AGENTS)
        .map(|a| project(a, profile, salt, &state.generated_at, &mut project_names))
        .collect();
    ObservedState {
        generated_at: state.generated_at.clone(),
        agents,
        truncated,
    }
}

/// The single function that decides what leaves this machine.
pub fn project(
    agent: &RadarAgent,
    profile: Profile,
    salt: &[u8; 16],
    frame_ts: &str,
    project_names: &mut HashMap<String, String>,
) -> ObservedAgent {
    let Profile::Shapes = profile;

    let project = agent.cwd.as_ref().map(|cwd| {
        let next = project_names.len();
        project_names
            .entry(cwd.clone())
            .or_insert_with(|| format!("project {}", pseudonym(next)))
            .clone()
    });

    let context_rows = agent
        .context_breakdown
        .rows
        .iter()
        .filter(|r| allowlisted(&r.key, CONTEXT_KEY_ALLOWLIST))
        .map(|r| ObservedContextRow {
            key: r.key.clone(),
            tokens: r.tokens,
            // Integer hundredths: enough for a bar, and it cannot smuggle a string.
            percent_x100: (r.percent * 100.0).clamp(0.0, 10_000.0) as u32,
        })
        .collect();

    let current_action = agent.current_action.as_ref().map(|a| ObservedAction {
        kind: a.kind.clone(),
        tool: if allowlisted(&a.tool, TOOL_ALLOWLIST) {
            a.tool.clone()
        } else {
            "tool".to_string()
        },
        elapsed_secs: a.elapsed_ms / 1000,
    });

    let recent_activity = agent
        .recent_activity
        .iter()
        .take(MAX_ACTIVITY)
        .map(|r| ObservedActivity {
            kind: r.kind.clone(),
            secs_ago: secs_between(frame_ts, &r.ts),
        })
        .collect();

    let team = agent.team.as_ref().map(|t| ObservedTeam {
        id: salted_id(&t.id, salt),
        member_count: t.member_count,
        is_lead: t.is_lead,
        member_type: t
            .member_type
            .as_ref()
            .filter(|v| allowlisted(v, ROLE_ALLOWLIST))
            .cloned(),
    });

    ObservedAgent {
        id: salted_id(&agent.id, salt),
        parent_id: agent.parent_id.as_ref().map(|p| salted_id(p, salt)),
        harness: agent.harness.clone(),
        depth: agent.depth,
        status: agent.status.clone(),
        project,
        role: agent
            .role
            .as_ref()
            .filter(|v| allowlisted(v, ROLE_ALLOWLIST))
            .cloned(),
        model: agent.model.clone(),
        context_tokens: agent.context_tokens,
        max_tokens: agent.max_tokens,
        fill_pct: agent.fill_pct,
        context_rows,
        child_count: agent.child_count,
        age_secs: secs_between(frame_ts, &agent.started_at),
        current_action,
        recent_activity,
        team,
    }
}

/// `0 -> A`, `25 -> Z`, `26 -> AA`. Keeps pseudonyms short and human-sayable.
fn pseudonym(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            return s;
        }
        n = n / 26 - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radar::{
        RadarAction, RadarActivity, RadarComposition, RadarContextBreakdown, RadarContextRow,
        RadarExact, RadarTeam,
    };

    const SALT: [u8; 16] = [7u8; 16];
    const FRAME: &str = "2026-07-26T12:00:30+00:00";

    /// Every free-text field carries a unique canary. If any of these strings appears in
    /// the serialized frame, a leak has shipped.
    fn canary_agent() -> RadarAgent {
        RadarAgent {
            id: "CANARY_ID".into(),
            harness: "claude".into(),
            origin: Some("CANARY_ORIGIN".into()),
            parent_id: Some("CANARY_PARENT".into()),
            depth: 1,
            label: "CANARY_PROMPT_TEXT".into(),
            nickname: Some("CANARY_NICK".into()),
            cwd: Some("CANARY_PROJECT".into()),
            role: Some("CANARY_ROLE".into()),
            model: Some("claude-opus-5".into()),
            title: Some("CANARY_TITLE".into()),
            current_action: Some(RadarAction {
                kind: "write".into(),
                tool: "CANARY_TOOL".into(),
                label: "CANARY_ACTION_LABEL".into(),
                target: Some("/Users/karimbaba/CANARY_PATH.rs".into()),
                started_at: "2026-07-26T12:00:00+00:00".into(),
                elapsed_ms: 30_000,
            }),
            team: Some(RadarTeam {
                id: "CANARY_TEAM_ID".into(),
                name: "CANARY_TEAM_NAME".into(),
                member_name: Some("CANARY_MEMBER".into()),
                member_type: Some("CANARY_MEMBER_TYPE".into()),
                member_count: 3,
                is_lead: false,
            }),
            status: "working".into(),
            context_tokens: 1000,
            max_tokens: 4000,
            fill_pct: 25.0,
            context_breakdown: RadarContextBreakdown {
                used_tokens: 1000,
                max_tokens: 4000,
                fill_pct: 25.0,
                rows: vec![
                    RadarContextRow {
                        key: "messages".into(),
                        label: "Messages".into(),
                        tokens: 900,
                        percent: 22.5,
                        count: None,
                        muted: false,
                    },
                    RadarContextRow {
                        key: "CANARY_ROW_KEY".into(),
                        label: "CANARY_ROW_LABEL".into(),
                        tokens: 100,
                        percent: 2.5,
                        count: None,
                        muted: false,
                    },
                ],
            },
            composition: RadarComposition {
                exact: RadarExact {
                    cache_read: 1,
                    fresh: 2,
                    output: 3,
                },
                estimated: None,
            },
            recent_activity: vec![RadarActivity {
                ts: "2026-07-26T12:00:00+00:00".into(),
                kind: "run".into(),
                label: "sed -n CANARY_CMD /Users/karimbaba/secret".into(),
                target: Some("/Users/karimbaba/CANARY_ACTIVITY_PATH.rs".into()),
            }],
            child_count: 0,
            started_at: "2026-07-26T11:59:00+00:00".into(),
            est_cost_usd: Some(12.34),
        }
    }

    const CANARIES: &[&str] = &[
        "CANARY_ID",
        "CANARY_ORIGIN",
        "CANARY_PARENT",
        "CANARY_PROMPT_TEXT",
        "CANARY_NICK",
        "CANARY_PROJECT",
        "CANARY_ROLE",
        "CANARY_TITLE",
        "CANARY_TOOL",
        "CANARY_ACTION_LABEL",
        "CANARY_PATH",
        "CANARY_TEAM_ID",
        "CANARY_TEAM_NAME",
        "CANARY_MEMBER",
        "CANARY_MEMBER_TYPE",
        "CANARY_ROW_KEY",
        "CANARY_ROW_LABEL",
        "CANARY_CMD",
        "CANARY_ACTIVITY_PATH",
    ];

    fn frame_json() -> String {
        let state = RadarState {
            generated_at: FRAME.into(),
            agents: vec![canary_agent()],
        };
        serde_json::to_string(&project_state(&state, Profile::Shapes, &SALT))
            .expect("frame serializes")
    }

    /// THE guarantee. This is what catches the field someone adds to `RadarAgent` next
    /// quarter and forgets is sensitive.
    #[test]
    fn no_canary_survives_projection() {
        let json = frame_json();
        for c in CANARIES {
            assert!(
                !json.contains(c),
                "LEAK: {c} reached the wire. Frame was: {json}"
            );
        }
    }

    #[test]
    fn no_home_directory_prefix_crosses_the_wire() {
        let json = frame_json();
        assert!(!json.contains("/Users/"), "LEAK: absolute path in {json}");
        assert!(!json.contains("karimbaba"), "LEAK: username in {json}");
        assert!(!json.contains('~'), "LEAK: home-folded path in {json}");
    }

    #[test]
    fn the_host_spend_never_leaves() {
        assert!(!frame_json().contains("12.34"));
    }

    #[test]
    fn structure_and_status_do_survive() {
        // The feature has to actually work: shape, status and numbers must get through.
        let json = frame_json();
        assert!(json.contains("\"harness\":\"claude\""));
        assert!(json.contains("\"status\":\"working\""));
        assert!(json.contains("\"contextTokens\":1000"));
        assert!(json.contains("\"depth\":1"));
        assert!(json.contains("\"kind\":\"write\""));
        assert!(json.contains("\"kind\":\"run\""));
        assert!(json.contains("\"memberCount\":3"));
        assert!(json.contains("\"model\":\"claude-opus-5\""));
    }

    #[test]
    fn unknown_role_tool_and_context_key_are_dropped_not_passed() {
        let mut names = HashMap::new();
        let o = project(&canary_agent(), Profile::Shapes, &SALT, FRAME, &mut names);
        assert_eq!(o.role, None, "unrecognised role must drop");
        assert_eq!(
            o.current_action.as_ref().map(|a| a.tool.as_str()),
            Some("tool"),
            "unrecognised tool must collapse to its category"
        );
        assert_eq!(o.context_rows.len(), 1, "unknown context key must drop");
        assert_eq!(o.context_rows[0].key, "messages");
        assert_eq!(
            o.team.as_ref().and_then(|t| t.member_type.clone()),
            None,
            "unrecognised member type must drop"
        );
    }

    #[test]
    fn known_role_and_tool_do_pass() {
        let mut a = canary_agent();
        a.role = Some("Explore".into());
        if let Some(act) = a.current_action.as_mut() {
            act.tool = "Edit".into();
        }
        let mut names = HashMap::new();
        let o = project(&a, Profile::Shapes, &SALT, FRAME, &mut names);
        assert_eq!(o.role.as_deref(), Some("Explore"));
        assert_eq!(o.current_action.map(|x| x.tool), Some("Edit".into()));
    }

    #[test]
    fn ids_are_salted_so_two_grants_cannot_be_correlated() {
        let a = canary_agent();
        let (mut n1, mut n2) = (HashMap::new(), HashMap::new());
        let o1 = project(&a, Profile::Shapes, &[1u8; 16], FRAME, &mut n1);
        let o2 = project(&a, Profile::Shapes, &[2u8; 16], FRAME, &mut n2);
        assert_ne!(o1.id, o2.id, "same session must hash differently per grant");
        // Stable within a grant, or the observer's node identity flickers every frame.
        let mut n3 = HashMap::new();
        let o3 = project(&a, Profile::Shapes, &[1u8; 16], FRAME, &mut n3);
        assert_eq!(o1.id, o3.id);
        assert_ne!(o1.id, "CANARY_ID");
    }

    #[test]
    fn parent_hashes_match_child_hashes_so_the_tree_still_joins() {
        // The edges are the whole visualization: a parent's id must hash to the same
        // value as the child's parentId, or the observer renders a flat pile.
        let mut parent = canary_agent();
        parent.id = "P".into();
        parent.parent_id = None;
        let mut child = canary_agent();
        child.id = "C".into();
        child.parent_id = Some("P".into());
        let state = RadarState {
            generated_at: FRAME.into(),
            agents: vec![parent, child],
        };
        let o = project_state(&state, Profile::Shapes, &SALT);
        assert_eq!(o.agents[1].parent_id.as_ref(), Some(&o.agents[0].id));
    }

    #[test]
    fn same_project_folder_shares_one_pseudonym_across_agents() {
        let mut a = canary_agent();
        a.id = "a".into();
        let mut b = canary_agent();
        b.id = "b".into();
        let mut c = canary_agent();
        c.id = "c".into();
        c.cwd = Some("OTHER_PROJECT".into());
        let state = RadarState {
            generated_at: FRAME.into(),
            agents: vec![a, b, c],
        };
        let o = project_state(&state, Profile::Shapes, &SALT);
        assert_eq!(o.agents[0].project, o.agents[1].project);
        assert_ne!(o.agents[0].project, o.agents[2].project);
        assert_eq!(o.agents[0].project.as_deref(), Some("project A"));
        assert_eq!(o.agents[2].project.as_deref(), Some("project B"));
    }

    #[test]
    fn timestamps_become_relative_ages() {
        let mut names = HashMap::new();
        let o = project(&canary_agent(), Profile::Shapes, &SALT, FRAME, &mut names);
        assert_eq!(o.age_secs, 90, "started 11:59:00, frame 12:00:30");
        assert_eq!(o.recent_activity[0].secs_ago, 30);
        assert_eq!(o.current_action.as_ref().expect("action").elapsed_secs, 30);
        // No absolute stamp for any agent-level time.
        let json = serde_json::to_string(&o).expect("serializes");
        assert!(!json.contains("11:59"));
    }

    #[test]
    fn malformed_timestamps_yield_zero_rather_than_breaking_the_frame() {
        let mut a = canary_agent();
        a.started_at = "not a timestamp".into();
        let mut names = HashMap::new();
        let o = project(&a, Profile::Shapes, &SALT, FRAME, &mut names);
        assert_eq!(o.age_secs, 0);
    }

    #[test]
    fn agent_and_activity_counts_are_capped_and_truncation_is_reported() {
        let agents: Vec<RadarAgent> = (0..MAX_AGENTS + 5)
            .map(|i| {
                let mut a = canary_agent();
                a.id = format!("agent-{i}");
                a
            })
            .collect();
        let state = RadarState {
            generated_at: FRAME.into(),
            agents,
        };
        let o = project_state(&state, Profile::Shapes, &SALT);
        assert_eq!(o.agents.len(), MAX_AGENTS);
        assert!(o.truncated, "dropping agents must be reported, not silent");

        let mut a = canary_agent();
        a.recent_activity = (0..MAX_ACTIVITY + 10)
            .map(|_| RadarActivity {
                ts: "2026-07-26T12:00:00+00:00".into(),
                kind: "run".into(),
                label: "CANARY_CMD".into(),
                target: None,
            })
            .collect();
        let mut names = HashMap::new();
        let o = project(&a, Profile::Shapes, &SALT, FRAME, &mut names);
        assert_eq!(o.recent_activity.len(), MAX_ACTIVITY);
    }

    #[test]
    fn not_truncated_when_under_the_cap() {
        let state = RadarState {
            generated_at: FRAME.into(),
            agents: vec![canary_agent()],
        };
        assert!(!project_state(&state, Profile::Shapes, &SALT).truncated);
    }

    #[test]
    fn pseudonyms_extend_past_z() {
        assert_eq!(pseudonym(0), "A");
        assert_eq!(pseudonym(25), "Z");
        assert_eq!(pseudonym(26), "AA");
        assert_eq!(pseudonym(27), "AB");
    }

    /// Locks the exact wire surface. If someone adds a field to `ObservedAgent`, this
    /// fails and forces a deliberate decision about whether it may leave the machine.
    /// Doubles as documentation: this list IS what an observer receives.
    #[test]
    fn the_wire_surface_is_exactly_these_fields() {
        let mut names = HashMap::new();
        let o = project(&canary_agent(), Profile::Shapes, &SALT, FRAME, &mut names);
        let v = serde_json::to_value(&o).expect("serializes");
        let mut keys: Vec<&str> = v
            .as_object()
            .expect("agent is a json object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "ageSecs",
                "childCount",
                "contextRows",
                "contextTokens",
                "currentAction",
                "depth",
                "fillPct",
                "harness",
                "id",
                "maxTokens",
                "model",
                "parentId",
                "project",
                "recentActivity",
                "role",
                "status",
                "team",
            ]
        );
    }

    #[test]
    fn an_empty_forest_projects_to_an_empty_frame() {
        let state = RadarState {
            generated_at: FRAME.into(),
            agents: vec![],
        };
        let o = project_state(&state, Profile::Shapes, &SALT);
        assert!(o.agents.is_empty());
        assert!(!o.truncated);
        assert_eq!(o.generated_at, FRAME);
    }
}
