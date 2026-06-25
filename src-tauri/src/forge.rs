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
use crate::util::{claude_md_path, ensure_parent, sha256_hex};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::path::{Path, PathBuf};

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

/// Resolve the real target file a `pattern_id`'s guardrail lives in, reusing the
/// SAME `fix_for_pattern(...).target_path()` resolution the preview + apply paths
/// use (env-overridable `~/.claude/CLAUDE.md`). Exposed so Living-Habits Piece 3's
/// durable erase erases from the exact file apply wrote to — never a hardcoded path.
pub fn target_for_pattern(pattern_id: &str) -> PathBuf {
    fix_for_pattern(pattern_id).target_path()
}

/// The literal header prefix that marks a WARDEN guardrail section. The full
/// header line is `## WARDEN guardrail — {pattern_id}`.
const GUARDRAIL_HEADER_PREFIX: &str = "## WARDEN guardrail — ";

/// Build the exact header line for a pattern's guardrail section.
fn guardrail_header(pattern_id: &str) -> String {
    format!("{GUARDRAIL_HEADER_PREFIX}{pattern_id}")
}

/// Extract the `pattern_id` from a guardrail `block` by reading its header line
/// (`## WARDEN guardrail — {pattern_id}`). Returns `None` for a block that is not a
/// guardrail block (no such header), so callers can fall back to append-only.
fn pattern_id_of_block(block: &str) -> Option<String> {
    block.lines().find_map(|l| {
        l.trim_end()
            .strip_prefix(GUARDRAIL_HEADER_PREFIX)
            .map(|id| id.trim().to_string())
    })
}

/// `true` when `line` opens a top-level markdown section (`## ` heading). Used as
/// the section boundary when upserting/removing a guardrail block: a section runs
/// from its `## ` header up to (but not including) the next `## ` header or EOF.
/// (Deeper `###`/`####` headings are NOT boundaries — they nest inside a section.)
fn is_section_header(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("## ") && !t.starts_with("### ")
}

