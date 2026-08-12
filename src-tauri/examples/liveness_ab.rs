//! Throwaway diagnostic: score the working/idle rule against real transcripts.
//!
//! Read-only. It replays local Claude transcripts through the REAL parser and the REAL
//! [`status_from_last_event`], and compares the verdict to what the transcript shows the
//! agent actually did next.
//!
//! Method: every assistant TEXT event is a decision point (that is the case the rule got
//! wrong). At each one, ask the rule "working or done?" using only the events up to that
//! point, then read the ground truth from the rest of the file:
//! another assistant/tool event follows before any operator prompt means it was WORKING,
//! while an operator prompt following (or the file ending) means it was DONE.
//!
//! Scoring both the old rule (any trailing text ends the turn) and the new one on the
//! same points gives an honest before/after.
//!
//!   cargo run --example liveness_ab

use chrono::{DateTime, Duration, Utc};
use warden_lib::ingest::claude_code::ClaudeCodeAdapter;
use warden_lib::ingest::Adapter;
use warden_lib::ir::{Event, EventRecord, Turn};
use warden_lib::radar::liveness::{status_from_last_event, AgentStatus};
use warden_lib::store::Store;
use warden_lib::util::default_claude_projects;

/// The pre-fix rule: a trailing assistant text always meant the turn was over.
fn old_rule(events: &[(Turn, EventRecord)], now: DateTime<Utc>, stale: u64) -> Option<AgentStatus> {
    let last = events
        .iter()
        .filter(|(_, e)| {
            matches!(
                e.event,
                Event::UserPrompt { .. }
                    | Event::ToolCall { .. }
                    | Event::ToolResult { .. }
                    | Event::AssistantText { .. }
            )
        })
        .max_by(|(_, a), (_, b)| a.ts.cmp(&b.ts).then(a.raw_ref.offset.cmp(&b.raw_ref.offset)))?;
    if matches!(last.1.event, Event::AssistantText { .. }) {
        return Some(AgentStatus::Idle);
    }
    let age = now.signed_duration_since(last.1.ts).num_seconds().max(0) as u64;
    Some(if age > stale {
        AgentStatus::Idle
    } else {
        AgentStatus::Working
    })
}

#[derive(Default)]
struct Score {
    right: u32,
    said_done_but_working: u32,
    said_working_but_done: u32,
}

impl Score {
    fn add(&mut self, verdict: Option<AgentStatus>, truly_working: bool) {
        let said_working = matches!(verdict, Some(AgentStatus::Working));
        match (said_working, truly_working) {
            (true, true) | (false, false) => self.right += 1,
            (false, true) => self.said_done_but_working += 1,
            (true, false) => self.said_working_but_done += 1,
        }
    }
    fn total(&self) -> u32 {
        self.right + self.said_done_but_working + self.said_working_but_done
    }
    fn report(&self, label: &str) {
        let t = self.total().max(1);
        println!(
            "  {label:<10} correct {:>5}/{:<5} ({:>5.1}%)   called a WORKING agent done: {:<5}   called a DONE agent working: {}",
            self.right,
            self.total(),
            100.0 * self.right as f64 / t as f64,
            self.said_done_but_working,
            self.said_working_but_done,
        );
    }
}

fn main() {
    let root = default_claude_projects();
    let db = std::env::temp_dir().join("warden_liveness_ab.db");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).expect("open scratch store");
    let adapter = ClaudeCodeAdapter::with_root(root.clone(), store);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect(&root, &mut files);
    // Newest first, and only a working sample: this is a diagnostic, not a full sweep.
    files.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| std::cmp::Reverse(DateTime::<Utc>::from(t)))
    });
    files.truncate(60);
    eprintln!("scoring {} transcripts under {}", files.len(), root.display());

    let stale = 180u64;
    let (mut old, mut new) = (Score::default(), Score::default());
    // Root sessions carry a PID and a live registry entry, whose `status` field is
    // authoritative and short-circuits this rule entirely. Subagents and team members
    // have neither, so the conversation-state rule is the ONLY thing judging them: score
    // them on their own, because that is the population the bug was reported against.
    let (mut old_sub, mut new_sub) = (Score::default(), Score::default());
    let mut worst = Duration::zero();

    for p in &files {
        let is_subagent = p.components().any(|c| c.as_os_str() == "subagents");
        let Ok(bytes) = std::fs::read(p) else { continue };
        let Ok(batches) = adapter.parse_range(p, &bytes, 0, 0) else {
            continue;
        };
        // The rule reads only the EventRecord half of each pair, so any turn serves as
        // the filler. Flatten every batch the file produced, in file order.
        let mut all: Vec<(Turn, EventRecord)> = Vec::new();
        for batch in &batches {
            let Some(t0) = batch.turns.first().cloned() else {
                continue;
            };
            all.extend(batch.events.iter().cloned().map(|e| (t0.clone(), e)));
        }
        all.sort_by(|(_, a), (_, b)| a.ts.cmp(&b.ts).then(a.raw_ref.offset.cmp(&b.raw_ref.offset)));

        for i in 0..all.len() {
            let Event::AssistantText { .. } = all[i].1.event else {
                continue;
            };
            let at = all[i].1.ts;
            // Judge one second after the text landed: the moment the radar would refresh.
            let now = at + Duration::seconds(1);
            let prefix = &all[..=i];

            // Ground truth from the rest of the file.
            let mut truly_working = false;
            for (_, e) in all.iter().skip(i + 1) {
                match &e.event {
                    Event::UserPrompt { .. } => break,
                    Event::AssistantText { .. }
                    | Event::ToolCall { .. }
                    | Event::ToolResult { .. }
                    | Event::Thinking { .. } => {
                        truly_working = true;
                        if e.ts - at > worst {
                            worst = e.ts - at;
                        }
                        break;
                    }
                    _ => {}
                }
            }

            let old_v = old_rule(prefix, now, stale);
            let new_v = status_from_last_event(prefix, now, stale);
            old.add(old_v, truly_working);
            new.add(new_v, truly_working);
            if is_subagent {
                old_sub.add(old_v, truly_working);
                new_sub.add(new_v, truly_working);
            }
        }
    }

    println!("\n=== working/idle accuracy at every assistant-text decision point ===");
    println!("\nALL sessions:");
    old.report("OLD rule");
    new.report("NEW rule");
    println!("\nSUBAGENTS and TEAM MEMBERS only (no PID, no registry entry, so this rule is the only judge):");
    old_sub.report("OLD rule");
    new_sub.report("NEW rule");
    println!(
        "\n  longest stretch a mid-turn preamble sat as the newest action: {}s",
        worst.num_seconds()
    );
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}
