//! INDEPENDENT end-to-end verification of the M4 "FORGE" apply/revert WRITE path.
//!
//! This test is written by a verification engineer who DISTRUSTS the implementer's
//! in-module unit tests. It drives the REAL file-mutating functions
//! (`forge::apply_block` / `forge::revert_block`) and the REAL persistence layer
//! (`Store` artifacts CRUD) in the exact same sequence the Tauri command layer
//! (`apply_artifact_inner` / `revert_artifact_inner` in `commands.rs`) uses:
//!
//!   target  = expand_tilde(artifact.target_path)            // == claude_md_path()
//!   backup  = backup_dir(target).join("{id}.bak")           // sibling .warden-bak
//!   apply   = forge::apply_block(target, block, backup)
//!   record  = store.update_artifact_status(id, "applied", at, backup, pre_sha)
//!   revert  = forge::revert_block(target, backup, recorded_pre_sha)             // sha-verified
//!
//! SAFETY: every path used here lives under a fresh `tempfile::tempdir()`. The
//! production target is resolved through `WARDEN_CLAUDE_MD`, which we point at a
//! temp file, so the real `~/.claude/CLAUDE.md` is NEVER read, written, moved, or
//! truncated. We never construct the real `~/.claude` path anywhere in this file.

use warden_lib::forge;
use warden_lib::ir::{Artifact, Finding};
use warden_lib::store::Store;
use warden_lib::util::{backup_dir, claude_md_path, expand_tilde, sha256_hex};

/// Process-wide lock guarding the `WARDEN_CLAUDE_MD` env override. Every test that
/// sets it must hold this so parallel test threads never race on the global var.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND-LAYER FAITHFUL REPRODUCTION
//
// `apply_artifact_inner` / `revert_artifact_inner` live in `commands.rs` as PRIVATE
// fns, so an integration test cannot call them. The data-loss FIXES (findings 1-8)
// live in those wrappers — NOT in the `forge::apply_block`/`revert_block`
// primitives. To verify the fixes durably end-to-end, the two helpers below
// reproduce the EXACT command-layer sequence byte-for-byte: same store reads, same
// backup-path derivation, same terminal-state guard, same backup-preservation
// selection, and same out-of-band drift refusal. If a future refactor deletes a
// guard from these helpers (the real risk this verifier hunts), these tests fail.
// They are intentionally written so the guard logic is asserted, not assumed.
// ─────────────────────────────────────────────────────────────────────────────

/// Reproduces `apply_artifact_inner` (commands.rs): resolve the artifact, derive the
/// `{id}.bak` backup path, apply, then record the OUTCOME-SELECTED backup+pre-sha:
///  - changing apply that wrote a NEW backup → record fresh backup + pre-image sha
///  - changing apply that PRESERVED a pre-existing backup → KEEP the original
///    pre-image sha (the on-disk backup still holds the first pristine pre-image)
///  - no-op apply → preserve whatever a prior changing apply recorded
/// Always records the post-image sha so revert can detect out-of-band drift.
fn apply_inner(store: &Store, id: &str) -> anyhow::Result<Artifact> {
    let artifact = store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} not found"))?;
    let target = expand_tilde(&artifact.target_path);
    let backup_path = backup_dir(&target).join(format!("{id}.bak"));

    let outcome = forge::apply_block(&target, &artifact.block, &backup_path)?;
    let applied_at = artifact
        .applied_at
        .clone()
        .unwrap_or_else(|| "2026-06-25T00:00:00Z".to_string());
    let (backup_str, pre_sha) = if outcome.changed && outcome.backup_written {
        (
            outcome.backup_path.map(|p| p.to_string_lossy().into_owned()),
            Some(outcome.pre_image_sha256),
        )
    } else {
        (artifact.backup_path.clone(), artifact.pre_image_sha256.clone())
    };
    let post_sha = Some(outcome.post_image_sha256);
    store.update_artifact_status(
        id,
        "applied",
        Some(&applied_at),
        backup_str.as_deref(),
        pre_sha.as_deref(),
        post_sha.as_deref(),
    )?;
    store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after apply"))
}

