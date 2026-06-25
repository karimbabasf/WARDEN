# M4 — FORGE (Apply) · Design Spec

- **Status:** Design — ready for plan execution
- **Date:** 2026-06-25
- **Milestone:** M4 "Forge" (the APPLY feature)
- **Scope (LOCKED):** Forge only. DOSSIER is explicitly deferred — do NOT build it.
- **Predecessor:** M2 fix-preview (`forge.rs`, read-only unified diff). M4 turns preview into a safe, reversible write.

---

## 1. Problem & Goal

WARDEN diagnoses agentic-workflow anti-patterns (findings such as `NO_DELEGATION`,
`UNVERIFIED_COMPLETION`, `CONTEXT_BLOAT`, …). The remediation for a finding is a small,
durable Markdown **guardrail block** appended to the operator's **agent-config**
(`~/.claude/CLAUDE.md` by default) — NOT to the user's application source code.

Today WARDEN can only *preview* that edit (`forge::fix_preview` → a real unified diff via the
`similar` crate, `applied: false` hardwired). **FORGE adds APPLY:** the operator approves the
previewed guardrail, WARDEN writes it to the target file with a backup, records the action as a
reversible artifact, and offers **Revert** to restore the pre-image. The whole loop is rendered
as a cinematic, honest approval experience in the FACE.

**Goal:** confirmed finding → approvable diff → Apply (backed-up, idempotent write) → applied-state
badge → Revert (verified restore). No patch engine, no AI slop, no writes to the real user config
during development.

---

## 2. Key Insight — apply is NOT a patch engine

The diff produced by `forge.rs` is **display-only**. Apply does NOT replay a diff. The write is
recomputed deterministically from the same pure function the preview already uses:

```
apply(artifact):
  1. current  = read(target)                       # missing file → ""
  2. proposed = ensure_block(current, block)        # append-if-absent, IDEMPOTENT
  3. if proposed == current: status = "applied" (no-op), record, return   # re-entrant
  4. backup the current bytes (sibling file) + record backup_path + pre_image_sha256
  5. write(target, proposed)
  6. record status="applied", applied_at=now()
```

`ensure_block` is a case-sensitive substring check on `block.trim()` (already in `forge.rs`).
Because the block is appended only when absent, **re-applying is a guaranteed no-op** and
**re-previewing an applied fix yields an empty diff**. This idempotency is the safety spine: it
makes apply re-entrant, makes double-clicks harmless, and makes revert's correctness checkable.

---

## 3. Architecture & Data Flow

```
 ┌──────────────────────── FACE (src/viz) ────────────────────────┐
 │ DetailPanel (chrome.tsx)                                         │
 │   [FIX PREVIEW] ──invoke get_fix_preview/get_orb_fix_preview──┐  │
 │   <diff render: target-path header + green add-lines>         │  │
 │   [APPLY] ──invoke stage_artifact(findingId|issueId) → id ────┼─▶│ creates PENDING artifact row
 │           ──invoke apply_artifact(id) ───────────────────────┼─▶│ backup + write + status=applied
 │   ⤷ card flips → applied badge + [REVERT]                     │  │
 │   [REVERT] ──invoke revert_artifact(id) ─────────────────────┼─▶│ verify sha + restore + status=reverted
 │   ⤷ card animates back to candidate                          │  │
 └──────────────────────────────┼─────────────────────────────────┘
                                 │ invoke (web→Rust)  /  emit (Rust→web)
 ┌──────────────────────────────▼─────────────────────────────────┐
 │ commands.rs   stage_artifact · apply_artifact · revert_artifact │
 │               · list_artifacts · get_artifact                   │
 │ forge.rs      apply_block · revert_block (pure write/restore)    │
 │ store.rs      artifacts table CRUD (save/get/list/update_status) │
 │ util.rs       claude_md_path() (already wired) + backup_dir()    │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                       target file (WARDEN_CLAUDE_MD || ~/.claude/CLAUDE.md)
                       backup file  (<target>.warden-bak/<artifact_id>.bak)
```

**Why a staged PENDING artifact instead of applying straight from a finding id?**
Two reasons. (1) **Orb issues are not persisted findings.** `get_orb_fix_preview` builds a
`Finding` on the fly from the live orb scene — there is no `findings`-table row to key an artifact
on. A `stage_artifact` command resolves *either* a saved finding id *or* an orb issue id into a
concrete `{target_path, diff, block, pattern_id}` and persists it as one PENDING `artifacts` row,
returning a stable `artifact_id`. (2) **The artifact row is the unit of reversible history.** Apply
and revert operate on that row, so the recorded diff/backup/status are always self-consistent and
survive a restart.

