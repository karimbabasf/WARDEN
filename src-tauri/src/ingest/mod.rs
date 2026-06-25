pub mod claude_code;
pub mod codex;

use crate::ir::*;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct SessionBatch {
    pub session: Session,
    pub turns: Vec<Turn>,
    pub events: Vec<EventRecord>,
    pub offset: u64,
}

pub trait Adapter: Send + Sync {
    fn harness(&self) -> Harness;
    fn detect(&self) -> Result<Vec<PathBuf>>;
    fn backfill(&self) -> Result<Vec<SessionBatch>>;
    fn parse_range(
        &self,
        path: &Path,
        bytes: &[u8],
        start_offset: u64,
        raw_hash: u64,
    ) -> Result<Vec<SessionBatch>>;
    fn roots(&self) -> Vec<PathBuf>;
}

pub struct IngestSummary {
    pub by_harness: Vec<(Harness, usize, usize)>,
    pub errors: Vec<String>,
}

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn Adapter>>,
}

impl AdapterRegistry {
    pub fn new(store: crate::store::Store) -> Self {
        let adapters: Vec<Box<dyn Adapter>> = vec![
            Box::new(claude_code::ClaudeCodeAdapter::new(store.clone())),
            Box::new(codex::CodexAdapter::new(store)),
        ];
        Self { adapters }
    }

    /// Build a registry from an explicit adapter list. Used by the scheduler's
    /// cross-module tests (and any caller that needs a bespoke root set) since the
    /// `adapters` field is private.
    pub fn from_adapters(adapters: Vec<Box<dyn Adapter>>) -> Self {
        Self { adapters }
    }

    /// Test-only alias for [`Self::from_adapters`].
    #[doc(hidden)]
    pub fn for_test(adapters: Vec<Box<dyn Adapter>>) -> Self {
        Self::from_adapters(adapters)
    }

    pub fn adapters(&self) -> &[Box<dyn Adapter>] {
        &self.adapters
    }