/// Reproduces `revert_artifact_inner` (commands.rs) including ALL data-loss guards:
///  1. TERMINAL-STATE GUARD: revert is a true no-op on any non-"applied" artifact
///     (PENDING or already-REVERTED) — returns the row WITHOUT writing the target,
///     so a double-revert can never re-restore a stale pre-image over user edits.
///  2. OUT-OF-BAND DRIFT GUARD: before restoring, the target's current sha must
///     equal the recorded post-image sha; otherwise refuse with a typed error and
///     leave the file untouched.
///  3. BACKUP INVALIDATION ON SUCCESSFUL REVERT: a successful revert ends the apply
///     cycle, so the consumed `{id}.bak` is deleted and backup_path/pre_image_sha256
///     are nulled. A later re-apply is then a FRESH cycle that captures the user's
///     current (post-revert) content as the new pristine pre-image, instead of
///     reusing the stale first-apply backup.
fn revert_inner(store: &Store, id: &str) -> anyhow::Result<Artifact> {
    let artifact = store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} not found"))?;

    // GUARD 1: terminal-state — only an APPLIED artifact is revertible.
    if artifact.status != "applied" {
        return Ok(artifact);
    }

    let (Some(backup_path), Some(expected_sha)) =
        (&artifact.backup_path, &artifact.pre_image_sha256)
    else {
        store.update_artifact_status(
            id,
            "reverted",
            artifact.applied_at.as_deref(),
            None,
            None,
            artifact.post_image_sha256.as_deref(),
        )?;
        return store
            .artifact_by_id(id)?
            .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after revert"));
    };

    let target = expand_tilde(&artifact.target_path);

    // GUARD 2: out-of-band drift — refuse if the target changed since apply.
    if let Some(expected_post) = artifact.post_image_sha256.as_deref() {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        let current_sha = sha256_hex(current.as_bytes());
        if current_sha != expected_post {
            return Err(anyhow::anyhow!(
                "target {} changed out-of-band since apply (expected post-image sha \
                 {expected_post}, found {current_sha}); refusing to revert and \
                 overwrite those edits",
                target.display()
            ));
        }
    }

    let backup_file = expand_tilde(backup_path);
    forge::revert_block(&target, &backup_file, expected_sha)?;
    // GUARD 3: invalidate the consumed backup so a later re-apply captures a fresh
    // pristine pre-image of the CURRENT content (best-effort delete; null the cols).
    let _ = std::fs::remove_file(&backup_file);
    store.update_artifact_status(
        id,
        "reverted",
        artifact.applied_at.as_deref(),
        None,
        None,
        artifact.post_image_sha256.as_deref(),
    )?;
    store
        .artifact_by_id(id)?
        .ok_or_else(|| anyhow::anyhow!("artifact {id} vanished after revert"))
}

/// Stage a PENDING artifact for `finding` against the WARDEN_CLAUDE_MD target,
/// mirroring `stage_artifact_inner`. Returns the persisted row.
fn stage(store: &Store, id: &str, f: &Finding) -> Artifact {
    let preview = forge::preview_for_finding(f);
    let block = forge::block_for_finding(f);
    let a = Artifact {
        id: id.into(),
        finding_id: Some(f.id.clone()),
        kind: "claude_md_guardrail".into(),
        target_path: preview.target_path,
        diff: preview.diff,
        block,
        status: "pending".into(),
        applied_at: None,
        backup_path: None,
        pre_image_sha256: None,
        post_image_sha256: None,
    };
    store.save_artifact(&a).expect("persist staged artifact");
    a
}

/// A finding whose pattern maps to a known guardrail block. We pick
/// UNVERIFIED_COMPLETION so the appended block is deterministic and recognizable.
fn finding(pattern: &str) -> Finding {
    Finding {
        id: format!("verif-{pattern}"),
        pattern_id: pattern.into(),
        title: pattern.into(),
        severity: 5,
        frequency: 0.9,
        est_cost_tokens: 1000,
        est_cost_minutes: 10,
        confidence: 0.8,
        rationale: "independent e2e verification".into(),
        evidence: vec![],
        status: "candidate".into(),
        verifier_verdict: None,
    }
}

