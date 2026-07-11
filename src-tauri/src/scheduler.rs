//! Task schedulers: the WHEN of WARDEN's background work, split by concern.
//!
//! Two independent drivers decide *when* background work runs; the *what* lives
//! in the `radar` / `ingest` DOMAIN modules they call into:
//!
//! * `watch` — the live-ingest file watcher: byte-offset watermark resume +
//!   FSEvents tailing ([`ingest_file_once`], [`spawn_watchers`]).
//! * `radar` — the single coalesced RADAR recompute task plus its liveness
//!   heartbeat and dirty signal ([`spawn_radar_watcher`], [`RadarDirtySignal`]).
//!
//! This file is a thin façade: it declares the submodules and re-exports the
//! curated public surface (`crate::scheduler::*`) the rest of the crate depends
//! on. Cross-submodule internals stay `pub(crate)`; nothing else is forwarded.

mod watch;
mod radar;

pub use watch::{ingest_file_once, spawn_watchers, WatcherGuard};
pub use radar::{
    cache_radar_state, latest_cached_radar_state, new_radar_state_cache, spawn_radar_watcher,
    RadarDirtySignal, RadarStateCache,
};
