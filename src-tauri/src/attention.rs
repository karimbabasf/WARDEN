//! "Something needs you": the rule that decides when the menu-bar HUD opens itself.
//!
//! The HUD answers one question in half a second — does anything need me right now —
//! and until now the operator had to ask it. An agent that stops on a question is the
//! one state where waiting to be asked is the wrong way round: nobody is watching the
//! menu bar at the moment an agent blocks, which is exactly when the answer changed.
//!
//! Three properties keep an auto-open from becoming the thing you turn off:
//!
//! * It fires on the TRANSITION into awaiting, never on the level. A red globe that is
//!   still red on the next recompute has already been announced; re-announcing it once
//!   a second is a strobe, not a notification.
//! * It never steals the keyboard. The window is shown WITHOUT focus (see
//!   `lib.rs::show_hud`), so a summon while the operator is mid-command in a terminal
//!   swallows no keystrokes. Hovering it is what hands it focus, and that is a
//!   deliberate act.
//! * It is a notification, so it leaves on its own. The frontend starts a linger clock
//!   on an auto summon and dismisses when it runs out, unless the pointer arrived.
//!
//! WHICH SURFACE SPEAKS. When boring.notch is attached (it holds `bridge.rs`'s
//! `/attention` stream for its whole life), the alert goes to the NOTCH and this panel
//! stays down. Two surfaces saying the same thing, one of them covering the other, is
//! worse than either alone, and the choice is made by a held connection rather than by
//! a setting that could disagree with what is actually on screen.
//!
//! Set `WARDEN_HUD_AUTO_AWAIT=0` to turn the whole behaviour off, on both surfaces.

use crate::radar::RadarState;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// The set of agents that were already awaiting the last time we looked, so an agent
/// only ever announces itself once per block.
///
/// Process-global rather than threaded through the recompute, because it is a property
/// of THIS running app's notification history and has no meaning to a caller. Entries
/// leave when the agent stops awaiting (it answered, or it died), which is what makes a
/// second block on the same agent announce again.
///
/// `None` means WARDEN has not seen a forest yet. That is NOT the same as an empty set:
/// see [`fold_awaiting`] for why the first one has to stay silent.
fn announced() -> &'static Mutex<Option<HashSet<String>>> {
    static ANNOUNCED: OnceLock<Mutex<Option<HashSet<String>>>> = OnceLock::new();
    ANNOUNCED.get_or_init(|| Mutex::new(None))
}

/// Is the auto-open enabled? `WARDEN_HUD_AUTO_AWAIT=0` turns it off.
pub fn auto_open_enabled() -> bool {
    !matches!(
        std::env::var("WARDEN_HUD_AUTO_AWAIT").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

/// Fold a fresh forest into the announced set and report whether an agent has JUST
/// started waiting on the operator.
pub fn newly_awaiting(state: &RadarState) -> Vec<String> {
    fold_awaiting(
        state
            .agents
            .iter()
            .filter(|a| a.status == "awaiting")
            .map(|a| a.id.clone())
            .collect(),
    )
}

/// How an agent is NAMED to a host that cannot see our types.
///
/// Mirrors `hudDisplayName` in `web/viz/views/hud/hudSort.ts` by hand, in the same
/// order, because the notch must say the same word about an agent that its own globes
/// are labelled with. A raw id would be honest and useless.
fn display_name(a: &crate::radar::RadarAgent) -> String {
    for candidate in [
        a.nickname.as_deref(),
        Some(a.label.as_str()),
        a.title.as_deref(),
        a.cwd.as_deref(),
    ] {
        match candidate {
            Some(s) if !s.trim().is_empty() => return s.to_string(),
            _ => {}
        }
    }
    a.id.chars().take(8).collect()
}

/// The alert an external host is handed: WHO just blocked, and how many are waiting.
///
/// A small closed shape, not a radar frame. The host renders a line and opens a panel;
/// giving it the forest would make it model one. `reason` is already the closed
/// `question | approval | input` vocabulary (see `radar::awaiting`), so nothing a
/// harness typed reaches this payload.
pub fn alert_payload(state: &RadarState, fresh: &[String]) -> serde_json::Value {
    let fresh_set: HashSet<&str> = fresh.iter().map(String::as_str).collect();
    let agents: Vec<serde_json::Value> = state
        .agents
        .iter()
        .filter(|a| fresh_set.contains(a.id.as_str()))
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": display_name(a),
                "harness": a.harness,
                "reason": a.awaiting_reason,
            })
        })
        .collect();
    serde_json::json!({
        "agents": agents,
        // EVERY agent waiting, not just the new one. "3 agents need you" is the line the
        // operator acts on; the transition is only what decides whether to speak.
        "awaiting": state.agents.iter().filter(|a| a.status == "awaiting").count(),
    })
}

/// [`newly_awaiting`]'s core, over the awaiting id set alone.
///
/// Split out so the rule is testable without building a whole forest: what it decides
/// depends on nothing about an agent except its id and whether it is awaiting.
fn fold_awaiting(now: HashSet<String>) -> Vec<String> {
    let Ok(mut seen) = announced().lock() else {
        return Vec::new();
    };
    // THE FIRST FOREST NEVER ANNOUNCES. An agent that was already waiting before WARDEN
    // launched has not just started waiting: it is the state of the world at boot, and
    // "here is everything that was already true" is not a notification.
    //
    // This is also what keeps the login case honest. The first recompute lands about
    // 85ms after start, long before the HUD's webview has loaded and subscribed, so a
    // summon there showed a window the frontend never learned to draw: an empty panel
    // that then blocked every later summon because the window counted as already up.
    let Some(previous) = seen.as_ref() else {
        *seen = Some(now);
        return Vec::new();
    };
    let mut fresh: Vec<String> = now.difference(previous).cloned().collect();
    fresh.sort();
    *seen = Some(now);
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// One test, not three: the announced set is process-global (it is this app's
    /// notification history, not a value a caller owns), so parallel test threads
    /// sharing it would flake. The sequence below is the whole rule in order.
    #[test]
    fn announces_transitions_into_awaiting_and_only_transitions() {
        // The FIRST forest is the state of the world, not news, whatever is in it.
        assert!(
            fold_awaiting(set(&["already-waiting-before-we-launched"])).is_empty(),
            "an agent that was already blocked at boot has not just blocked"
        );
        let _ = fold_awaiting(set(&[])); // and now to a known empty state

        assert!(fold_awaiting(set(&[])).is_empty(), "nothing awaiting, nothing to say");

        assert_eq!(
            fold_awaiting(set(&["a"])),
            vec!["a".to_string()],
            "the transition into awaiting announces"
        );
        assert!(
            fold_awaiting(set(&["a"])).is_empty(),
            "still awaiting is not a new event; announcing it again would strobe the panel"
        );

        assert_eq!(
            fold_awaiting(set(&["a", "b"])),
            vec!["b".to_string()],
            "only the newcomer announces, not the agent already on the board"
        );

        assert!(fold_awaiting(set(&[])).is_empty(), "both answered");
        assert_eq!(
            fold_awaiting(set(&["a"])),
            vec!["a".to_string()],
            "it answered and blocked again: that is a new thing needing the operator"
        );

        let _ = fold_awaiting(set(&[]));
    }
}