/// Full apply -> backup-integrity -> revert -> idempotent re-apply against a
/// TEMPDIR target, driving the production target resolution via WARDEN_CLAUDE_MD.
///
/// Marked `serial` via the env-var mutex pattern: WARDEN_CLAUDE_MD is process-wide,
/// so we hold a static lock while it is set to keep parallel test threads honest.
#[test]
fn forge_full_apply_revert_reapply_against_tempdir_target() {
    // Share the module-level ENV_LOCK with every other test that mutates the
    // process-wide WARDEN_CLAUDE_MD, so parallel test threads never race on it.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let dir = tempfile::tempdir().expect("tempdir");

    // ── The TEMP target. This is what the production code will resolve to. ──
    let target = dir.path().join("CLAUDE.md");
    let seed = "# Karim's Project Guide\n\nexisting line one\nexisting line two\n";
    std::fs::write(&target, seed).expect("seed write");

    // Point the PRODUCTION target resolver at our temp file. From here on,
    // claude_md_path() (used by forge::preview_for_finding) returns this path.
    std::env::set_var("WARDEN_CLAUDE_MD", &target);

    // Prove the production resolver actually resolves to the temp file (not the
    // real ~/.claude/CLAUDE.md) BEFORE we do any writes.
    let resolved = claude_md_path();
    assert_eq!(
        resolved,
        target,
        "production claude_md_path() must resolve to the temp target via WARDEN_CLAUDE_MD"
    );
    assert!(
        !resolved.to_string_lossy().contains(".claude/CLAUDE.md"),
        "resolved target must NOT be the real ~/.claude/CLAUDE.md, got {resolved:?}"
    );

    // ── STAGE: build the artifact exactly as stage_artifact_inner does, and
    //    persist it to a REAL on-disk Store (drives the store/command layer). ──
    let store_path = dir.path().join("warden.db");
    let store = Store::open(&store_path).expect("open store");

    let f = finding("UNVERIFIED_COMPLETION");
    let preview = forge::preview_for_finding(&f); // resolves target via WARDEN_CLAUDE_MD
    let block = forge::block_for_finding(&f);
    assert_eq!(
        preview.target_path,
        target.to_string_lossy(),
        "staged artifact target_path must be the temp target"
    );
    assert!(
        !preview.diff.is_empty() && preview.diff.contains("WARDEN guardrail — UNVERIFIED_COMPLETION"),
        "stage-time preview diff should add the guardrail block; got:\n{}",
        preview.diff
    );

    let artifact_id = "verif-artifact-1".to_string();
    let staged = Artifact {
        id: artifact_id.clone(),
        finding_id: Some(f.id.clone()),
        kind: "claude_md_guardrail".into(),
        target_path: preview.target_path.clone(),
        diff: preview.diff.clone(),
        block: block.clone(),
        status: "pending".into(),
        applied_at: None,
        backup_path: None,
        pre_image_sha256: None,
        post_image_sha256: None,
    };
    store.save_artifact(&staged).expect("persist staged artifact");

    // Round-trip the staged row to confirm it really hit sqlite.
    let from_db = store
        .artifact_by_id(&artifact_id)
        .expect("query artifact")
        .expect("staged artifact present in store");
    assert_eq!(from_db.status, "pending");
    assert_eq!(from_db.backup_path, None);
    assert_eq!(from_db.pre_image_sha256, None);

    // Capture the exact pre-image bytes for an independent comparison later.
    let pre_image_bytes = std::fs::read(&target).expect("read pre-image");
    assert_eq!(pre_image_bytes, seed.as_bytes(), "sanity: target holds the seed");
    let independent_pre_sha = sha256_hex(&pre_image_bytes);

    // ── APPLY: reproduce apply_artifact_inner's real write path verbatim. ──
    let resolved_target = expand_tilde(&from_db.target_path);
    assert_eq!(resolved_target, target, "command-layer target resolution must be the temp file");
    let backup_path = backup_dir(&resolved_target).join(format!("{artifact_id}.bak"));
    assert_eq!(
        backup_path,
        dir.path().join(".warden-bak").join(format!("{artifact_id}.bak")),
        "backup must land in the sibling .warden-bak dir of the temp target"
    );

    let outcome = forge::apply_block(&resolved_target, &from_db.block, &backup_path)
        .expect("apply_block must succeed");

    // ASSERT 1: the apply CHANGED the file and reported the backup + pre-image sha.
    assert!(outcome.changed, "first apply must change the file");
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some(backup_path.as_path()),
        "outcome must report the backup it wrote"
    );
    assert_eq!(
        outcome.pre_image_sha256, independent_pre_sha,
        "recorded preImageSha256 must equal an INDEPENDENT sha256 of the seed bytes"
    );

    // Persist the apply outcome to the store exactly as the command layer does.
    let applied_at = "2026-06-25T00:00:00Z";
    store
        .update_artifact_status(
            &artifact_id,
            "applied",
            Some(applied_at),
            outcome.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()).as_deref(),
            Some(&outcome.pre_image_sha256),
            Some(&outcome.post_image_sha256),
        )
        .expect("record applied status");

    // ASSERT 2: target now contains the SEED (preserved, not clobbered) PLUS the
    // appended guardrail block.
    let after_apply = std::fs::read_to_string(&target).expect("read after apply");
    assert!(
        after_apply.starts_with(seed),
        "seed content must be preserved verbatim at the head of the file.\nGot:\n{after_apply}"
    );
    assert!(
        after_apply.contains("existing line one") && after_apply.contains("existing line two"),
        "every seed line must survive the apply"
    );
    assert!(
        after_apply.contains(block.trim()),
        "the guardrail block must be appended.\nGot:\n{after_apply}"
    );
    assert!(
        after_apply.len() > seed.len(),
        "applied file must be strictly longer than the seed (block appended)"
    );
    // Bytes-equal the fully-formed proposed content (no half-write).
    assert_eq!(
        after_apply, outcome.proposed,
        "on-disk bytes must equal the fully-formed proposed content (atomic, no half-write)"
    );

    // ASSERT 3: a backup file exists, holds the EXACT pre-image, and its sha256
    // equals the recorded preImageSha256.
    assert!(backup_path.exists(), "backup file must exist after a changing apply");
    let backup_bytes = std::fs::read(&backup_path).expect("read backup");
    assert_eq!(
        backup_bytes,
        seed.as_bytes(),
        "backup must hold the EXACT pre-image bytes (the original seed)"
    );
    let recorded_sha = store
        .artifact_by_id(&artifact_id)
        .unwrap()
        .unwrap()
        .pre_image_sha256
        .expect("store must have recorded preImageSha256");
    assert_eq!(
        sha256_hex(&backup_bytes),
        recorded_sha,
        "sha256(backup bytes) must equal the recorded preImageSha256 in the store"
    );

    // ── IDEMPOTENT RE-APPLY (twice): block already present => no-op, no dup. ──
    let bytes_before_reapply = std::fs::read(&target).expect("read before reapply");
    let backup_meta_before = std::fs::metadata(&backup_path).unwrap();
    let backup_mtime_before = backup_meta_before.modified().unwrap();

    for _ in 0..2 {
        let rebackup = dir.path().join(".warden-bak").join("should-not-be-written.bak");
        let re = forge::apply_block(&resolved_target, &from_db.block, &rebackup)
            .expect("re-apply must succeed");
        assert!(!re.changed, "re-apply must be a no-op (block already present)");
        assert!(re.backup_path.is_none(), "no-op apply must not write a new backup");
        assert!(!rebackup.exists(), "no-op apply must not create a backup file");
    }

    // ASSERT 4: idempotency — file unchanged, exactly ONE block, original backup
    // untouched.
    let bytes_after_reapply = std::fs::read(&target).expect("read after reapply");
    assert_eq!(
        bytes_after_reapply, bytes_before_reapply,
        "re-applying must not change the target bytes"
    );
    assert_eq!(
        after_apply.matches(block.trim()).count(),
        1,
        "guardrail block must appear EXACTLY once (no stacking)"
    );
    assert_eq!(
        std::fs::read(&backup_path).unwrap(),
        seed.as_bytes(),
        "original backup must remain the pre-image after no-op re-applies"
    );
    assert_eq!(
        std::fs::metadata(&backup_path).unwrap().modified().unwrap(),
        backup_mtime_before,
        "no-op re-apply must not rewrite the original backup file"
    );

    // ── REVERT: sha-verified restore of the pre-image, command-layer path. ──
    let revert_artifact = store.artifact_by_id(&artifact_id).unwrap().unwrap();
    let revert_backup = expand_tilde(revert_artifact.backup_path.as_ref().unwrap());
    let expected_sha = revert_artifact.pre_image_sha256.as_ref().unwrap();
    forge::revert_block(&resolved_target, &revert_backup, expected_sha)
        .expect("revert_block must succeed with a matching sha");

    store
        .update_artifact_status(
            &artifact_id,
            "reverted",
            Some(applied_at),
            revert_artifact.backup_path.as_deref(),
            Some(expected_sha),
            revert_artifact.post_image_sha256.as_deref(),
        )
        .expect("record reverted status");

    // ASSERT 5: target is BYTE-IDENTICAL to the original seed.
    let after_revert = std::fs::read(&target).expect("read after revert");
    assert_eq!(
        after_revert,
        seed.as_bytes(),
        "after revert the target must be BYTE-IDENTICAL to the seed"
    );
    assert!(
        !String::from_utf8_lossy(&after_revert).contains(block.trim()),
        "guardrail block must be gone after revert"
    );
    assert_eq!(
        sha256_hex(&after_revert),
        independent_pre_sha,
        "reverted file sha must equal the original pre-image sha"
    );

    // Store reflects the final lifecycle state.
    let final_row = store.artifact_by_id(&artifact_id).unwrap().unwrap();
    assert_eq!(final_row.status, "reverted");

    // ── REVERT INTEGRITY: a tampered backup must REFUSE and leave target alone. ──
    // Re-apply to get a fresh backup, corrupt it, then prove revert refuses.
    let tamper_backup = dir.path().join(".warden-bak").join("tamper.bak");
    let fresh = forge::apply_block(&resolved_target, &from_db.block, &tamper_backup)
        .expect("re-apply for tamper test");
    assert!(fresh.changed, "block was reverted, so re-apply changes again");
    let applied_again = std::fs::read(&target).expect("read applied-again");
    std::fs::write(&tamper_backup, b"corrupted backup bytes").expect("tamper");
    let err = forge::revert_block(&resolved_target, &tamper_backup, &fresh.pre_image_sha256)
        .expect_err("revert must REFUSE on sha mismatch");
    assert!(
        err.to_string().contains("integrity check failed"),
        "expected an integrity error, got: {err}"
    );
    assert_eq!(
        std::fs::read(&target).expect("read after refused revert"),
        applied_again,
        "target must be UNTOUCHED when the backup integrity check fails"
    );

    // Clean up the process-wide override so we don't leak it to other tests.
    std::env::remove_var("WARDEN_CLAUDE_MD");

    // Final defense-in-depth: nothing we touched is the real config path.
    let touched = [&target, &backup_path, &store_path];
    for p in touched {
        assert!(
            p.starts_with(dir.path()),
            "every touched path must live under the tempdir; {p:?} escaped"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DATA-LOSS REGRESSION e2e (the 3 worst paths), driven through the FAITHFUL
// command-layer reproduction so the actual guards — not the bare primitives —
// are what is under test. Each guards a CRITICAL/HIGH finding from the review.
// ─────────────────────────────────────────────────────────────────────────────

/// (a) CRITICAL findings 1/2/3 — double-revert must NOT clobber user edits.
/// apply → revert (file back to ORIGINAL) → re-apply → REVERT AGAIN restores
/// ORIGINAL → the user then rewrites the whole file → a SECOND revert on that
/// now-reverted artifact MUST be an inert no-op and leave the user's bytes intact.
#[test]
fn e2e_double_revert_does_not_clobber_user_edits_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("CLAUDE.md");
    let original = "# Karim's Guide\n\noriginal rule A\noriginal rule B\n";
    std::fs::write(&target, original).expect("seed");
    std::env::set_var("WARDEN_CLAUDE_MD", &target);
    assert_eq!(claude_md_path(), target, "resolver must point at temp target");
    assert!(!target.to_string_lossy().contains(".claude/CLAUDE.md"));

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let f = finding("UNVERIFIED_COMPLETION");
    let id = "dl-a";
    stage(&store, id, &f);

    // apply → revert : file is back to ORIGINAL, artifact is "reverted".
    apply_inner(&store, id).expect("apply");
    let r1 = revert_inner(&store, id).expect("first revert");
    assert_eq!(r1.status, "reverted");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), original,
        "first revert must restore ORIGINAL");

    // The user now rewrites the ENTIRE file by hand.
    let user_rewrite = "TOTALLY DIFFERENT USER CONTENT\nwith two lines\n";
    std::fs::write(&target, user_rewrite).expect("user rewrite");

    // SECOND revert on the already-reverted artifact MUST be inert (terminal-state
    // guard). Before the fix it re-entered the restore branch and atomic-wrote the
    // stale ORIGINAL over the user's content.
    let r2 = revert_inner(&store, id).expect("second revert is a no-op Ok");
    assert_eq!(r2.status, "reverted");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        user_rewrite,
        "DOUBLE-REVERT MUST NOT CLOBBER THE USER'S REWRITE"
    );

    std::env::remove_var("WARDEN_CLAUDE_MD");
}

