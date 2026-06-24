//! Read-only fix preview (M2). Renders a unified diff of the guidance change a
//! given finding implies — against the *real* target file (e.g. the durable
//! `~/.claude/CLAUDE.md`) — but NEVER writes anything. Applying the diff is M4
//! (`apply_artifact`), so `FixPreview::applied` is always `false`.
//!
//! Strategy: each `pattern_id` maps to a short, durable guidance block plus the
//! file it belongs in. We read the current target (missing file → empty), append
//! the block if it is not already present, and diff old→new with `similar` so the
//! war-room renders an honest "here is the edit WARDEN would propose".
use crate::ir::Finding;
use crate::store::Store;
use crate::util::claude_md_path;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixPreview {
    pub finding_id: String,
    pub pattern_id: String,
    pub target_path: String,
    pub diff: String,
    /// Always false in M2 — preview only. Apply is the M4 Forge slice.
    pub applied: bool,
}

/// Build a read-only unified-diff preview for `finding_id`. Resolves the finding
/// from the store, picks the per-pattern guidance fix, reads the real target file
/// (never writing), and returns the diff that *would* be applied.
pub fn fix_preview(store: &Store, finding_id: &str) -> Result<FixPreview> {
    let finding = store
        .finding_by_id(finding_id)?
        .ok_or_else(|| anyhow!("finding {finding_id} not found"))?;
    Ok(preview_for_finding(&finding))
}

/// Resolve the real target (env-overridable `~/.claude/CLAUDE.md`) and render the
/// preview against it. Thin wrapper over the pure, target-injected core below.
pub fn preview_for_finding(finding: &Finding) -> FixPreview {
    let fix = fix_for_pattern(&finding.pattern_id);
    let target = fix.target_path();
    preview_against(finding, &fix, &target)
}

/// Pure core, isolated from the store AND from global path resolution: read the
/// injected `target` file (missing → empty), append the per-pattern guidance
/// block if absent, and render the old→new unified diff. Taking `target` as a
/// parameter — instead of reading the process-wide `WARDEN_CLAUDE_MD` here — is
/// what lets the unit tests run in parallel without racing on that shared env var.
fn preview_against(finding: &Finding, fix: &PatternFix, target: &std::path::Path) -> FixPreview {
    let current = std::fs::read_to_string(target).unwrap_or_default();
    let proposed = ensure_block(&current, &fix.block);
    let diff = unified_diff(target, &current, &proposed);
    FixPreview {
        finding_id: finding.id.clone(),
        pattern_id: finding.pattern_id.clone(),
        target_path: target.to_string_lossy().into_owned(),
        diff,
        applied: false,
    }
}

/// Where a fix lands + the guidance block it inserts. Every WARDEN-known pattern
/// resolves to a durable rule in `~/.claude/CLAUDE.md`; unknown patterns get a
/// generic CLAUDE.md note so the preview is never empty.
struct PatternFix {
    block: String,
}

impl PatternFix {
    fn target_path(&self) -> PathBuf {
        // All current fixes are CLAUDE.md guidance edits. Kept as a method so a
        // future pattern can target a hook/skill path without touching callers.
        claude_md_path()
    }
}

