pub mod claude_code;

use crate::ir::*;
use anyhow::Result;
use std::path::PathBuf;

pub struct SessionBatch {
    pub session: Session,
    pub turns: Vec<Turn>,
    pub events: Vec<EventRecord>,
    pub offset: u64,
}

pub trait Adapter {
    fn harness(&self) -> Harness;
    fn detect(&self) -> Result<Vec<PathBuf>>;
    fn backfill(&self) -> Result<Vec<SessionBatch>>;
}
