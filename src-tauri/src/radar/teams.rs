//! Claude Code agent-team rosters.
//!
//! A team is the one place on disk where agents carry names a human chose (`ClaudeFormat`,
//! `BackendMap`) instead of the positional `subagent 3` the radar derives from tree shape.
//! Reading the roster is what lets the FACE label a subagent with the name its operator
//! actually recognises.
//!
//! Layout, verified against real rosters:
//! ```text
//! ~/.claude/teams/session-<8charhex>/config.json
//!   { name, createdAt, leadAgentId, leadSessionId,
//!     members: [ { agentId: "<name>@session-<hex>", name, agentType, model, cwd, ... } ] }
//! ```
//! `leadSessionId` is a full session UUID and joins straight to a stored session's
//! `external_id`. Members are joined by the `<name>` half of `agentId`, which is also the
//! filename stem of the member's own transcript
//! (`<project>/<parentSessionId>/subagents/agent-<agentId>.jsonl`).
//!
//! Read-only and best-effort by design: a malformed or half-written roster yields no team
//! rather than an error, because a missing label must never take the radar down.

use super::model::RadarTeam;
use std::collections::HashMap;
use std::path::PathBuf;

/// One member row from a roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TeamMember {
    pub name: String,
    pub agent_type: Option<String>,
}

/// One parsed roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Team {
    pub id: String,
    pub name: String,
    pub lead_session_id: String,
    pub lead_member_name: Option<String>,
    /// Member name (the `<name>` half of `agentId`) to its row.
    pub members: HashMap<String, TeamMember>,
}

impl Team {
    pub fn member_count(&self) -> u32 {
        self.members.len() as u32
    }
}

/// Every roster currently on disk, keyed by team id (`session-<hex>`).
#[derive(Debug, Clone, Default)]
pub(crate) struct TeamIndex {
    teams: Vec<Team>,
}

/// The `~/.claude/teams` root, overridable for tests via `WARDEN_CLAUDE_TEAMS`.
fn teams_root() -> PathBuf {
    std::env::var("WARDEN_CLAUDE_TEAMS")
        .ok()
        .map(|s| crate::util::expand_tilde(&s))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude/teams")
        })
}

impl TeamIndex {
    /// Load every readable roster. Never fails: an unreadable root yields an empty index.
    pub fn load() -> Self {
        Self::load_from(&teams_root())
    }

    pub fn load_from(root: &std::path::Path) -> Self {
        let mut teams = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else {
            return Self { teams };
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
                continue;
            };
            if let Some(team) = parse_roster(&id, &raw) {
                teams.push(team);
            }
        }
        Self { teams }
    }

    /// The team whose LEAD is this session, matched on the session's `external_id`.
    pub fn team_for_lead(&self, external_id: &str) -> Option<&Team> {
        self.teams
            .iter()
            .find(|t| !t.lead_session_id.is_empty() && t.lead_session_id == external_id)
    }

    /// The team owning a member transcript, resolved from that transcript's file stem
    /// (`agent-<name>@session-<hex>`), plus the member name parsed out of it.
    pub fn team_for_member_stem(&self, stem: &str) -> Option<(&Team, String)> {
        let (name, team_id) = split_agent_id(stem.strip_prefix("agent-").unwrap_or(stem))?;
        let team = self.teams.iter().find(|t| t.id == team_id)?;
        // Only claim the stem when the roster actually lists that member, so an unrelated
        // file that happens to look like an agent id cannot invent a membership.
        team.members.contains_key(&name).then_some((team, name))
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.teams.is_empty()
    }
}

/// Split `"<name>@session-<hex>"` into its member name and team id.
fn split_agent_id(agent_id: &str) -> Option<(String, String)> {
    let (name, team) = agent_id.rsplit_once('@')?;
    if name.is_empty() || team.is_empty() {
        return None;
    }
    Some((name.to_string(), team.to_string()))
}

fn parse_roster(id: &str, raw: &str) -> Option<Team> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let lead_session_id = v
        .get("leadSessionId")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(id)
        .to_string();
    let lead_member_name = v
        .get("leadAgentId")
        .and_then(|x| x.as_str())
        .and_then(split_agent_id)
        .map(|(n, _)| n);

    let mut members = HashMap::new();
    if let Some(arr) = v.get("members").and_then(|m| m.as_array()) {
        for m in arr {
            // Prefer the explicit `name`, falling back to the name half of `agentId`, so a
            // roster missing one of the two still yields a usable label.
            let from_id = m
                .get("agentId")
                .and_then(|x| x.as_str())
                .and_then(split_agent_id)
                .map(|(n, _)| n);
            let Some(member_name) = m
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or(from_id)
            else {
                continue;
            };
            let agent_type = m
                .get("agentType")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string());
            members.insert(
                member_name.clone(),
                TeamMember {
                    name: member_name,
                    agent_type,
                },
            );
        }
    }
    // A roster with no members and no lead session says nothing worth rendering.
    if members.is_empty() && lead_session_id.is_empty() {
        return None;
    }
    Some(Team {
        id: id.to_string(),
        name,
        lead_session_id,
        lead_member_name,
        members,
    })
}