/// Idempotent UPSERT of a guardrail `block` keyed by its header `## WARDEN guardrail
/// — {pattern_id}`. This is the anti-bloat core: today's appender leaves a stale
/// block on disk whenever a guardrail's text changes, so the file grows a second
/// copy. Instead we REPLACE the existing section in place (header line → up to but
/// not including the next `## ` header or EOF), or INSERT at the end when absent.
///
/// Guarantees:
///  - Exactly one `## WARDEN guardrail — {pattern_id}` header survives (never two).
///  - An identical block re-applied is byte-identical (true idempotency).
///  - OTHER patterns' guardrail blocks and the user's surrounding prose are left
///    intact; only this pattern's section is rewritten.
///  - Surrounding blank lines are normalized so repeated upserts don't accumulate
///    blank lines (no slow whitespace bloat).
///
/// `block` is the canonical `\n## WARDEN guardrail — {pattern_id}\n{rule}\n` shape;
/// it is normalized to start/end on a single newline boundary before splicing.
fn upsert_block(current: &str, pattern_id: &str, block: &str) -> String {
    let header = guardrail_header(pattern_id);
    // The block's own body, trimmed of its leading/trailing blank lines so we
    // control the surrounding spacing ourselves (one blank line before, newline
    // after) — this is what makes repeated upserts converge to a fixed point.
    let block_core = block.trim_matches('\n');

    let lines: Vec<&str> = current.lines().collect();
    // Find this pattern's section: the line whose trimmed-end equals the header.
    let start = lines
        .iter()
        .position(|l| l.trim_end() == header);

    match start {
        Some(start) => {
            // Section end = next `## ` header after `start`, else EOF.
            let end = lines[start + 1..]
                .iter()
                .position(|l| is_section_header(l))
                .map(|rel| start + 1 + rel)
                .unwrap_or(lines.len());

            // Stitch: [before the section] + [new block] + [after the section].
            let before = &lines[..start];
            let after = &lines[end..];

            let mut out = String::new();
            // Preamble (everything before this section), trimmed of trailing blank
            // lines so we re-impose exactly one blank-line separator.
            let mut before_joined = before.join("\n");
            while before_joined.ends_with('\n') {
                before_joined.pop();
            }
            let before_trimmed = before_joined.trim_end_matches('\n');
            if !before_trimmed.is_empty() {
                out.push_str(before_trimmed);
                out.push_str("\n\n");
            }
            out.push_str(block_core);
            out.push('\n');

            // Trailing content (the next section onward), with its own leading blank
            // lines collapsed so we don't stack blanks between sections.
            let after_joined = after.join("\n");
            let after_trimmed = after_joined.trim_start_matches('\n');
            if !after_trimmed.is_empty() {
                out.push('\n');
                out.push_str(after_trimmed);
                // Preserve a single trailing newline if the original had any text.
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            out
        }
        None => {
            // Absent → append at the end. Preserve the user's content verbatim and
            // separate the new section with a single blank line.
            let mut out = current.to_string();
            // Normalize the seam: strip trailing newlines, then add exactly one
            // blank-line separator before the block (unless the file is empty).
            while out.ends_with('\n') {
                out.pop();
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(block_core);
            out.push('\n');
            out
        }
    }
}

/// Remove a guardrail section keyed by `pattern_id` (header line → up to but not
/// including the next `## ` header or EOF), leaving surrounding content and OTHER
/// patterns' blocks intact and tidy. Absent pattern → returns `current` unchanged
/// (a no-op). This is the implode counterpart to `upsert_block`.
fn remove_block(current: &str, pattern_id: &str) -> String {
    let header = guardrail_header(pattern_id);
    let lines: Vec<&str> = current.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == header) else {
        // Absent → exact no-op (byte-identical return).
        return current.to_string();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| is_section_header(l))
        .map(|rel| start + 1 + rel)
        .unwrap_or(lines.len());

    let before = &lines[..start];
    let after = &lines[end..];

    let mut before_joined = before.join("\n");
    while before_joined.ends_with('\n') {
        before_joined.pop();
    }
    let before_trimmed = before_joined.trim_end_matches('\n');
    let after_joined = after.join("\n");
    let after_trimmed = after_joined.trim_start_matches('\n');

    let mut out = String::new();
    out.push_str(before_trimmed);
    if !before_trimmed.is_empty() && !after_trimmed.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(after_trimmed);
    // Restore a single trailing newline if anything remains (matches the canonical
    // "file ends in newline" shape upsert produces). Empty result → empty string.
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Idempotently ensure `block` is present in `current`, REPLACING any prior copy of
/// the same guardrail (keyed by its `## WARDEN guardrail — {pattern_id}` header)
/// rather than appending a duplicate. Thin shim over `upsert_block`: it parses the
/// pattern_id out of the block's header so the two existing call sites (preview +
/// apply) keep their `(current, block)` signature unchanged. A block with no
/// guardrail header (defensive: should never happen for WARDEN fixes) falls back to
/// the legacy append-if-absent behavior.
fn ensure_block(current: &str, block: &str) -> String {
    match pattern_id_of_block(block) {
        Some(pattern_id) => upsert_block(current, &pattern_id, block),
        None => {
            // No recognizable header → legacy append-if-absent (never duplicates an
            // identical block; preserves old behavior for non-guardrail blocks).
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
    }
}

/// The literal guardrail block a given finding implies — the same per-pattern
/// block the preview appends. M4 `stage_artifact` records this on the artifact so
/// apply/revert never re-derive the pattern; the artifact row is the single source
/// of truth for what to write.
pub fn block_for_finding(finding: &Finding) -> String {
    fix_for_pattern(&finding.pattern_id).block
}

/// Outcome of an `apply_block`: whether the target's bytes actually changed (an
/// already-present block is a no-op), the backup file written (only when changed),
/// the SHA-256 of the pre-image, and the proposed content now on disk.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    /// True when the target was rewritten; false when the block was already present.
    pub changed: bool,
    /// Where the pre-image was backed up (only set when `changed`).
    pub backup_path: Option<PathBuf>,
    /// True when this apply wrote a NEW backup file. False when a backup already
    /// existed and was preserved (a re-apply): the caller must then keep the
    /// originally-recorded `pre_image_sha256`, not this run's, since the on-disk
    /// backup still holds the FIRST pre-image, not the current bytes.
    pub backup_written: bool,
    /// SHA-256 (hex) of the pre-image (the bytes that were on disk before apply).
    pub pre_image_sha256: String,
    /// SHA-256 (hex) of the content now on disk (the post-image). Recorded so revert
    /// can detect out-of-band edits since apply and refuse to clobber them.
    pub post_image_sha256: String,
    /// The content now on disk (equals the pre-image when unchanged).
    pub proposed: String,
}

/// Idempotently ensure `block` is present in `target`, with a backed-up,
/// atomic-ish write. This is the apply core (M4): NOT a patch engine — it
/// recomputes the proposed content with the same `ensure_block` the preview uses,
/// so re-applying is a guaranteed no-op.
///
/// Steps: read current (missing file → ""); `proposed = ensure_block(current)`;
/// if unchanged → return `changed:false`, no backup written. Else: copy the
/// current bytes to `backup_path` (recording its SHA) *before* touching the
/// target, then write `proposed` to a temp file in the target's directory and
/// `rename` it over the target (atomic on the same filesystem) so a crash
/// mid-write never leaves a half-file. Parent dirs are created as needed.
pub fn apply_block(target: &Path, block: &str, backup_path: &Path) -> Result<ApplyOutcome> {
    let current = read_target_utf8(target)?;
    let proposed = ensure_block(&current, block);
    commit_proposed(target, &current, proposed, backup_path)
}

/// Remove the guardrail section for `pattern_id` from `target` (the "implode"
/// counterpart to `apply_block`), through the SAME backup + sha256 + atomic-write
/// machinery — so a removal is exactly as reversible as a write. If the section is
/// absent the content is unchanged → `changed:false`, no backup written. Otherwise
/// the pre-image is backed up FIRST (never overwriting an existing backup), then the
/// section-stripped content is atomic-written. `revert_block` restores the backed-up
/// pre-image, putting the block right back — identical reversibility to apply.
pub fn remove_block_from_target(
    target: &Path,
    pattern_id: &str,
    backup_path: &Path,
) -> Result<ApplyOutcome> {
    let current = read_target_utf8(target)?;
    let proposed = remove_block(&current, pattern_id);
    commit_proposed(target, &current, proposed, backup_path)
}

/// Safe, data-loss-proof read of a guardrail target as a UTF-8 string.
///
/// SAFE-REFUSAL (data-loss fix): read the target as RAW BYTES, not via
/// `read_to_string(...).unwrap_or_default()`. The old path silently collapsed a
/// file holding invalid UTF-8 (binary / wrong-encoding content) to "", then
/// recorded an empty pre-image, wrote a 0-byte backup, and atomic-wrote
/// block-only content over the original — DESTROYING it irrecoverably (a later
/// revert would restore 0 bytes). A line-oriented Markdown guardrail has no
/// meaning against non-text content, so if the bytes are not valid UTF-8 we
/// REFUSE. A missing file is a fresh apply (pre-image = ""). A UTF-8 BOM is valid
/// UTF-8 and flows through unchanged.
fn read_target_utf8(target: &Path) -> Result<String> {
    match std::fs::read(target) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(s) => Ok(s.to_string()),
            Err(_) => Err(anyhow!(
                "target {} is not valid UTF-8; refusing to rewrite to avoid \
                 clobbering non-text content",
                target.display()
            )),
        },
        // Missing file → fresh apply with an empty pre-image (matches prior behavior).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(anyhow!("read target {}: {e}", target.display())),
    }
}