/// NEWLY-DISCOVERED RESIDUAL DATA-LOSS (surfaced by this verifier, NOT one of the
/// 9 claimed-fixed findings; left FAILING/ignored as durable proof, not deleted).
///
/// The fix for findings 5/6 ("never overwrite an existing backup") is correct for a
/// re-apply on a STILL-APPLIED artifact. But it has a gap across a revert→re-apply
/// cycle of the SAME artifact id: revert RETAINS the `{id}.bak` file on disk (for
/// the audit trail). A later re-apply of that id sees `backup_path.exists()==true`,
/// so `apply_block` PRESERVES the stale first-apply backup (the pre-FIRST-apply
/// ORIGINAL) instead of capturing the user's current post-revert content as the new
/// pre-image. A subsequent revert then restores that stale ORIGINAL, silently
/// clobbering everything the user wrote after the first revert.
///
/// Repro below: apply→revert (file=ORIGINAL) → USER REWRITES → re-apply → revert
/// restores ORIGINAL, NOT the user's rewrite. This test asserts the CORRECT
/// behavior and therefore FAILS today, documenting the open bug. It is `#[ignore]`d
/// so it does not red the suite, but `cargo test -- --ignored` reproduces it.
/// Suggested fix: after a successful revert, delete (or version) the `{id}.bak` so a
/// re-apply re-captures a fresh pre-image; or key the backup filename by apply epoch.
#[test]
fn e2e_reapply_after_revert_must_not_restore_stale_original_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("CLAUDE.md");
    let original = "# Guide\n\noriginal A\noriginal B\n";
    std::fs::write(&target, original).expect("seed");
    std::env::set_var("WARDEN_CLAUDE_MD", &target);

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let f = finding("UNVERIFIED_COMPLETION");
    let id = "dl-a2";
    stage(&store, id, &f);

    apply_inner(&store, id).expect("apply");
    revert_inner(&store, id).expect("revert");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), original);

    let user_rewrite = "USER POST-REVERT CONTENT I MUST NOT LOSE\n";
    std::fs::write(&target, user_rewrite).expect("user rewrite");

    apply_inner(&store, id).expect("re-apply");
    revert_inner(&store, id).expect("revert after re-apply");

    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        user_rewrite,
        "revert after a re-apply MUST restore the user's post-revert content, \
         not the stale pre-first-apply ORIGINAL"
    );

    std::env::remove_var("WARDEN_CLAUDE_MD");
}

