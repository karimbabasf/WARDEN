//! DOSSIER — longitudinal, evidence-cited operator profile.
//!
//! Self-contained module that reads existing WARDEN data (sessions / turns /
//! events / features / findings) and writes only new `dossier_*` tables. The
//! first submodule is [`scope`], which defines the time-[`scope::Window`] toggle
//! and the session-scoping helper every aggregator builds on.

pub mod scope;