    pub fn backfill_all(&self, store: &crate::store::Store) -> IngestSummary {
        let mut by_harness: Vec<(Harness, usize, usize)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for adapter in &self.adapters {
            let harness = adapter.harness();
            let result = (|| -> anyhow::Result<(usize, usize)> {
                let batches = adapter.backfill()?;
                let mut sessions = 0usize;
                let mut events = 0usize;
                for b in batches {
                    events += b.events.len();
                    store.upsert_session_batch(&b.session, &b.turns, &b.events, b.offset)?;
                    sessions += 1;
                }
                Ok((sessions, events))
            })();
            match result {
                Ok((sessions, events)) => {
                    by_harness.push((harness, sessions, events));
                }
                Err(e) => {
                    errors.push(format!("{}: {e:#}", harness.as_str()));
                    by_harness.push((harness, 0, 0));
                }
            }
        }

        // RADAR: once every adapter's batches are persisted, resolve parent→child
        // links over the store (both parent and child rows now exist). Best-effort:
        // a linkage failure is surfaced as an error but never discards the ingested
        // data. This is the expensive whole-store point; steady RADAR reads stay
        // read-only and cached.
        if let Err(e) = claude_code::link_claude_subagents_in_store(store) {
            errors.push(format!("claude linkage: {e:#}"));
        }
        if let Err(e) = codex::link_codex_subagents_in_store(store) {
            errors.push(format!("codex linkage: {e:#}"));
        }

        IngestSummary { by_harness, errors }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::tempdir;

    #[test]
    fn registry_has_at_least_one_adapter() {
        let store = Store::memory().unwrap();
        let registry = AdapterRegistry::new(store);
        assert!(
            !registry.adapters().is_empty(),
            "AdapterRegistry must register at least one adapter"
        );
    }

    #[test]
    fn backfill_all_empty_root_no_errors() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        // Build registry with an empty temp root so no files are found
        let mut adapter = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store.clone(),
        );
        adapter.max_files = None;
        // Create registry manually with the temp-root adapter
        let registry = AdapterRegistry {
            adapters: vec![Box::new(adapter)],
        };
        let summary = registry.backfill_all(&store);
        assert!(
            summary.errors.is_empty(),
            "no errors expected on empty root; got: {:?}",
            summary.errors
        );
        assert_eq!(
            summary.by_harness.len(),
            1,
            "one entry per registered adapter"
        );
    }

    #[test]
    fn backfill_all_returns_sessions_and_events() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Write a minimal JSONL session file
        let p = root.join("test.jsonl");
        std::fs::write(
            &p,
            "{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hello\"}}\n",
        )
        .unwrap();
        let store = Store::memory().unwrap();
        let adapter = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(root, store.clone());
        let registry = AdapterRegistry {
            adapters: vec![Box::new(adapter)],
        };
        let summary = registry.backfill_all(&store);
        assert!(summary.errors.is_empty(), "no errors: {:?}", summary.errors);
        let (harness, sessions, _events) = &summary.by_harness[0];
        assert!(matches!(harness, Harness::ClaudeCode));
        assert_eq!(*sessions, 1, "expected 1 session ingested");
    }

    #[test]
    fn backfill_all_links_claude_subagents_after_persisting_batches() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("proj");
        let session = root.join("019sess");
        let subs = session.join("subagents");
        std::fs::create_dir_all(&subs).unwrap();

        let parent_jsonl = session.join("019sess.jsonl");
        std::fs::write(&parent_jsonl, "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"019sess\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"sourceToolAssistantUuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_registry\",\"name\":\"Task\",\"input\":{}}]}}\n").unwrap();

        let child_jsonl = subs.join("agent-feedface.jsonl");
        std::fs::write(&child_jsonl, "{\"type\":\"user\",\"uuid\":\"cu\",\"sessionId\":\"019sess-sub\",\"isSidechain\":true,\"agentId\":\"feedface\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"content\":\"work\"}}\n").unwrap();
        std::fs::write(
            subs.join("agent-feedface.meta.json"),
            r#"{"agentType":"Explore","description":"trace the live radar path","toolUseId":"toolu_registry"}"#,
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let adapter =
            crate::ingest::claude_code::ClaudeCodeAdapter::with_root(root.clone(), store.clone());
        let registry = AdapterRegistry {
            adapters: vec![Box::new(adapter)],
        };
        let summary = registry.backfill_all(&store);
        assert!(summary.errors.is_empty(), "no errors: {:?}", summary.errors);

        let parent_sid =
            crate::util::stable_id(&["claude_code", "019sess", &parent_jsonl.to_string_lossy()]);
        let child_sid = crate::util::stable_id(&[
            "claude_code",
            "agent-feedface",
            &child_jsonl.to_string_lossy(),
        ]);
        assert_eq!(
            store.parent_of(&child_sid).unwrap(),
            Some(parent_sid),
            "registry backfill must preserve Claude subagent nesting without relying on RADAR recompute"
        );
    }

    #[test]
    fn registry_new_default_has_claude_code() {
        let store = Store::memory().unwrap();
        let registry = AdapterRegistry::new(store);
        let has_claude = registry
            .adapters()
            .iter()
            .any(|a| matches!(a.harness(), Harness::ClaudeCode));
        assert!(has_claude, "registry must include ClaudeCodeAdapter");
    }

    #[test]
    fn parse_range_offset_zero_delegates_to_parse_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(
            &p,
            "{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hello\"}}\n",
        )
        .unwrap();
        let store = Store::memory().unwrap();
        let adapter = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store,
        );
        let bytes = std::fs::read(&p).unwrap();
        let batches = adapter.parse_range(&p, &bytes, 0, 0).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].session.external_id, "s");
    }

    #[test]
    fn parse_range_nonzero_offset_parses_tail_slice() {
        // Task 4 lit up the tail path: a non-zero start offset now parses the
        // appended slice (no longer an error). Feed one appended assistant line
        // with start_offset = original EOF; expect exactly that event back, with
        // an absolute RawRef.offset equal to the start offset.
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let original = b"{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hi\"}}\n";
        std::fs::write(&p, original as &[u8]).unwrap();
        let eof = original.len() as u64;
        let appended = b"{\"type\":\"assistant\",\"uuid\":\"a\",\"parentUuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"claude\",\"content\":\"tail\"}}\n";
        let store = Store::memory().unwrap();
        let adapter = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store,
        );
        let batches = adapter
            .parse_range(&p, appended, eof, 0)
            .expect("tail slice parses");
        assert_eq!(batches.len(), 1);
        let ev = batches[0]
            .events
            .iter()
            .find(|e| matches!(&e.event, Event::AssistantText { text } if text == "tail"))
            .expect("appended AssistantText present");
        assert_eq!(ev.raw_ref.offset, eof, "tail offset must be absolute");
    }

    #[test]
    fn roots_returns_adapter_root() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let adapter = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store,
        );
        let roots = adapter.roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0], dir.path());
    }

    /// A test-only adapter that always fails on backfill.
    struct FailingAdapter;

    impl Adapter for FailingAdapter {
        fn harness(&self) -> Harness {
            Harness::Generic("fail".into())
        }
        fn detect(&self) -> anyhow::Result<Vec<PathBuf>> {
            Ok(vec![])
        }
        fn backfill(&self) -> anyhow::Result<Vec<SessionBatch>> {
            anyhow::bail!("boom")
        }
        fn parse_range(
            &self,
            _path: &Path,
            _bytes: &[u8],
            _start_offset: u64,
            _raw_hash: u64,
        ) -> anyhow::Result<Vec<SessionBatch>> {
            anyhow::bail!("boom")
        }
        fn roots(&self) -> Vec<PathBuf> {
            vec![]
        }
    }

    #[test]
    fn backfill_all_continues_past_failing_adapter() {
        // Registry: [FailingAdapter, ClaudeCodeAdapter(empty root)]
        // After backfill_all: exactly 1 error AND the succeeding adapter's entry is present,
        // proving the loop continued past the failure.
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let succeeding = crate::ingest::claude_code::ClaudeCodeAdapter::with_root(
            dir.path().to_path_buf(),
            store.clone(),
        );
        let registry = AdapterRegistry {
            adapters: vec![Box::new(FailingAdapter), Box::new(succeeding)],
        };
        let summary = registry.backfill_all(&store);

        // The failing adapter must have produced exactly one error.
        assert_eq!(
            summary.errors.len(),
            1,
            "expected exactly 1 error from FailingAdapter; got: {:?}",
            summary.errors
        );

        // The succeeding adapter must still have an entry in by_harness,
        // proving the loop continued past the failure.
        let has_claude_code_entry = summary
            .by_harness
            .iter()
            .any(|(h, _, _)| matches!(h, Harness::ClaudeCode));
        assert!(
            has_claude_code_entry,
            "ClaudeCodeAdapter entry missing — loop did not continue past FailingAdapter"
        );
    }
}
