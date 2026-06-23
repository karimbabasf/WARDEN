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
        let adapter =
            crate::ingest::claude_code::ClaudeCodeAdapter::with_root(root, store.clone());
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
        let adapter =
            crate::ingest::claude_code::ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store);
        let bytes = std::fs::read(&p).unwrap();
        let batches = adapter.parse_range(&p, &bytes, 0, 0).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].session.external_id, "s");
    }

    #[test]
    fn parse_range_nonzero_offset_errors() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, b"dummy" as &[u8]).unwrap();
        let store = Store::memory().unwrap();
        let adapter =
            crate::ingest::claude_code::ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store);
        let result = adapter.parse_range(&p, b"dummy", 100, 0);
        assert!(result.is_err(), "non-zero offset must error until Task 4");
    }

    #[test]
    fn roots_returns_adapter_root() {
        let dir = tempdir().unwrap();
        let store = Store::memory().unwrap();
        let adapter =
            crate::ingest::claude_code::ClaudeCodeAdapter::with_root(dir.path().to_path_buf(), store);
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