---

## 4. Artifact Lifecycle

```
            stage_artifact                apply_artifact              revert_artifact
   (finding) ───────────▶  PENDING  ───────────────────▶  APPLIED  ───────────────────▶  REVERTED
                              │  diff, block, target         │  backup_path,                │  (re-stage to
                              │  recorded                    │  pre_image_sha256,           │   apply again)
                              │                              │  applied_at recorded         │
                              └── apply on already-present block ──▶ APPLIED (no-op, no backup needed)
```

- **PENDING** — staged from a fix-preview. Row holds `finding_id`, `kind`, `target_path`, `diff`,
  `status="pending"`; `applied_at`/`backup_path` null. Also stores the literal `block` and the
  `pre_image_sha256` columns added in M4 (see §6) so apply/revert never need to re-resolve the
  pattern.
- **APPLIED** — target written (or already contained the block → no-op). `applied_at` set;
  `backup_path` + `pre_image_sha256` set (null for the pure no-op path, where there is nothing to
  restore differently from current).
- **REVERTED** — target restored from the verified backup. Status flips; `backup_path` retained for
  the audit trail. A reverted artifact can be re-staged and applied again (idempotent).

**Re-entrancy guarantees**
- `apply` on an APPLIED artifact whose block is still present → no-op, returns Ok.
- `apply` on a target that already contains the block (applied out-of-band) → records APPLIED,
  writes nothing, no spurious backup.
- `revert` on a PENDING/REVERTED artifact → returns Ok no-op (nothing was applied) or a typed error
  if there is genuinely nothing to restore (decision: **no-op Ok** to keep the UI forgiving).

---

## 5. Safety Model (NON-NEGOTIABLE)

1. **Never touch the real user config in dev/test/verify.** Every test and every manual
   verification pins `WARDEN_CLAUDE_MD` to a temp file. The real `~/.claude/CLAUDE.md` is sacred.
   This is enforced in code by `claude_md_path()` honoring the env override, and in tests by writing
   only into `tempfile::tempdir()` paths.
2. **Backup before write, always.** When apply changes bytes, the current file content is copied to
   a sibling backup *before* the new content is written. The backup path + a SHA-256 of the
   pre-image are recorded in the artifact row.
3. **Verify-before-restore on revert.** Revert reads the backup, recomputes its SHA-256, and
   compares against the recorded `pre_image_sha256`. On mismatch (backup tampered/lost) revert
   **refuses** with a typed error rather than restoring a wrong file. Integrity over convenience.
4. **Idempotent + re-entrant.** `ensure_block` guarantees re-apply is a no-op; double-clicks and
   repeated invokes cannot corrupt the file or stack duplicate blocks.
5. **Atomic-ish write.** Write to a temp file in the same directory then `rename` over the target
   (rename is atomic on the same filesystem), so a crash mid-write never leaves a half-file.
6. **Parent-dir creation is guarded.** `ensure_parent` (util.rs) creates the `.claude` dir if the
   target's parent is missing — apply must never panic on a fresh machine.
7. **Backups live beside the target, namespaced by artifact id** so concurrent artifacts cannot
   clobber each other's pre-image: `<target_dir>/.warden-bak/<artifact_id>.bak`.

---

## 6. Store Schema — `artifacts`

The table already exists (schema-only shell). M4 adds **two columns** via the idempotent
`pragma_table_info` ALTER pattern already used for `sessions.parent_session_id`:

| column | type | role |
|---|---|---|
| `id` | TEXT PK | `stable_id([finding_id, kind, target_path, nanos])` |
| `finding_id` | TEXT | source finding/issue id (nullable) |
| `kind` | TEXT NOT NULL | `"claude_md_guardrail"` |
| `target_path` | TEXT NOT NULL | resolved absolute target |
| `diff` | TEXT NOT NULL | display diff captured at stage time |
| `status` | TEXT NOT NULL | `pending` → `applied` → `reverted` |
| `applied_at` | TEXT | RFC3339 when applied (nullable) |
| `backup_path` | TEXT | sibling backup path (nullable) |
| **`block`** *(NEW)* | TEXT | the literal guardrail block to ensure |
| **`pre_image_sha256`** *(NEW)* | TEXT | SHA-256 of the file content backed up (nullable) |

Storing `block` (not just the diff) means apply/revert never re-derive the pattern — the artifact
is the single source of truth for what to write and how to undo it.