fn fix_for_pattern(pattern_id: &str) -> PatternFix {
    let rule = match pattern_id {
        "CONTEXT_BLOAT" => {
            "- Delegate broad search/file-reading to subagents; keep the main context for decisions and edits, not raw file dumps."
        }
        "NO_DELEGATION" => {
            "- For multi-file discovery, dispatch an Explore/general-purpose subagent and keep only its conclusion — never inventory files in the main context."
        }
        "UNVERIFIED_COMPLETION" => {
            "- Never claim done without running the build/tests and reading real output. Evidence before assertions."
        }
        "IGNORED_TOOL_ERROR" => {
            "- Treat every tool error as a stop signal: read it, fix the root cause, and re-verify before continuing."
        }
        "VAGUE_PROMPT" => {
            "- State the goal, constraints, and the acceptance check up front so the first attempt can be verified instead of reprompted."
        }
        "WHACK_A_MOLE" => {
            "- On a second failing attempt, stop patching symptoms: reset to the root cause before editing again."
        }
        "CACHE_COLD_RESTARTS" => {
            "- Reuse a warm session for related work instead of cold restarts; cold context re-reads burn tokens with low cache hits."
        }
        "REPEATED_EXPLANATION" => {
            "- Move recurring project context into this CLAUDE.md so it is not re-explained every session."
        }
        _ => "- Review this recurring workflow hole flagged by WARDEN and add a durable guardrail here.",
    };
    PatternFix {
        block: format!("\n## WARDEN guardrail — {pattern_id}\n{rule}\n"),
    }
}

/// Append `block` to `current` unless an identical block is already present.
/// Idempotent: previewing an already-applied fix yields no change (empty diff).
fn ensure_block(current: &str, block: &str) -> String {
    if current.contains(block.trim()) {
        return current.to_string();
    }
    let mut out = current.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out
}

/// Render a standard unified diff (`---`/`+++` headers + `@@` hunks) for the file
/// at `target`, old→new. Empty when there is no change.
fn unified_diff(target: &std::path::Path, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let label = target.to_string_lossy();
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("a/{label}"), &format!("b/{label}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(pattern: &str) -> Finding {
        Finding {
            id: format!("id-{pattern}"),
            pattern_id: pattern.into(),
            title: pattern.into(),
            severity: 4,
            frequency: 0.5,
            est_cost_tokens: 1,
            est_cost_minutes: 1,
            confidence: 0.7,
            rationale: "r".into(),
            evidence: vec![],
            status: "candidate".into(),
            verifier_verdict: None,
        }
    }

    // Render directly against an explicit target path. No `WARDEN_CLAUDE_MD`
    // mutation, so these tests are deterministic and parallel-safe — the shared
    // process-global env var previously raced across the parallel test threads
    // (the missing-target case could read a sibling test's already-blocked file
    // and see an empty diff).
    fn preview_at(pattern: &str, target: &std::path::Path) -> FixPreview {
        preview_against(&finding(pattern), &fix_for_pattern(pattern), target)
    }

    #[test]
    fn preview_is_nonempty_diff_and_never_applied() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "# Project\n\nexisting line\n").unwrap();

        let preview = preview_at("UNVERIFIED_COMPLETION", &target);

        assert!(!preview.applied, "preview must never be applied (apply = M4)");
        assert!(!preview.diff.is_empty(), "expected a non-empty unified diff");
        assert!(
            preview.diff.contains("+## WARDEN guardrail — UNVERIFIED_COMPLETION"),
            "diff should add the per-pattern guardrail block; got:\n{}",
            preview.diff
        );
        assert!(
            preview.diff.contains("@@"),
            "expected unified-diff hunk header"
        );
        assert_eq!(preview.target_path, target.to_string_lossy());
    }

    #[test]
    fn preview_against_missing_target_still_produces_diff() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nonexistent-CLAUDE.md");

        let preview = preview_at("CONTEXT_BLOAT", &target);

        assert!(!preview.diff.is_empty());
        assert!(preview.diff.contains("WARDEN guardrail — CONTEXT_BLOAT"));
    }

    #[test]
    fn already_present_block_yields_empty_diff() {
        let block = "\n## WARDEN guardrail — CONTEXT_BLOAT\n- Delegate broad search/file-reading to subagents; keep the main context for decisions and edits, not raw file dumps.\n";
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, format!("# Project\n{block}")).unwrap();

        let preview = preview_at("CONTEXT_BLOAT", &target);

        assert!(
            preview.diff.is_empty(),
            "idempotent: re-previewing an applied fix is a no-op; got:\n{}",
            preview.diff
        );
    }
}