/// Shared write core for both apply (upsert) and implode (remove): given the
/// `current` bytes and the `proposed` rewrite, back up the pre-image and
/// atomic-write the proposal — or no-op when nothing changed. This is the single
/// source of truth for the backup/integrity/atomic-write behavior, so a removal is
/// just as reversible as a write.
fn commit_proposed(
    target: &Path,
    current: &str,
    proposed: String,
    backup_path: &Path,
) -> Result<ApplyOutcome> {
    let pre_image_sha256 = sha256_hex(current.as_bytes());
    let post_image_sha256 = sha256_hex(proposed.as_bytes());

    if proposed == *current {
        // No change (already present, already absent, or out-of-band applied) → no
        // write, no backup.
        return Ok(ApplyOutcome {
            changed: false,
            backup_path: None,
            backup_written: false,
            pre_image_sha256,
            post_image_sha256,
            proposed,
        });
    }

    // Backup the pre-image FIRST, but NEVER overwrite an existing backup. The first
    // backup is the true pristine pre-image; a re-apply after the user edited the
    // file out-of-band must not clobber it with the (now-edited) current bytes —
    // otherwise a later revert would restore the wrong baseline. If a backup already
    // exists for this artifact, keep it and do not re-record its sha. If this write
    // fails we abort before touching the target, so the original is never left in a
    // half-written state.
    ensure_parent(backup_path)?;
    let backup_preexisted = backup_path.exists();
    if !backup_preexisted {
        std::fs::write(backup_path, current.as_bytes())
            .map_err(|e| anyhow!("write backup {}: {e}", backup_path.display()))?;
    }

    ensure_parent(target)?;
    atomic_write(target, proposed.as_bytes())?;

    Ok(ApplyOutcome {
        changed: true,
        // Always report the (possibly pre-existing) backup path so the caller keeps
        // pointing the artifact at the pristine pre-image.
        backup_path: Some(backup_path.to_path_buf()),
        backup_written: !backup_preexisted,
        pre_image_sha256,
        post_image_sha256,
        proposed,
    })
}

/// Restore `target` from `backup_path`, refusing if the backup's content no longer
/// matches `expected_sha` (integrity over convenience). The backup bytes are
/// atomic-written over the target. A missing/tampered backup → typed error and the
/// target is left untouched.
pub fn revert_block(target: &Path, backup_path: &Path, expected_sha: &str) -> Result<()> {
    let backup = std::fs::read(backup_path)
        .map_err(|e| anyhow!("read backup {}: {e}", backup_path.display()))?;
    let actual_sha = sha256_hex(&backup);
    if actual_sha != expected_sha {
        return Err(anyhow!(
            "backup integrity check failed: expected sha {expected_sha}, got {actual_sha}"
        ));
    }
    ensure_parent(target)?;
    atomic_write(target, &backup)?;
    Ok(())
}