**New Store methods** (mirror `save_findings`/`finding_by_id`/`all_findings`):
- `save_artifact(&self, a: &Artifact) -> Result<()>` — `INSERT OR REPLACE`.
- `artifact_by_id(&self, id: &str) -> Result<Option<Artifact>>` — `query_row` + `.optional()`.
- `artifacts_for_finding(&self, finding_id: &str) -> Result<Vec<Artifact>>` — `query_map`.
- `all_artifacts(&self) -> Result<Vec<Artifact>>` — `query_map`, `ORDER BY` applied/created desc.
- `update_artifact_status(&self, id, status, applied_at, backup_path, pre_image_sha256) -> Result<()>`.

An `Artifact` struct lives in `ir.rs` (canonical IR home) so both store and forge share it.

---

## 7. IPC Contract

All web→Rust via `#[tauri::command]` + `invoke`; all return `Result<T, String>`. The two existing
stubs (`apply_artifact`, `revert_artifact`) keep their **exact `(id: String)` signatures** so the
roster in `lib.rs` is unchanged for them.

| command | Rust signature | request | response | behavior |
|---|---|---|---|---|
| `stage_artifact` | `async fn stage_artifact(state, finding_id: Option<String>, issue_id: Option<String>) -> Result<Artifact, String>` | `{ findingId?, issueId? }` | `Artifact` (status `pending`) | Resolve finding (saved) or orb issue (scene) → fix-preview → persist PENDING row → return it. Idempotent on `(finding_id,target)`: re-staging returns the existing non-applied row. |
| `apply_artifact` | `async fn apply_artifact(state, id: String) -> Result<Artifact, String>` | `{ id }` | `Artifact` (status `applied`) | Load row; `forge::apply_block(target, block)`; backup+write if changed; update status. Re-entrant no-op if already present. |
| `revert_artifact` | `async fn revert_artifact(state, id: String) -> Result<Artifact, String>` | `{ id }` | `Artifact` (status `reverted`) | Load row; verify `pre_image_sha256`; restore backup; status=reverted. No-op Ok if never applied. |
| `get_artifact` | `async fn get_artifact(state, id: String) -> Result<Option<Artifact>, String>` | `{ id }` | `Artifact?` | Single-row read for state reconciliation on panel open. |
| `list_artifacts` | `async fn list_artifacts(state, finding_id: Option<String>) -> Result<Vec<Artifact>, String>` | `{ findingId? }` | `Artifact[]` | History list; filtered by finding when provided. |

> **Compatibility note:** the existing stubs return `Result<(), String>`. M4 upgrades the return to
> `Result<Artifact, String>` so the FACE can flip the card from the returned status without a second
> round-trip. The arg (`id: String`) is unchanged; the roster entry name is unchanged. No frontend
> currently calls these (confirmed), so this is a safe widening.

No new Rust→web events are required for the happy path (apply/revert are request/response). Optional:
reuse the existing toast/error channel surfaced via the returned `Result::Err` string.

---

## 8. UI / UX Design (FACE)

**Surface:** `DetailPanel` in `src/viz/chrome.tsx` — the already-open right dock that renders the
fix diff. The approval UX is a natural extension *below* the diff. **Do not** put it on
`RadarDetailPanel` (liveness only) or the dead `diagnosis.ts` path.

**Cinematic state machine (every animation maps to a real state transition):**

1. **Provenance header slides in** — the resolved absolute `target_path` rendered prominently above
   the diff (`a/<path>` is in the diff, but the header makes it unmissable so the write is never a
   surprise). Owner decision: show the resolved path; default global, env-overridable.
2. **Diff reveal** — upgrade the flat `<pre>` to line-typed spans: `+` add-lines in phosphor green
   (`--green`/`--acid`), context lines dim (`--ink-faint`), hunk headers in `--dim`. Slide/clip-in
   panel-by-panel on the green-phosphor background.
3. **APPLY button** — acid CTA (mirrors `.wd-ask-run`), pulses **verdict-amber** (`--amber #ff5a37`)
   on hover to signal "this writes." Disabled + `cursor: progress` while the invoke is in flight.
   When the staged diff is empty (already present), Apply shows "ALREADY APPLIED" and is inert.
4. **Post-apply flip** — the card flips to a **locked applied-state badge** (lock glyph + `applied`
   + `applied_at`) and reveals a **REVERT** affordance. Drive `applied` from the returned
   `Artifact.status`, never from faked client state.
