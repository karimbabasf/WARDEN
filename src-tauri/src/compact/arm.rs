//! The armed set: WARDEN-side persistence for "compact this agent when it
//! finishes".
//!
//! Arming writes a row HERE and nothing else. No request is pre-delivered, no
//! file outside WARDEN's own database is touched, and the agent is not told
//! anything. That single choice is what makes cancel free, instant, and total:
//! cancelling deletes the row, and because nothing was ever sent there is
//! nothing to retract. The alternative (hand the agent a request and ask it to
//! hold it) would make cancellation a negotiation with a process that might be
//! mid-turn, offline, or already acting on it.
//!
//! The SQL lives next to the types it round-trips, following `observe::grants`:
//! the DDL is in `store.rs` with the rest of the schema, and every read and write
//! of the table is in this one file so the lifecycle is auditable in one place.

use super::model::{ArmRecord, ArmState, DeliveryMode, IdleSource};
use crate::store::Store;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

const COLUMNS: &str = "agent_id,session_id,pid,harness,mode,mode_reason,idle_source,label,\
     baseline_status,armed_at,state,fired_at,detail";

fn to_ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

/// Epoch milliseconds back to a timestamp. A row we wrote always converts; the
/// fallback exists so a corrupted value degrades to the epoch instead of
/// panicking inside a watcher thread.
fn from_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArmRecord> {
    Ok(ArmRecord {
        agent_id: r.get(0)?,
        session_id: r.get(1)?,
        pid: r.get::<_, Option<i64>>(2)?.map(|p| p as u32),
        harness: r.get(3)?,
        mode: DeliveryMode::from_wire(&r.get::<_, String>(4)?),
        mode_reason: r.get(5)?,
        idle_source: IdleSource::from_wire(&r.get::<_, String>(6)?),
        label: r.get(7)?,
        baseline_status: r.get(8)?,
        armed_at: from_ms(r.get(9)?),
        state: ArmState::from_wire(&r.get::<_, String>(10)?),
        fired_at: r.get::<_, Option<i64>>(11)?.map(from_ms),
        detail: r.get(12)?,
    })
}

/// Arm (or re-arm) one agent. Keyed by `agent_id`, so clicking twice replaces
/// rather than duplicating, and re-arming a fired record resets it cleanly.
pub(crate) fn put(store: &Store, rec: &ArmRecord) -> Result<()> {
    store.with_conn(|c| {
        c.execute(
            &format!(
                "INSERT OR REPLACE INTO compact_arms({COLUMNS}) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)"
            ),
            params![
                rec.agent_id,
                rec.session_id,
                rec.pid.map(|p| p as i64),
                rec.harness,
                rec.mode.as_str(),
                rec.mode_reason,
                rec.idle_source.as_str(),
                rec.label,
                rec.baseline_status,
                to_ms(rec.armed_at),
                rec.state.as_str(),
                rec.fired_at.map(to_ms),
                rec.detail,
            ],
        )?;
        Ok(())
    })
}

/// Disarm. Returns whether a row was actually removed, so the command can tell
/// the caller "there was nothing armed" instead of silently succeeding.
pub(crate) fn delete(store: &Store, agent_id: &str) -> Result<bool> {
    store.with_conn(|c| {
        let n = c.execute("DELETE FROM compact_arms WHERE agent_id=?", params![agent_id])?;
        Ok(n > 0)
    })
}

pub(crate) fn get(store: &Store, agent_id: &str) -> Result<Option<ArmRecord>> {
    store.with_conn(|c| {
        c.prepare(&format!(
            "SELECT {COLUMNS} FROM compact_arms WHERE agent_id=?"
        ))?
        .query_row(params![agent_id], row_to_record)
        .optional()
        .map_err(Into::into)
    })
}

/// Every armed record, newest first.
pub(crate) fn all(store: &Store) -> Result<Vec<ArmRecord>> {
    store.with_conn(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLUMNS} FROM compact_arms ORDER BY armed_at DESC"
        ))?;
        let rows = st.query_map([], row_to_record)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    })
}

/// Only the records still waiting on idle. The watcher walks these and nothing
/// else, so a delivered or expired row is inert history.
pub(crate) fn pending(store: &Store) -> Result<Vec<ArmRecord>> {
    Ok(all(store)?
        .into_iter()
        .filter(|r| r.state.pending())
        .collect())
}

/// Move a record to a terminal state.
///
/// Conditional on the row still being `armed`: a cancel that lands between the
/// idle trigger and this write must WIN, and it wins by having deleted the row,
/// which makes this update affect zero rows. Returns whether it stuck, so the
/// caller can skip a send whose record was cancelled underneath it.
pub(crate) fn settle(
    store: &Store,
    agent_id: &str,
    state: ArmState,
    detail: Option<&str>,
    fired_at: DateTime<Utc>,
) -> Result<bool> {
    store.with_conn(|c| {
        let n = c.execute(
            "UPDATE compact_arms SET state=?, detail=?, fired_at=? WHERE agent_id=? AND state='armed'",
            params![state.as_str(), detail, to_ms(fired_at), agent_id],
        )?;
        Ok(n > 0)
    })
}