/// Write `bytes` to `path` atomically: write to a sibling temp file in the same
/// directory, then `rename` it over `path`. Rename is atomic on the same
/// filesystem, so a reader/crash never observes a partially written target. The
/// temp lives beside the target (not in `/tmp`) to keep the rename same-device.
///
/// Symlink-preserving: if `path` is a symlink (a common dotfiles setup where
/// `~/.claude/CLAUDE.md` links into a git repo), a plain rename-over would replace
/// the LINK with a regular file and silently sever it. We resolve the link to its
/// real destination and write there instead, so the symlink survives and future
/// edits to the linked-to copy stay reflected. Broken/dangling links fall through
/// to the normal path (the rename then creates the file the link points at).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    // Resolve a symlinked target to its real path so the link is preserved. Only
    // follow when the link actually points at an existing file; a dangling link
    // (canonicalize errs) falls back to writing at `path` directly.
    let resolved;
    let path = if std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        match std::fs::canonicalize(path) {
            Ok(real) => {
                resolved = real;
                resolved.as_path()
            }
            Err(_) => path,
        }
    } else {
        path
    };
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_string());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{file_name}.warden-tmp-{nanos}"));
    std::fs::write(&tmp, bytes).map_err(|e| anyhow!("write temp {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Best-effort cleanup of the temp on a failed rename.
        let _ = std::fs::remove_file(&tmp);
        anyhow!("rename {} -> {}: {e}", tmp.display(), path.display())
    })?;
    Ok(())
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

    // ── Piece 4: Clean Writes — pure upsert_block / remove_block (anti-bloat) ──

    /// Helper: count how many times a pattern's guardrail HEADER appears.
    fn header_count(s: &str, pattern_id: &str) -> usize {
        let header = guardrail_header(pattern_id);
        s.lines().filter(|l| l.trim_end() == header).count()
    }

    fn block_for(pattern_id: &str, rule: &str) -> String {
        format!("\n## WARDEN guardrail — {pattern_id}\n{rule}\n")
    }

    #[test]
    fn upsert_inserts_when_absent_header_appears_once() {
        let current = "# Project\n\nuser line\n";
        let block = block_for("UNVERIFIED_COMPLETION", "- rule one.");
        let out = upsert_block(current, "UNVERIFIED_COMPLETION", &block);

        // The user's content is preserved verbatim at the head.
        assert!(out.starts_with("# Project\n\nuser line\n"), "user content preserved; got:\n{out}");
        assert!(out.contains("## WARDEN guardrail — UNVERIFIED_COMPLETION"));
        assert!(out.contains("- rule one."));
        assert_eq!(
            header_count(&out, "UNVERIFIED_COMPLETION"),
            1,
            "exactly one header when inserting a fresh block; got:\n{out}"
        );
    }

    #[test]
    fn upsert_into_empty_string_inserts_block() {
        let block = block_for("CONTEXT_BLOAT", "- rule.");
        let out = upsert_block("", "CONTEXT_BLOAT", &block);
        assert!(out.contains("## WARDEN guardrail — CONTEXT_BLOAT"));
        assert!(out.contains("- rule."));
        assert_eq!(header_count(&out, "CONTEXT_BLOAT"), 1);
        // No leading blank line accumulated against an empty file.
        assert!(out.starts_with("## WARDEN guardrail — CONTEXT_BLOAT"), "got:\n{out}");
    }

    // THE ANTI-BLOAT CORE TEST: same pattern_id, DIFFERENT body → REPLACE in place.
    #[test]
    fn upsert_same_pattern_different_body_replaces_in_place_no_duplicate() {
        let current = "# Guide\n\nbase line\n";
        let v1 = block_for("UNVERIFIED_COMPLETION", "- OLD body text v1.");
        let after_v1 = upsert_block(current, "UNVERIFIED_COMPLETION", &v1);
        assert!(after_v1.contains("- OLD body text v1."));
        assert_eq!(header_count(&after_v1, "UNVERIFIED_COMPLETION"), 1);

        // Now upsert the SAME pattern with a DIFFERENT body.
        let v2 = block_for("UNVERIFIED_COMPLETION", "- NEW body text v2.");
        let after_v2 = upsert_block(&after_v1, "UNVERIFIED_COMPLETION", &v2);

        // The old body is GONE.
        assert!(
            !after_v2.contains("- OLD body text v1."),
            "stale body must be replaced, not left behind; got:\n{after_v2}"
        );
        // The new body is PRESENT.
        assert!(after_v2.contains("- NEW body text v2."), "new body must be present; got:\n{after_v2}");
        // The file did NOT grow a duplicate header — this is the whole point.
        assert_eq!(
            header_count(&after_v2, "UNVERIFIED_COMPLETION"),
            1,
            "exactly ONE header after replacing the body (no bloat); got:\n{after_v2}"
        );
        // The user's surrounding content survives.
        assert!(after_v2.contains("# Guide") && after_v2.contains("base line"));
    }

    #[test]
    fn upsert_is_idempotent_byte_identical_on_second_apply() {
        let current = "# Guide\n\nbase\n";
        let block = block_for("CACHE_COLD_RESTARTS", "- reuse a warm session.");
        let first = upsert_block(current, "CACHE_COLD_RESTARTS", &block);
        let second = upsert_block(&first, "CACHE_COLD_RESTARTS", &block);
        assert_eq!(first, second, "an identical upsert must be byte-identical (true idempotency)");
        // And a third pass still converges.
        let third = upsert_block(&second, "CACHE_COLD_RESTARTS", &block);
        assert_eq!(second, third, "repeated upserts must reach a fixed point (no blank-line bloat)");
    }

    #[test]
    fn upsert_leaves_other_patterns_and_surrounding_content_untouched() {
        let mut s = "# Guide\n\nintro paragraph\n".to_string();
        let a = block_for("UNVERIFIED_COMPLETION", "- verify before done.");
        let b = block_for("NO_DELEGATION", "- delegate discovery.");
        s = upsert_block(&s, "UNVERIFIED_COMPLETION", &a);
        s = upsert_block(&s, "NO_DELEGATION", &b);
        assert_eq!(header_count(&s, "UNVERIFIED_COMPLETION"), 1);
        assert_eq!(header_count(&s, "NO_DELEGATION"), 1);

        // Re-upsert A with a new body; B and the prose must be byte-stable.
        let a2 = block_for("UNVERIFIED_COMPLETION", "- verify with REAL output.");
        let s2 = upsert_block(&s, "UNVERIFIED_COMPLETION", &a2);
        assert!(s2.contains("- verify with REAL output."));
        assert!(!s2.contains("- verify before done."), "A's old body must be gone");
        // B's block is intact (header + body unchanged).
        assert!(s2.contains("## WARDEN guardrail — NO_DELEGATION"));
        assert!(s2.contains("- delegate discovery."), "B's body must be untouched");
        assert_eq!(header_count(&s2, "NO_DELEGATION"), 1);
        // The user's intro prose survives.
        assert!(s2.contains("# Guide") && s2.contains("intro paragraph"));
    }

    #[test]
    fn remove_deletes_section_and_leaves_rest_intact() {
        let mut s = "# Guide\n\nbase\n".to_string();
        let a = block_for("UNVERIFIED_COMPLETION", "- a rule.");
        let b = block_for("NO_DELEGATION", "- b rule.");
        s = upsert_block(&s, "UNVERIFIED_COMPLETION", &a);
        s = upsert_block(&s, "NO_DELEGATION", &b);

        let out = remove_block(&s, "UNVERIFIED_COMPLETION");
        assert_eq!(
            header_count(&out, "UNVERIFIED_COMPLETION"),
            0,
            "removed pattern's header must be gone; got:\n{out}"
        );
        assert!(!out.contains("- a rule."), "removed pattern's body must be gone; got:\n{out}");
        // The other pattern and the user's prose survive, tidy.
        assert!(out.contains("## WARDEN guardrail — NO_DELEGATION"));
        assert!(out.contains("- b rule."));
        assert!(out.contains("# Guide") && out.contains("base"));
        assert!(out.ends_with('\n'), "result should keep the canonical trailing newline; got:\n{out}");
    }

    #[test]
    fn remove_when_absent_is_a_noop() {
        let current = "# Guide\n\nbase line\n\n## Some user section\ncontent\n";
        let out = remove_block(current, "UNVERIFIED_COMPLETION");
        assert_eq!(out, current, "removing an absent pattern must be a byte-identical no-op");
    }

    #[test]
    fn upsert_then_remove_round_trips_other_content() {
        // A user block before and a user block after the guardrail must both survive
        // an upsert followed by a remove (the guardrail leaves no residue).
        let current = "# Guide\n\nbefore section\n\n## User Heading\nuser body\n";
        let block = block_for("WHACK_A_MOLE", "- stop patching symptoms.");
        let upserted = upsert_block(current, "WHACK_A_MOLE", &block);
        assert_eq!(header_count(&upserted, "WHACK_A_MOLE"), 1);
        assert!(upserted.contains("## User Heading") && upserted.contains("user body"));

        let removed = remove_block(&upserted, "WHACK_A_MOLE");
        assert_eq!(header_count(&removed, "WHACK_A_MOLE"), 0);
        // Both user sections survive the round trip.
        assert!(removed.contains("# Guide") && removed.contains("before section"));
        assert!(removed.contains("## User Heading") && removed.contains("user body"));
    }

    // ── M4 Forge apply / revert core (all temp-path targets — NEVER real config) ──

    const TEST_BLOCK: &str = "\n## WARDEN guardrail — UNVERIFIED_COMPLETION\n- Never claim done without running the build/tests and reading real output.\n";

    fn empty_sha() -> String {
        crate::util::sha256_hex(b"")
    }

    #[test]
    fn apply_block_writes_once_then_noop() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "# Project\n\nexisting line\n").unwrap();
        let backup = dir.path().join(".warden-bak/art-1.bak");

        // First apply changes the file and writes a backup of the pre-image.
        let pre_image = std::fs::read_to_string(&target).unwrap();
        let first = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(first.changed, "first apply must change the file");
        assert_eq!(first.backup_path.as_deref(), Some(backup.as_path()));
        assert_eq!(
            first.pre_image_sha256,
            crate::util::sha256_hex(pre_image.as_bytes())
        );
        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert!(on_disk.contains(TEST_BLOCK.trim()), "block must be present");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            pre_image,
            "backup must hold the exact pre-image"
        );

        // Second apply is an idempotent no-op: no change, no second backup, and the
        // block appears exactly once.
        let backup2 = dir.path().join(".warden-bak/art-1-second.bak");
        let second = apply_block(&target, TEST_BLOCK, &backup2).unwrap();
        assert!(!second.changed, "re-apply must be a no-op");
        assert!(second.backup_path.is_none(), "no backup on a no-op");
        assert!(!backup2.exists(), "no-op must not write a backup file");
        let after = std::fs::read_to_string(&target).unwrap();
        assert_eq!(after, on_disk, "file unchanged on re-apply");
        assert_eq!(
            after.matches(TEST_BLOCK.trim()).count(),
            1,
            "block must not stack / duplicate"
        );
    }

    #[test]
    fn apply_block_missing_target_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        // Nested missing parent dir → apply must create it (ensure_parent).
        let target = dir.path().join("nested/dir/CLAUDE.md");
        let backup = dir.path().join(".warden-bak/art-missing.bak");

        let outcome = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(outcome.changed);
        // Pre-image of a missing file is the empty string → sha("").
        assert_eq!(outcome.pre_image_sha256, empty_sha());
        assert!(target.exists(), "target file must be created");
        assert!(std::fs::read_to_string(&target).unwrap().contains(TEST_BLOCK.trim()));
        // Backup of the empty pre-image is an empty file.
        assert_eq!(std::fs::read(&backup).unwrap(), b"");
    }

    #[test]
    fn apply_block_writes_full_proposed_no_halfwrite() {
        // The atomic temp-then-rename path must leave the FULL proposed content on
        // disk — never a partial. We assert the final bytes equal the proposed.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "head\n").unwrap();
        let backup = dir.path().join(".warden-bak/art-atomic.bak");

        let outcome = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            on_disk, outcome.proposed,
            "on-disk content must equal the fully-formed proposed content"
        );
        // No stray temp files left behind in the target directory.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("warden-tmp"))
            .collect();
        assert!(strays.is_empty(), "no temp file should remain after rename");
    }

    #[test]
    fn revert_block_restores_pre_image() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let original = "# Project\n\nexisting line\n";
        std::fs::write(&target, original).unwrap();
        let backup = dir.path().join(".warden-bak/art-rev.bak");

        let outcome = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(outcome.changed);
        assert!(std::fs::read_to_string(&target).unwrap().contains(TEST_BLOCK.trim()));

        // Revert restores the exact pre-image.
        revert_block(&target, &backup, &outcome.pre_image_sha256).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), original);

        // Idempotent: re-previewing the restored file proposes the block again
        // (proving the guardrail is genuinely gone).
        let restored = std::fs::read_to_string(&target).unwrap();
        assert!(!restored.contains(TEST_BLOCK.trim()));
    }

    #[test]
    fn revert_block_refuses_on_sha_mismatch_and_leaves_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "# Project\n").unwrap();
        let backup = dir.path().join(".warden-bak/art-bad.bak");

        let outcome = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        let applied_content = std::fs::read_to_string(&target).unwrap();

        // Corrupt the backup AFTER recording the pre-image sha.
        std::fs::write(&backup, "tampered backup bytes").unwrap();

        let err = revert_block(&target, &backup, &outcome.pre_image_sha256).unwrap_err();
        assert!(
            err.to_string().contains("integrity check failed"),
            "expected integrity error, got: {err}"
        );
        // Target must be untouched (still the applied content).
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            applied_content,
            "target must not be modified when the backup integrity check fails"
        );
    }

    #[test]
    fn revert_block_missing_backup_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "x\n").unwrap();
        let backup = dir.path().join(".warden-bak/does-not-exist.bak");
        let err = revert_block(&target, &backup, &empty_sha()).unwrap_err();
        assert!(err.to_string().contains("read backup"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[test]
    fn apply_block_reports_post_image_sha_and_backup_written_flag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "seed\n").unwrap();
        let backup = dir.path().join(".warden-bak/art-post.bak");

        let out = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        // post-image sha equals an independent sha of the proposed content on disk.
        assert_eq!(
            out.post_image_sha256,
            crate::util::sha256_hex(std::fs::read(&target).unwrap().as_slice())
        );
        assert!(out.backup_written, "fresh backup must be reported as written");

        // A no-op re-apply still reports the (matching) post-image sha and no write.
        let backup2 = dir.path().join(".warden-bak/art-post-2.bak");
        let noop = apply_block(&target, TEST_BLOCK, &backup2).unwrap();
        assert!(!noop.changed);
        assert!(!noop.backup_written);
        assert_eq!(noop.post_image_sha256, out.post_image_sha256);
    }

    #[test]
    fn apply_block_never_overwrites_an_existing_backup() {
        // A changing apply that finds a pre-existing backup must PRESERVE it and
        // report backup_written=false, so the caller keeps the first pre-image.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "ORIGINAL\n").unwrap();
        let backup = dir.path().join(".warden-bak/art-keep.bak");

        let first = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(first.changed && first.backup_written);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "ORIGINAL\n");

        // Simulate the user removing the block + editing out-of-band, then re-apply.
        std::fs::write(&target, "USER EDIT\n").unwrap();
        let second = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(second.changed, "block re-added → changing apply");
        assert!(!second.backup_written, "pre-existing backup must be preserved");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "ORIGINAL\n",
            "the pristine pre-image must survive a changing re-apply"
        );
    }

    #[test]
    fn apply_block_refuses_invalid_utf8_target_and_preserves_bytes() {
        // SAFE-REFUSAL regression: a target holding strictly-invalid UTF-8 must make
        // apply REFUSE (typed Err), leave the file BYTE-IDENTICAL, and write NO backup.
        // Before the fix, read_to_string(...).unwrap_or_default() collapsed these bytes
        // to "", recorded an empty pre-image, wrote a 0-byte backup, and atomic-wrote
        // block-only content over the original — total, irrecoverable content loss.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        // 0xff 0xfe is not a valid UTF-8 lead byte; 0x80 is a stray continuation byte.
        let binary: &[u8] = &[0xff, 0xfe, b'h', b'i', 0x00, 0x80];
        std::fs::write(&target, binary).unwrap();
        let backup = dir.path().join(".warden-bak/art-binary.bak");

        let err = apply_block(&target, TEST_BLOCK, &backup)
            .expect_err("apply must REFUSE a non-UTF8 target");
        assert!(
            err.to_string().contains("not valid UTF-8"),
            "expected a UTF-8 refusal error, got: {err}"
        );
        // Target is byte-for-byte unchanged.
        assert_eq!(
            std::fs::read(&target).unwrap(),
            binary,
            "the non-text target must be left BYTE-IDENTICAL after a refused apply"
        );
        // No backup file was created (nothing was written anywhere).
        assert!(
            !backup.exists(),
            "a refused apply must create NO backup file"
        );
        assert!(
            !dir.path().join(".warden-bak").exists()
                || std::fs::read_dir(dir.path().join(".warden-bak"))
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true),
            "a refused apply must not leave a backup behind"
        );
    }

    #[test]
    fn apply_then_revert_round_trips_a_utf8_bom_file() {
        // A UTF-8 BOM (U+FEFF, bytes EF BB BF) is VALID UTF-8 and must keep working:
        // apply appends the block, the BOM is preserved at the head, and revert
        // restores the file byte-for-byte (BOM and all).
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let original = "\u{feff}# Title\n\nexisting line\n";
        std::fs::write(&target, original).unwrap();
        let original_bytes = std::fs::read(&target).unwrap();
        assert_eq!(
            &original_bytes[..3],
            &[0xef, 0xbb, 0xbf],
            "sanity: file begins with the UTF-8 BOM bytes"
        );
        let backup = dir.path().join(".warden-bak/art-bom.bak");

        // Apply succeeds: BOM preserved at the head, block appended.
        let out = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(out.changed, "BOM file must apply (it is valid UTF-8)");
        let after = std::fs::read(&target).unwrap();
        assert_eq!(
            &after[..3],
            &[0xef, 0xbb, 0xbf],
            "BOM must be preserved at the head of the applied file"
        );
        assert!(
            String::from_utf8_lossy(&after).contains(TEST_BLOCK.trim()),
            "block must be appended to the BOM file"
        );
        // Backup holds the exact original (BOM included).
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            original_bytes,
            "backup must hold the exact original bytes including the BOM"
        );

        // Revert restores the file byte-for-byte (BOM survives the round-trip).
        revert_block(&target, &backup, &out.pre_image_sha256).unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap(),
            original_bytes,
            "revert must restore the BOM file BYTE-IDENTICAL"
        );
    }

    // ── Piece 4: removal apply-path (implode) goes through the SAME backup/sha/revert ──

    /// The pattern_id that TEST_BLOCK carries (its header is
    /// `## WARDEN guardrail — UNVERIFIED_COMPLETION`).
    const TEST_PATTERN: &str = "UNVERIFIED_COMPLETION";

    #[test]
    fn remove_block_from_target_strips_section_and_backs_up_pre_image() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let original = "# Project\n\nexisting line\n";
        std::fs::write(&target, original).unwrap();

        // Apply the block first (so there is something to implode).
        let apply_backup = dir.path().join(".warden-bak/apply.bak");
        let applied = apply_block(&target, TEST_BLOCK, &apply_backup).unwrap();
        assert!(applied.changed);
        let after_apply = std::fs::read_to_string(&target).unwrap();
        assert!(after_apply.contains(TEST_BLOCK.trim()), "block present after apply");

        // Implode (remove) the block through the removal apply-path.
        let remove_backup = dir.path().join(".warden-bak/remove.bak");
        let removed = remove_block_from_target(&target, TEST_PATTERN, &remove_backup).unwrap();
        assert!(removed.changed, "removing a present block must change the file");
        assert!(removed.backup_written, "removal must back up its pre-image");
        // The block is gone; the user's content survives.
        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert!(!on_disk.contains(TEST_BLOCK.trim()), "block must be gone after implode");
        assert!(on_disk.contains("existing line"), "user content must survive implode");
        // The removal backup holds the pre-removal image (the file WITH the block).
        assert_eq!(
            std::fs::read_to_string(&remove_backup).unwrap(),
            after_apply,
            "removal backup must hold the exact pre-image (the applied content)"
        );
        // The recorded pre-image sha matches that backup → revert is exact.
        assert_eq!(
            removed.pre_image_sha256,
            crate::util::sha256_hex(after_apply.as_bytes())
        );
    }

    #[test]
    fn remove_block_from_target_is_a_noop_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let original = "# Project\n\nno guardrail here\n";
        std::fs::write(&target, original).unwrap();
        let backup = dir.path().join(".warden-bak/none.bak");

        let outcome = remove_block_from_target(&target, TEST_PATTERN, &backup).unwrap();
        assert!(!outcome.changed, "removing an absent block must be a no-op");
        assert!(!outcome.backup_written, "no-op removal must not write a backup");
        assert!(!backup.exists(), "no-op removal must not create a backup file");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            original,
            "no-op removal must leave the file untouched"
        );
    }

    #[test]
    fn remove_then_revert_restores_the_block_same_reversibility_as_apply() {
        // A removal must be exactly as reversible as a write: revert_block restores
        // the backed-up pre-image, putting the imploded block right back.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "# Project\n\nbody\n").unwrap();

        let apply_backup = dir.path().join(".warden-bak/apply.bak");
        apply_block(&target, TEST_BLOCK, &apply_backup).unwrap();
        let with_block = std::fs::read_to_string(&target).unwrap();

        // Implode.
        let remove_backup = dir.path().join(".warden-bak/remove.bak");
        let removed = remove_block_from_target(&target, TEST_PATTERN, &remove_backup).unwrap();
        assert!(!std::fs::read_to_string(&target).unwrap().contains(TEST_BLOCK.trim()));

        // Revert the removal → the block is back, byte-for-byte.
        revert_block(&target, &remove_backup, &removed.pre_image_sha256).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            with_block,
            "reverting a removal must restore the block exactly (same reversibility as apply)"
        );
    }

    #[test]
    fn remove_block_from_target_refuses_invalid_utf8_and_preserves_bytes() {
        // The removal apply-path shares apply's SAFE-REFUSAL: a non-UTF8 target must
        // make removal REFUSE, leave the file byte-identical, and write NO backup.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let binary: &[u8] = &[0xff, 0xfe, b'h', b'i', 0x00, 0x80];
        std::fs::write(&target, binary).unwrap();
        let backup = dir.path().join(".warden-bak/bin.bak");

        let err = remove_block_from_target(&target, TEST_PATTERN, &backup)
            .expect_err("removal must REFUSE a non-UTF8 target");
        assert!(err.to_string().contains("not valid UTF-8"), "got: {err}");
        assert_eq!(std::fs::read(&target).unwrap(), binary, "non-text target must be byte-identical");
        assert!(!backup.exists(), "a refused removal must create NO backup");
    }

    #[test]
    fn apply_revert_round_trip_restores_original_bytes_exactly() {
        // M4 integrity preserved under the upsert change: a full apply→revert cycle
        // restores the ORIGINAL bytes verbatim (not just "block gone").
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CLAUDE.md");
        let original = "# Project\n\nline A\nline B\n\n## Existing user section\nkeep me\n";
        std::fs::write(&target, original).unwrap();
        let original_bytes = std::fs::read(&target).unwrap();
        let backup = dir.path().join(".warden-bak/roundtrip.bak");

        let out = apply_block(&target, TEST_BLOCK, &backup).unwrap();
        assert!(out.changed);
        assert!(std::fs::read_to_string(&target).unwrap().contains(TEST_BLOCK.trim()));

        revert_block(&target, &backup, &out.pre_image_sha256).unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap(),
            original_bytes,
            "apply→revert must restore the ORIGINAL file BYTE-IDENTICAL (M4 integrity)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_a_symlinked_target() {
        // ~/.claude/CLAUDE.md is often a symlink into a dotfiles repo. Apply must
        // write THROUGH the link (preserving it), not replace the link with a file.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles/CLAUDE.md");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "seed\n").unwrap();
        let link = dir.path().join("CLAUDE.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let backup = dir.path().join(".warden-bak/art-link.bak");

        let out = apply_block(&link, TEST_BLOCK, &backup).unwrap();
        assert!(out.changed);
        // The link is still a symlink (not severed into a regular file).
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "apply must preserve the symlink, not replace it with a regular file"
        );
        // The block landed in the REAL linked-to file, visible through the link.
        assert!(std::fs::read_to_string(&link).unwrap().contains(TEST_BLOCK.trim()));
        assert!(std::fs::read_to_string(&real).unwrap().contains(TEST_BLOCK.trim()));
    }
}