5. **REVERT** — uses the established destructive idiom (`rgba(255,90,55,0.12)` bg / `--amber` text /
   `rgba(255,90,55,0.5)` border). On success the card animates **back to candidate state** (badge
   collapses, Apply returns). Mirrors a real `reverted` status.

**Honesty rules:** never fabricate `applied`/diff data; the browser-QA fallback in
`WarRoom.onRequestFix` keeps its explicit "never writes in browser QA" copy. All colors come from
existing `style.css` tokens + `harnessTheme` (Claude emerald `#3dffa0`, Codex violet `#b98cff`).
Honor `prefers-reduced-motion` for the flip/slide.

---

## 9. Error Handling

- **Finding/issue not found** → `anyhow!()` → `Result::Err(String)` surfaced in `.wd-ask-error`.
- **Target unreadable / unwritable** → typed error string; no partial write (atomic rename).
- **Backup write fails** → abort apply *before* touching the target; nothing is written.
- **Revert sha mismatch / missing backup** → refuse with explicit "backup integrity check failed"
  error; do not restore.
- **Already-present block** → success no-op (empty diff), status `applied`, no backup.
- **Missing target file on apply** → `ensure_parent` + create; pre-image is empty string (sha of
  empty), backup is an empty file; revert restores to empty/absent semantics.
- File reads continue the existing `.unwrap_or_default()` convention for the *missing → empty* path;
  genuine IO errors propagate via `?`/`anyhow!`.

---

## 10. File Ownership (DISJOINT — parallel tracks)

| Track | Owns (writes) | May read |
|---|---|---|
| **BACKEND** | `src-tauri/src/forge.rs`, `src-tauri/src/store.rs`, `src-tauri/src/ir.rs`, `src-tauri/src/util.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs` (roster only) | frontend types for contract parity |
| **FRONTEND** | `src/viz/chrome.tsx`, `src/viz/WarRoom.tsx`, `src/style.css`, `src/viz/*.test.ts(x)` (new forge tests) | the IPC contract (this spec §7) |
| **INTEGRATION/VERIFY** | none new — runs builds/tests, may make a tiny roster/wiring fix if a seam is missed | both |

`lib.rs` is backend-owned but the only M4 edit there is adding `stage_artifact`, `get_artifact`,
`list_artifacts` to the handler roster — a 3-line, conflict-free change. Frontend never edits Rust.

---

## 11. Test Plan

**Backend (Rust, `cd src-tauri && cargo test`):**
- `forge::apply_block` writes the block to a temp target, returns changed=true, backup matches
  pre-image; second call is a no-op (changed=false), no duplicate block, no second backup.
- `forge::apply_block` on a missing target creates parent + file; pre-image sha = sha("").
- `forge::revert_block` restores the exact pre-image; idempotent re-preview is empty after revert.
- `forge::revert_block` refuses on sha mismatch (corrupt the backup → typed error, target untouched).
- `store` round-trips an `Artifact` through save → get → list → update_status (status transitions).
- Atomic write: target is never observed half-written (write-temp-then-rename).
- **Every test pins `WARDEN_CLAUDE_MD` to a tempdir path OR injects the target directly** (mirror
  the existing `preview_against` target-injection pattern to stay parallel-safe — no shared env-var
  race).

**Frontend (`pnpm build` + vitest):**
- `chrome.tsx` Apply button renders disabled while in flight; flips to applied badge + Revert when
  `Artifact.status === "applied"`; returns to candidate on `reverted`.
- Diff line-typing: `+` lines get the add class, headers get the header class.
- Browser-QA fallback path still shows the "never writes" copy and no Apply write occurs.

**Integration/Verify:**
- `cd src-tauri && cargo test` green; `pnpm build` green.
- Manual `pnpm tauri dev` smoke with `WARDEN_CLAUDE_MD=/tmp/warden-forge/CLAUDE.md`: stage → apply →
  inspect temp file has exactly one guardrail block → revert → temp file restored. **Never** point
  at the real config.

---

## 12. Open Risks

1. **Stub return-type widening** (`Result<()>`→`Result<Artifact>`) is safe today (no callers) but the
   roster and any future external caller must be checked at integration.
2. **Orb-issue artifacts are keyed on non-persisted ids** — staging must persist the resolved finding
   snapshot so a later orb refresh can't orphan the artifact.
3. **Backup directory hygiene** — `.warden-bak` accumulates; out of scope to GC in M4, but note it.
4. **Cross-filesystem rename** — atomic rename assumes backup/temp share the target's filesystem;
   `.warden-bak` beside the target guarantees this.
5. **Reduced-motion / a11y** for the cinematic flip must not gate the state change itself.