/// Force a state change regardless of the current one. Used only to record the
/// OUTCOME of a fire that already claimed the record via [`settle`].
pub(crate) fn force_state(
    store: &Store,
    agent_id: &str,
    state: ArmState,
    detail: Option<&str>,
) -> Result<()> {
    store.with_conn(|c| {
        c.execute(
            "UPDATE compact_arms SET state=?, detail=? WHERE agent_id=?",
            params![state.as_str(), detail, agent_id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact::model::{ArmState, DeliveryMode, IdleSource};

    fn rec(agent: &str) -> ArmRecord {
        ArmRecord {
            agent_id: agent.to_string(),
            session_id: "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            pid: Some(71015),
            harness: "claude_code".to_string(),
            mode: DeliveryMode::TerminalAppleScript,
            mode_reason: "Terminal tab on /dev/ttys001".to_string(),
            idle_source: IdleSource::RegistryTransition,
            label: "machine-89".to_string(),
            baseline_status: "busy".to_string(),
            armed_at: from_ms(1785183890237),
            state: ArmState::Armed,
            fired_at: None,
            detail: None,
        }
    }

    fn store() -> Store {
        Store::memory().expect("in-memory store")
    }

    #[test]
    fn arm_then_read_back_round_trips_every_field() {
        let s = store();
        let r = rec("agent-1");
        put(&s, &r).expect("put");
        let got = get(&s, "agent-1").expect("get").expect("row exists");
        assert_eq!(got, r);
        assert_eq!(all(&s).expect("all").len(), 1);
    }

    #[test]
    fn cancel_deletes_the_row_so_there_is_nothing_left_to_retract() {
        let s = store();
        put(&s, &rec("agent-1")).expect("put");
        assert!(delete(&s, "agent-1").expect("delete"));
        assert!(get(&s, "agent-1").expect("get").is_none());
        assert!(all(&s).expect("all").is_empty());
        // Cancelling nothing reports nothing, rather than pretending it worked.
        assert!(!delete(&s, "agent-1").expect("delete again"));
    }

    #[test]
    fn arming_the_same_agent_twice_replaces_rather_than_duplicating() {
        let s = store();
        put(&s, &rec("agent-1")).expect("put");
        let mut second = rec("agent-1");
        second.mode = DeliveryMode::NotifyOnly;
        second.mode_reason = "notify only: no control channel".to_string();
        put(&s, &second).expect("put again");
        let all = all(&s).expect("all");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].mode, DeliveryMode::NotifyOnly);
    }

    /// The cancel-versus-fire race, decided in the database rather than by luck.
    #[test]
    fn a_cancel_that_lands_first_makes_the_fire_claim_fail() {
        let s = store();
        put(&s, &rec("agent-1")).expect("put");
        assert!(delete(&s, "agent-1").expect("cancel"));
        let claimed = settle(&s, "agent-1", ArmState::Firing, None, Utc::now()).expect("settle");
        assert!(!claimed, "a cancelled record must not be claimable");
    }

    #[test]
    fn a_record_can_only_be_claimed_once() {
        let s = store();
        put(&s, &rec("agent-1")).expect("put");
        let now = Utc::now();
        assert!(settle(&s, "agent-1", ArmState::Firing, None, now).expect("first claim"));
        assert!(
            !settle(&s, "agent-1", ArmState::Firing, None, now).expect("second claim"),
            "the watcher must never fire the same record twice"
        );

        force_state(&s, "agent-1", ArmState::Delivered, Some("/compact typed")).expect("outcome");
        let got = get(&s, "agent-1").expect("get").expect("row");
        assert_eq!(got.state, ArmState::Delivered);
        assert_eq!(got.detail.as_deref(), Some("/compact typed"));
        assert!(got.fired_at.is_some());
    }

    #[test]
    fn pending_excludes_records_that_already_ran() {
        let s = store();
        put(&s, &rec("armed-one")).expect("put");
        put(&s, &rec("fired-one")).expect("put");
        settle(&s, "fired-one", ArmState::Delivered, None, Utc::now()).expect("settle");
        let pend = pending(&s).expect("pending");
        assert_eq!(pend.len(), 1);
        assert_eq!(pend[0].agent_id, "armed-one");
        assert_eq!(all(&s).expect("all").len(), 2, "history stays visible");
    }

    #[test]
    fn a_codex_thread_persists_without_a_pid() {
        let s = store();
        let mut r = rec("codex-1");
        r.pid = None;
        r.harness = "codex".to_string();
        r.mode = DeliveryMode::CodexAppServer;
        r.idle_source = IdleSource::CodexQuietWindow;
        put(&s, &r).expect("put");
        assert_eq!(get(&s, "codex-1").expect("get").expect("row"), r);
    }
}