/// Build the wire struct for a member (or the lead, when `member_name` is the lead's).
pub(crate) fn radar_team(team: &Team, member_name: Option<&str>, is_lead: bool) -> RadarTeam {
    let member = member_name.and_then(|n| team.members.get(n));
    RadarTeam {
        id: team.id.clone(),
        name: team.name.clone(),
        member_name: member_name.map(|s| s.to_string()),
        member_type: member.and_then(|m| m.agent_type.clone()),
        member_count: team.member_count(),
        is_lead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROSTER: &str = r#"{
        "name": "warden build",
        "leadAgentId": "team-lead@session-f3e4ef77",
        "leadSessionId": "f3e4ef77-5ee5-4aa0-a94a-fc8e296d67f4",
        "members": [
            {"agentId": "ClaudeFormat@session-f3e4ef77", "name": "ClaudeFormat", "agentType": "Explore", "model": "sonnet"},
            {"agentId": "BackendMap@session-f3e4ef77", "name": "BackendMap"}
        ]
    }"#;

    #[test]
    fn parses_a_real_shaped_roster() {
        let t = parse_roster("session-f3e4ef77", ROSTER).expect("roster parses");
        assert_eq!(t.name, "warden build");
        assert_eq!(t.lead_session_id, "f3e4ef77-5ee5-4aa0-a94a-fc8e296d67f4");
        assert_eq!(t.lead_member_name.as_deref(), Some("team-lead"));
        assert_eq!(t.member_count(), 2);
        assert_eq!(
            t.members.get("ClaudeFormat").and_then(|m| m.agent_type.as_deref()),
            Some("Explore")
        );
    }

    #[test]
    fn member_without_agent_type_still_lands() {
        let t = parse_roster("session-x", ROSTER).expect("roster parses");
        assert!(t.members.contains_key("BackendMap"));
        assert_eq!(t.members["BackendMap"].agent_type, None);
    }

    #[test]
    fn falls_back_to_the_name_half_of_agent_id() {
        let raw = r#"{"leadSessionId":"s1","members":[{"agentId":"Solo@session-ab"}]}"#;
        let t = parse_roster("session-ab", raw).expect("roster parses");
        assert!(t.members.contains_key("Solo"));
    }

    #[test]
    fn malformed_json_yields_no_team_instead_of_panicking() {
        assert!(parse_roster("session-x", "{not json").is_none());
        assert!(parse_roster("session-x", "").is_none());
    }

    #[test]
    fn empty_roster_with_no_lead_is_dropped() {
        assert!(parse_roster("session-x", r#"{"members":[]}"#).is_none());
    }

    #[test]
    fn splits_agent_ids_and_rejects_malformed_ones() {
        assert_eq!(
            split_agent_id("ClaudeFormat@session-f3e4ef77"),
            Some(("ClaudeFormat".into(), "session-f3e4ef77".into()))
        );
        assert_eq!(split_agent_id("noatsign"), None);
        assert_eq!(split_agent_id("@session-x"), None);
        assert_eq!(split_agent_id("name@"), None);
    }

    #[test]
    fn index_joins_lead_and_members_but_not_strangers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let team_dir = dir.path().join("session-f3e4ef77");
        std::fs::create_dir_all(&team_dir).expect("mkdir");
        std::fs::write(team_dir.join("config.json"), ROSTER).expect("write");

        let idx = TeamIndex::load_from(dir.path());
        assert!(!idx.is_empty());

        let lead = idx
            .team_for_lead("f3e4ef77-5ee5-4aa0-a94a-fc8e296d67f4")
            .expect("lead joins");
        assert_eq!(lead.name, "warden build");
        assert!(idx.team_for_lead("some-other-session").is_none());

        let (team, name) = idx
            .team_for_member_stem("agent-ClaudeFormat@session-f3e4ef77")
            .expect("member joins");
        assert_eq!(name, "ClaudeFormat");
        assert_eq!(team.id, "session-f3e4ef77");

        // Right shape, but not on the roster: must not invent a membership.
        assert!(idx
            .team_for_member_stem("agent-Ghost@session-f3e4ef77")
            .is_none());
        // Right member name, unknown team.
        assert!(idx
            .team_for_member_stem("agent-ClaudeFormat@session-zzzzzz")
            .is_none());
    }

    #[test]
    fn missing_root_is_an_empty_index_not_an_error() {
        let idx = TeamIndex::load_from(std::path::Path::new("/nonexistent/warden/teams"));
        assert!(idx.is_empty());
        assert!(idx.team_for_lead("anything").is_none());
    }

    #[test]
    fn radar_team_carries_the_member_identity() {
        let t = parse_roster("session-f3e4ef77", ROSTER).expect("roster parses");
        let rt = radar_team(&t, Some("ClaudeFormat"), false);
        assert_eq!(rt.member_name.as_deref(), Some("ClaudeFormat"));
        assert_eq!(rt.member_type.as_deref(), Some("Explore"));
        assert_eq!(rt.member_count, 2);
        assert!(!rt.is_lead);

        let lead = radar_team(&t, Some("team-lead"), true);
        assert!(lead.is_lead);
        // The lead is not in `members`, so it has no recorded type: absent, not invented.
        assert_eq!(lead.member_type, None);
    }
}
