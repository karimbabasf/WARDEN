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
//! Set `WARDEN_HUD_AUTO_AWAIT=0` to turn the whole behaviour off.

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
fn announced() -> &'static Mutex<HashSet<String>> {
    static ANNOUNCED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ANNOUNCED.get_or_init(|| Mutex::new(HashSet::new()))
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

/// [`newly_awaiting`]'s core, over the awaiting id set alone.
///
/// Split out so the rule is testable without building a whole forest: what it decides
/// depends on nothing about an agent except its id and whether it is awaiting.
fn fold_awaiting(now: HashSet<String>) -> Vec<String> {
    let Ok(mut seen) = announced().lock() else {
        return Vec::new();
    };
    let mut fresh: Vec<String> = now.difference(&seen).cloned().collect();
    fresh.sort();
    *seen = now;
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
        let _ = fold_awaiting(set(&[])); // start from a known state

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