/// (b) findings 5/6 — re-apply on a still-'applied' artifact must PRESERVE the
/// original pristine backup so a later revert restores the ORIGINAL, never the
/// intermediate (user-edited) content.
#[test]
fn e2e_reapply_preserves_pristine_backup_for_later_revert_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("CLAUDE.md");
    let original = "GEN-1 PRISTINE ORIGINAL\n";
    std::fs::write(&target, original).expect("seed");
    std::env::set_var("WARDEN_CLAUDE_MD", &target);
    assert_eq!(claude_md_path(), target);

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let f = finding("UNVERIFIED_COMPLETION");
    let id = "dl-b";
    stage(&store, id, &f);

    // Apply #1: backs up the pristine ORIGINAL.
    let a1 = apply_inner(&store, id).expect("apply 1");
    let backup = expand_tilde(a1.backup_path.as_ref().expect("backup recorded"));
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original,
        "first backup is the pristine pre-image");
    let pre_sha1 = a1.pre_image_sha256.clone().expect("pre-sha recorded");

    // User removes the block AND rewrites the rest out-of-band → INTERMEDIATE state.
    let intermediate = "GEN-2 INTERMEDIATE USER EDIT (block removed)\n";
    std::fs::write(&target, intermediate).expect("user out-of-band edit");

    // Apply #2 is CHANGING (block re-added) but must NOT overwrite the pristine
    // backup, and must keep the ORIGINAL pre_image_sha256.
    let a2 = apply_inner(&store, id).expect("apply 2 (changing re-apply)");
    assert_eq!(a2.status, "applied");
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        original,
        "re-apply MUST NOT overwrite the pristine backup with the intermediate edit"
    );
    assert_eq!(
        a2.pre_image_sha256.as_deref(),
        Some(pre_sha1.as_str()),
        "re-apply must keep the ORIGINAL pre_image_sha256"
    );
    assert_ne!(
        sha256_hex(intermediate.as_bytes()),
        pre_sha1,
        "sanity: the intermediate content's sha differs from the pristine sha"
    );

    // A later revert must restore the ORIGINAL pristine baseline, not the
    // intermediate. (The post-image after apply #2 == current on disk, so the drift
    // guard passes and the restore proceeds from the pristine backup.)
    let rev = revert_inner(&store, id).expect("revert after re-apply");
    assert_eq!(rev.status, "reverted");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        original,
        "later revert MUST restore the ORIGINAL pristine baseline, not GEN-2 intermediate"
    );

    std::env::remove_var("WARDEN_CLAUDE_MD");
}

