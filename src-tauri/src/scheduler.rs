//! Live-ingest scheduler: byte-offset watermark resume + FSEvents tailing.
//!
//! Two layers, deliberately separated so the logic is unit-testable without a
//! running event loop:
//!
//! * [`ingest_file_once`] — the **testable core**. Given a single `*.jsonl`
//!   path it figures out which adapter owns it, consults the saved byte
//!   watermark, reads only the new tail (or the whole file on first sight / after
//!   a rewrite), maps it to IR via [`Adapter::parse_range`], and persists the
//!   batch advancing the watermark to the new EOF. No watcher, no `AppHandle`.
//! * [`spawn_watchers`] — the **notify glue**. One `RecommendedWatcher`
//!   (FSEvents on macOS) per adapter root; on a debounced `*.jsonl`
//!   create/modify it calls [`ingest_file_once`] and emits `ingest_progress`.
//!
//! Watermarks are byte offsets, not record counts: FSEvents coalesces rapid
//! writes, so on every event we seek to the saved offset and read to EOF rather
//! than trusting how many events the OS reported (see CLAUDE.md “Watermarks are
//! byte-offset”).

use crate::ingest::AdapterRegistry;
use crate::store::Store;
use crate::util::hash64;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Debounce window for coalescing FSEvents bursts on a single file.
/// Override with `WARDEN_WATCH_DEBOUNCE_MS` (mirrors the `util.rs` env pattern).
fn debounce() -> Duration {
    let ms = std::env::var("WARDEN_WATCH_DEBOUNCE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(250);
    Duration::from_millis(ms)
}

/// True when `path` lives under (or is) one of `root`'s subtrees.
fn path_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Ingest the new bytes of a single transcript file, advancing its watermark.
///
/// Returns the number of events ingested (0 when nothing new). This is the
/// testable core invoked by the watcher and reusable for an on-ask trigger.
///
/// Algorithm (brief §Task 4):
/// * `len` = current file length; `off` = saved byte watermark (0 if unseen).
/// * `len == off` AND the full-file hash is unchanged → nothing new, skip.
/// * `len  < off` (file rewritten/truncated) → reset `off = 0`, full reparse.
/// * otherwise read `bytes[off..]` and `parse_range(path, slice, off, hash)`.
/// * persist each batch with `watermark_offset = len` (the new EOF).
pub fn ingest_file_once(
    registry: &AdapterRegistry,
    store: &Store,
    path: &Path,
) -> Result<usize> {
    // Only transcript files are ingestible.
    if path.extension().map(|x| x != "jsonl").unwrap_or(true) {
        return Ok(0);
    }

    // Find the adapter that owns this path (its root is an ancestor).
    let adapter = registry
        .adapters()
        .iter()
        .find(|a| a.roots().iter().any(|r| path_under(path, r)));
    let adapter = match adapter {
        Some(a) => a,
        None => return Ok(0), // not under any watched root — ignore
    };

    // A file may vanish between the FSEvent and our read; treat as nothing to do.
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Ok(0),
    };
    let len = bytes.len() as u64;
    let hash = hash64(&bytes);
    let mut off = store.watermark_offset(path)?;

    if len == off {
        // No growth. If the content hash also matches what we stored, there is
        // genuinely nothing new (a touch / metadata-only event). Otherwise the
        // file was rewritten in place at the same length → full reparse.
        let unchanged = store
            .source_raw_hash(path)?
            .map(|h| h == hash)
            .unwrap_or(false);
        if unchanged {
            return Ok(0);
        }
        off = 0;
    } else if len < off {
        // Truncated or rewritten shorter than the watermark → start over.
        off = 0;
    }

    let slice = &bytes[off as usize..];
    let batches = adapter
        .parse_range(path, slice, off, hash)
        .with_context(|| format!("parse_range {} @ {off}", path.display()))?;

    let mut events = 0usize;
    for b in &batches {
        events += b.events.len();
        // Persist with the absolute EOF as the new watermark.
        store.upsert_session_batch(&b.session, &b.turns, &b.events, len)?;
    }
    Ok(events)
}