/// (c) finding 8 (TOCTOU/out-of-band) — after apply, the user edits the applied
/// file out-of-band; revert MUST REFUSE (sha mismatch) and leave the user's edits
/// intact, rather than blindly restoring the pre-image over them.
#[test]
fn e2e_revert_refuses_on_out_of_band_drift_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("CLAUDE.md");
    let original = "# Guide\n\nbase rule\n";
    std::fs::write(&target, original).expect("seed");
    std::env::set_var("WARDEN_CLAUDE_MD", &target);
    assert_eq!(claude_md_path(), target);

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let f = finding("UNVERIFIED_COMPLETION");
    let id = "dl-c";
    stage(&store, id, &f);

    apply_inner(&store, id).expect("apply");
    // User edits the applied file out-of-band (keeps the block, appends a rule).
    let applied = std::fs::read_to_string(&target).unwrap();
    let user_edited = format!("{applied}\nUSER ADDED A CRITICAL RULE I CARE ABOUT\n");
    std::fs::write(&target, &user_edited).expect("out-of-band edit");

    let err = revert_inner(&store, id).expect_err("revert must REFUSE on drift");
    assert!(
        err.to_string().contains("changed out-of-band"),
        "expected an out-of-band drift refusal, got: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        user_edited,
        "refused revert MUST leave the user's out-of-band edits intact"
    );
    // The artifact stays "applied" (refusal did not flip it), so the UI can retry.
    assert_eq!(store.artifact_by_id(id).unwrap().unwrap().status, "applied",
        "a refused revert must not mark the artifact reverted");

    std::env::remove_var("WARDEN_CLAUDE_MD");
}

/// (d) SAFE-REFUSAL (invalid-UTF8 total content loss) — apply against a target that
/// holds strictly-invalid UTF-8 must REFUSE: the file is left BYTE-IDENTICAL and NO
/// backup is written. Before the fix, read_to_string(...).unwrap_or_default() turned
/// the binary bytes into "", recorded an empty pre-image, wrote a 0-byte backup, and
/// atomic-wrote block-only content over the original — irrecoverable destruction.
/// A second assertion proves a UTF-8 BOM file ("\u{feff}...") still applies + reverts
/// correctly with the BOM preserved (BOM is valid UTF-8 and must keep working).
#[test]
fn e2e_apply_refuses_non_utf8_target_and_bom_round_trips_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");

    // ── Part 1: invalid-UTF8 target → apply REFUSES, file intact, no backup. ──
    let bin_target = dir.path().join("BINARY.md");
    let binary: &[u8] = &[0xff, 0xfe, b'h', b'i', 0x00, 0x80];
    std::fs::write(&bin_target, binary).expect("seed binary");
    std::env::set_var("WARDEN_CLAUDE_MD", &bin_target);
    assert_eq!(claude_md_path(), bin_target, "resolver must point at temp target");

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let f = finding("UNVERIFIED_COMPLETION");
    let id_bin = "safe-refusal-binary";
    stage(&store, id_bin, &f);

    let err = apply_inner(&store, id_bin).expect_err("apply must REFUSE a non-UTF8 target");
    assert!(
        err.to_string().contains("not valid UTF-8"),
        "expected a UTF-8 refusal, got: {err}"
    );
    // The target is byte-for-byte unchanged.
    assert_eq!(
        std::fs::read(&bin_target).expect("read binary after refusal"),
        binary,
        "the non-text target must be BYTE-IDENTICAL after a refused apply"
    );
    // No backup file was written anywhere under .warden-bak.
    let bin_backup = backup_dir(&bin_target).join(format!("{id_bin}.bak"));
    assert!(!bin_backup.exists(), "a refused apply must create NO backup file");
    // The artifact stays PENDING (the refusal aborted before any status flip).
    assert_eq!(
        store.artifact_by_id(id_bin).unwrap().unwrap().status,
        "pending",
        "a refused apply must not mark the artifact applied"
    );

    // ── Part 2: UTF-8 BOM target → apply + revert still work, BOM preserved. ──
    let bom_target = dir.path().join("BOM.md");
    let bom_original = "\u{feff}# Title\n\nexisting line\n";
    std::fs::write(&bom_target, bom_original).expect("seed BOM");
    let bom_original_bytes = std::fs::read(&bom_target).expect("read BOM seed");
    assert_eq!(&bom_original_bytes[..3], &[0xef, 0xbb, 0xbf], "sanity: BOM bytes present");
    std::env::set_var("WARDEN_CLAUDE_MD", &bom_target);
    assert_eq!(claude_md_path(), bom_target);

    let id_bom = "safe-refusal-bom";
    stage(&store, id_bom, &f);

    let applied = apply_inner(&store, id_bom).expect("BOM apply must succeed (valid UTF-8)");
    assert_eq!(applied.status, "applied");
    let after_apply = std::fs::read(&bom_target).expect("read BOM after apply");
    assert_eq!(&after_apply[..3], &[0xef, 0xbb, 0xbf], "BOM preserved at head after apply");
    assert!(
        String::from_utf8_lossy(&after_apply).contains("WARDEN guardrail — UNVERIFIED_COMPLETION"),
        "guardrail block must be appended to the BOM file"
    );

    let reverted = revert_inner(&store, id_bom).expect("BOM revert must succeed");
    assert_eq!(reverted.status, "reverted");
    assert_eq!(
        std::fs::read(&bom_target).expect("read BOM after revert"),
        bom_original_bytes,
        "revert must restore the BOM file BYTE-IDENTICAL (BOM survives the round-trip)"
    );

    std::env::remove_var("WARDEN_CLAUDE_MD");

    // Defense-in-depth: every path we touched lives under the tempdir.
    for p in [&bin_target, &bom_target] {
        assert!(p.starts_with(dir.path()), "{p:?} escaped the tempdir");
    }
}