/// Owns the live `RecommendedWatcher`s so they outlive `setup()`. A bare
/// `Vec<RecommendedWatcher>` is `Send` but not `Sync`, so it can't go into
/// Tauri-managed state directly; the `Mutex` makes the holder `Sync`. Dropping
/// this stops all watchers.
pub struct WatcherGuard {
    _watchers: std::sync::Mutex<Vec<notify::RecommendedWatcher>>,
}

impl WatcherGuard {
    pub fn new(watchers: Vec<notify::RecommendedWatcher>) -> Self {
        Self {
            _watchers: std::sync::Mutex::new(watchers),
        }
    }
}

/// Spawn one filesystem watcher per adapter root. On a debounced `*.jsonl`
/// create/modify, [`ingest_file_once`] runs and an `ingest_progress` event is
/// emitted (`{harness, path, events, phase:"live"}`).
///
/// The returned watchers must be kept alive for the lifetime of the app (drop =
/// stop watching), so callers store them in long-lived state (see
/// [`WatcherGuard`]).
pub fn spawn_watchers(
    registry: Arc<AdapterRegistry>,
    store: Store,
    app: tauri::AppHandle,
) -> Result<Vec<notify::RecommendedWatcher>> {
    use notify::{EventKind, RecursiveMode, Watcher};
    use tauri::Emitter;

    // Unique roots across all adapters (an adapter may expose several).
    let mut roots: Vec<PathBuf> = Vec::new();
    for a in registry.adapters() {
        for r in a.roots() {
            if !roots.contains(&r) {
                roots.push(r);
            }
        }
    }

    let mut watchers = Vec::new();
    for root in roots {
        // FSEvents fires on a parent even if `root` does not yet exist; only watch
        // roots that are present to avoid a watcher error on a fresh machine.
        if !root.exists() {
            tracing::info!(root=%root.display(), "watch root absent; skipping (backfill will create on first session)");
            continue;
        }
        let registry = registry.clone();
        let store = store.clone();
        let app = app.clone();
        let debounce = debounce();
        // Per-file last-handled timestamp for debouncing coalesced bursts.
        let mut last: HashMap<PathBuf, Instant> = HashMap::new();

        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error=?e, "fs watch error");
                        return;
                    }
                };
                // Only creations and content modifications matter.
                if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    return;
                }
                for path in event.paths {
                    if path.extension().map(|x| x != "jsonl").unwrap_or(true) {
                        continue;
                    }
                    // Debounce: skip if we handled this exact path within the window.
                    let now = Instant::now();
                    if let Some(prev) = last.get(&path) {
                        if now.duration_since(*prev) < debounce {
                            continue;
                        }
                    }
                    last.insert(path.clone(), now);

                    match ingest_file_once(&registry, &store, &path) {
                        Ok(events) if events > 0 => {
                            let harness = registry
                                .adapters()
                                .iter()
                                .find(|a| a.roots().iter().any(|r| path.starts_with(r)))
                                .map(|a| a.harness().as_str().to_string())
                                .unwrap_or_default();
                            let _ = app.emit(
                                "ingest_progress",
                                serde_json::json!({
                                    "harness": harness,
                                    "path": path.to_string_lossy(),
                                    "events": events,
                                    "phase": "live",
                                }),
                            );
                        }
                        Ok(_) => {} // nothing new
                        Err(e) => {
                            tracing::warn!(path=%path.display(), error=?e, "live ingest failed")
                        }
                    }
                }
            },
        )
        .context("create fs watcher")?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", root.display()))?;
        tracing::info!(root=%root.display(), "watching for live transcripts");
        watchers.push(watcher);
    }
    Ok(watchers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::claude_code::ClaudeCodeAdapter;
    use crate::ingest::codex::CodexAdapter;
    use crate::ingest::{Adapter, AdapterRegistry};
    use crate::ir::*;
    use tempfile::tempdir;

    /// Build a registry over a single Codex adapter rooted at `root` (its archived
    /// root points at the same dir, harmless for these tests).
    fn codex_registry(root: &Path, store: Store) -> AdapterRegistry {
        AdapterRegistry::for_test(vec![Box::new(CodexAdapter::with_root(
            root.to_path_buf(),
            root.to_path_buf(),
            store,
        ))])
    }

    /// A realistically-named rollout file whose filename encodes the session uuid
    /// that also appears in `session_meta.payload.id` — required so a tail parse
    /// (deriving the id from the filename) lands on the same session as backfill.
    fn write_rollout(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join(
            "rollout-2026-06-19T16-33-00-019ee0ba-8295-7ba0-9971-c5af95e77191.jsonl",
        );
        std::fs::write(&p, body).unwrap();
        p
    }

    const META: &str = "{\"timestamp\":\"2026-06-19T16:33:45.869Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019ee0ba-8295-7ba0-9971-c5af95e77191\",\"cwd\":\"/work\",\"model_provider\":\"openai\"}}\n";
    const MSG1: &str = "{\"timestamp\":\"2026-06-19T16:34:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"first\"}}\n";
    const MSG2: &str = "{\"timestamp\":\"2026-06-19T16:35:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"second\"}}\n";

    /// `ingest_file_once` on a fresh file ingests everything and sets the watermark
    /// to the file length; a re-run on the unchanged file adds nothing (resume).
    #[test]
    fn ingest_file_once_seeds_then_resumes() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);
        let len = std::fs::metadata(&p).unwrap().len();

        let first = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(first >= 1, "first ingest must add events, got {first}");
        let (sessions, events_after_first, _) = store.counts().unwrap();
        assert_eq!(sessions, 1, "one session ingested");
        assert!(events_after_first >= 1);
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            len,
            "watermark must equal the file length after the first ingest"
        );

        // Re-run on the byte-identical file → resume, nothing new.
        let again = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(again, 0, "unchanged file must add 0 events (watermark resume)");
        let (_, events_after_resume, _) = store.counts().unwrap();
        assert_eq!(
            events_after_resume, events_after_first,
            "event count unchanged after a resume run"
        );
    }

    /// Appending one line yields exactly one new event and advances the watermark;
    /// the appended event's RawRef.offset is the ABSOLUTE position of its line.
    #[test]
    fn ingest_file_once_tails_appended_line() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);
        let _ = ingest_file_once(&registry, &store, &p).unwrap();
        let (_, base_events, _) = store.counts().unwrap();
        let off_before = std::fs::metadata(&p).unwrap().len();

        // Append a second agent_message line.
        let full = format!("{body}{MSG2}");
        std::fs::write(&p, &full).unwrap();
        let new_len = std::fs::metadata(&p).unwrap().len();

        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "exactly one appended event");
        let (_, events_now, _) = store.counts().unwrap();
        assert_eq!(
            events_now,
            base_events + 1,
            "store gained exactly one event"
        );
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            new_len,
            "watermark advanced to the new EOF"
        );

        // The newest AssistantText event must point at the appended line's start.
        let sid = store.sessions().unwrap()[0].id.clone();
        let appended = store
            .session_events(&sid)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match &e.event {
                Event::AssistantText { text } if text == "second" => Some(e.raw_ref.offset),
                _ => None,
            })
            .expect("appended 'second' event present");
        assert_eq!(
            appended, off_before,
            "appended event offset must be the absolute start of its line"
        );
    }

    /// A shrink/rewrite below the watermark forces a full reparse from offset 0.
    #[test]
    fn ingest_file_once_reparses_on_shrink() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let long = format!("{META}{MSG1}{MSG2}");
        let p = write_rollout(dir.path(), &long);
        let _ = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(store.watermark_offset(&p).unwrap(), long.len() as u64);

        // Rewrite the file shorter (e.g. log rotation) — watermark now exceeds len.
        let short = format!("{META}{MSG1}");
        std::fs::write(&p, &short).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(
            added >= 1,
            "a shrink must trigger a full reparse (offset reset to 0)"
        );
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            short.len() as u64,
            "watermark reset to the new (shorter) EOF"
        );
    }

    /// CRITICAL no-clobber: a live tail parse (which lacks session_meta) must not
    /// overwrite the good `model_ids` / `started_at` written by the full backfill.
    ///
    /// Simulated end-to-end: full-backfill the file via the adapter+store, then
    /// append a line and run `ingest_file_once` (the live path). Re-read the
    /// session row and assert the original model_ids and started_at survived.
    #[test]
    fn live_tail_does_not_clobber_backfilled_session() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        let body = format!("{META}{MSG1}");
        let p = write_rollout(dir.path(), &body);

        // Full backfill (offset 0) — resolves model_ids=["openai"] and the true
        // started_at from session_meta. Use the adapter's parse_range directly so
        // the session row mirrors a real backfill.
        let full_hash = hash64(body.as_bytes());
        let backfilled = registry.adapters()[0]
            .parse_range(&p, body.as_bytes(), 0, full_hash)
            .unwrap();
        let original = &backfilled[0].session;
        let original_model_ids = original.model_ids.clone();
        let original_started_at = original.started_at;
        assert_eq!(
            original_model_ids,
            vec!["openai".to_string()],
            "precondition: backfill resolved model_ids from session_meta"
        );
        store
            .upsert_session_batch(
                &backfilled[0].session,
                &backfilled[0].turns,
                &backfilled[0].events,
                body.len() as u64,
            )
            .unwrap();

        // Now a LIVE append + tail ingest (offset>0, no session_meta in slice).
        let full = format!("{body}{MSG2}");
        std::fs::write(&p, &full).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert_eq!(added, 1, "tail added the appended event");

        // Re-read the persisted session and assert nothing good was clobbered.
        let row = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == original.id)
            .expect("session row present");
        assert_eq!(
            row.model_ids, original_model_ids,
            "tail parse must NOT erase the backfilled model_ids"
        );
        assert_eq!(
            row.started_at, original_started_at,
            "tail parse must NOT push started_at later than the true session start"
        );
        // And the project survived too (tail has no cwd → null project).
        assert!(
            row.project.is_some(),
            "tail parse must NOT null out the backfilled project"
        );
    }

    /// Paths outside every adapter root, and non-jsonl files, are ignored (0).
    #[test]
    fn ingest_file_once_ignores_foreign_and_nonjsonl() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = codex_registry(dir.path(), store.clone());

        // Non-jsonl inside the root.
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "hello").unwrap();
        assert_eq!(ingest_file_once(&registry, &store, &txt).unwrap(), 0);

        // jsonl OUTSIDE the root.
        let other = tempdir().unwrap();
        let foreign = write_rollout(other.path(), &format!("{META}{MSG1}"));
        assert_eq!(
            ingest_file_once(&registry, &store, &foreign).unwrap(),
            0,
            "a path under no watched root must be ignored"
        );
    }

    /// Claude path smoke-test: the Claude adapter participates in `ingest_file_once`
    /// (filename == sessionId so backfill and tail share a session id).
    #[test]
    fn ingest_file_once_handles_claude() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let registry = AdapterRegistry::for_test(vec![Box::new(
            ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store.clone()),
        )]);
        // Filename stem IS the sessionId, matching the real Claude layout.
        let p = dir.path().join("019ee0ba.jsonl");
        let l1 = "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"019ee0ba\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hi\"}}\n";
        std::fs::write(&p, l1).unwrap();
        let first = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(first >= 1);
        let len1 = std::fs::metadata(&p).unwrap().len();
        assert_eq!(store.watermark_offset(&p).unwrap(), len1);

        let l2 = "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"019ee0ba\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":\"tail\"}}\n";
        std::fs::write(&p, format!("{l1}{l2}")).unwrap();
        let added = ingest_file_once(&registry, &store, &p).unwrap();
        assert!(added >= 1, "claude tail adds the appended assistant event");
        assert_eq!(
            store.watermark_offset(&p).unwrap(),
            std::fs::metadata(&p).unwrap().len()
        );
    }
}