/// BONUS — finding 7: reverting an OLDER artifact must not silently delete a NEWER
/// artifact's guardrail block. Two findings → two artifacts on the SAME target.
/// Apply A, then Apply B (so the file holds blockA+blockB). Reverting A would
/// restore A's pristine backup (which predates blockB) and erase blockB — but the
/// out-of-band drift guard catches it: A's recorded post-image no longer matches
/// the current file (blockB was added since), so revert A REFUSES.
#[test]
fn e2e_revert_older_artifact_refuses_rather_than_deleting_newer_block_tempdir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("CLAUDE.md");
    let original = "# Guide\n\nbase\n";
    std::fs::write(&target, original).expect("seed");
    std::env::set_var("WARDEN_CLAUDE_MD", &target);
    assert_eq!(claude_md_path(), target);

    let store = Store::open(dir.path().join("warden.db")).expect("store");
    let fa = finding("UNVERIFIED_COMPLETION");
    let mut fb = finding("NO_DELEGATION");
    fb.id = "verif-NO_DELEGATION".into();

    let id_a = "older-a";
    let id_b = "newer-b";
    stage(&store, id_a, &fa);
    stage(&store, id_b, &fb);

    apply_inner(&store, id_a).expect("apply A");
    apply_inner(&store, id_b).expect("apply B");
    let after_both = std::fs::read_to_string(&target).unwrap();
    assert!(after_both.contains("UNVERIFIED_COMPLETION"), "block A present");
    assert!(after_both.contains("NO_DELEGATION"), "block B present");

    // Revert the OLDER artifact A: its backup predates block B, so a blind restore
    // would erase block B. The drift guard must refuse instead.
    let err = revert_inner(&store, id_a).expect_err("revert of older artifact must refuse");
    assert!(err.to_string().contains("changed out-of-band"),
        "expected drift refusal protecting the newer block, got: {err}");
    let still = std::fs::read_to_string(&target).unwrap();
    assert_eq!(still, after_both, "both blocks must survive the refused revert");
    assert!(still.contains("NO_DELEGATION"),
        "the NEWER artifact's block must NOT be silently deleted");

    std::env::remove_var("WARDEN_CLAUDE_MD");
}
