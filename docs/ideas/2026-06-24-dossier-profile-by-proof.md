# DOSSIER — "Profile by Proof"

> **Status:** idea capture only (2026-06-24). No spec, no plan, no code yet.
> Spec + plan to be written by Karim when ready to implement.
> **Milestone numbering is UNRESOLVED** — Karim referred to this as "M4," but the
> current roadmap already has **M4 = Forge (apply)**. Decide at spec time whether
> DOSSIER becomes the new M4 (and Forge slides), or takes its own slot. This doc
> is deliberately number-agnostic and calls it **DOSSIER**.

---

## 1. The idea (one paragraph)

WARDEN today produces a **point-in-time diagnosis**: "here are the holes in how you
ran your agents recently." DOSSIER promotes that into a **persistent, longitudinal,
evidence-backed profile of the operator** — a full read on *how this person drives
their agents over time*. Not a single verdict but a living file: how they orchestrate,
their signature patterns, their holes and mistakes, the things they do well, where they
bleed time/tokens/rework, what kinds of projects they tend to build, and whether they're
getting better or worse. It is powered mostly by **GLM-5.2 via NEAR AI** (the existing
brain), sitting on top of the **detectors/featurizer (BRAIN)** and the **orchestration
signals (RADAR)** we already built. A **time-window toggle** (all-time / 6mo / 3mo / 30d
/ 2wk) simply changes how recent the data we feed the pipeline has to be.

The name fits the WARDEN universe: a **dossier** is an evidence-backed file compiled on
a subject. Every claim in it is "by proof" — cited to real sessions/turns, never vibes.

---

## 2. What the profile contains (the dimensions)

1. **Orchestration style** — *powered by RADAR's historical signals.* Concurrency (how
   many agents in parallel), delegation/subagent rate, hierarchy depth, parallel-vs-serial
   working style, context-size discipline, harness mix (Claude vs Codex).
2. **Signature patterns / habits** — recurring behaviors, good and bad, that define how
   this operator works (not just failures).
3. **Holes & mistakes** — the existing detector taxonomy, but **aggregated and trended**
   across the window instead of per-session.
4. **Strengths** — what they consistently do well (verification discipline, tight prompts,
   good delegation, cache hygiene…). The profile must be balanced, not just a hit list.
5. **Where they lose** — token / time / rework leakage, **quantified and ranked**. "Your
   single biggest leak this quarter was X, costing ~N tokens / ~M minutes."
6. **Project archetypes** — *what they tend to develop* (web app, CLI, infra, data, viz,
   refactors…) and **how their behavior shifts per archetype** ("you delegate well on
   backend work but thrash on frontend").
7. **Trajectory** — per-trait trend lines. Improving? Regressing? Plateaued? This is the
   payoff of the time windows: you can see movement.

---

## 3. The time-window toggle

A single control: **all-time · 6mo · 3mo · 30d · 2wk**. It is *only* a recency filter on
which sessions feed the aggregation — the cutoff timestamp, nothing more.

- The store already timestamps every session/turn/event, so this is a `WHERE ts >= cutoff`.
- **Seam already exists:** `RunScope` in `ir.rs:293` carries `harness/query/force/max_files`.
  Add a `window` (enum) or `since: Option<DateTime>` field and thread it through the
  pipeline. Minimal new surface.
- A trait that is true all-time may be false in the last 2 weeks. The profile must present
  claims **per active window** and never say "you always" — it says "in the last 3 months."

---

## 4. Rough architecture (layers)

> Two non-negotiable design forces shape everything below: **cost** (6 months of transcript
> cannot be dumped into an LLM) and **honesty** ("by proof" — no confident claims from thin
> data). The architecture is built around both.

**Layer 1 — Scoped data access.** Extend `RunScope` with the time window; window → cutoff →
scoped session set. Everything downstream operates on that set.

**Layer 2 — Deterministic aggregation (cheap, local, does the heavy lifting).** Roll the
existing per-session `FeatureVector`s into:
- time-bucketed aggregates (weekly/monthly bins) → trend lines
- per-project rollups
- per-pattern frequency + cost + trend
- orchestration metrics sourced from RADAR history
The LLM **only ever sees these compact aggregates + sampled evidence**, never raw transcripts.
This is what keeps it affordable *and* grounded.

**Layer 3 — Hierarchical summarization (map-reduce) for the LLM.** You can't fit a window
in a context budget, so summarize in tiers:
- **per-session micro-summary** — computed once per session, **cached forever**, reused
  across every window and every future profile (marginal cost of a new session ≈ one small
  summary call).
- **→ per-week / per-project rollup** — fold micro-summaries.
- **→ window-level profile synthesis** — the one expensive GLM-5.2 call.
This tiering is the core trick that makes "all-time over 6 months" tractable. Cache each
tier by content hash; only *new* sessions ever get summarized.

**Layer 4 — Project clustering & archetype classification.** Group sessions by `project`,
then classify archetype. Semantic grouping/dedup of "the same mistake across many sessions"
wants **embeddings** (see §5).

**Layer 5 — Profile synthesis (GLM-5.2 / NEAR AI).** Consumes aggregates + rollup summaries +
evidence samples; emits the structured profile (per-dimension narrative + ranked leaks +
trends), each claim carrying its proof.

**Layer 6 — "By Proof" guard.** Every trait/claim must carry **≥ N evidence instances**
within the window or it is downgraded to "emerging / tentative," not asserted. Reuse the
existing `EvidenceRef` system. Trend claims additionally gated on a **minimum sample size
per bucket** (otherwise a 2-week trend is noise).

**Layer 7 — Persistence + cache.** A `profiles` table keyed by `(window, data_hash)`.
Toggling windows is **instant when cached**; recompute on demand or via a nightly scheduled
rollup. Detector-only / no-API degradation mode mirrors the existing diagnosis fallback.

---

## 5. Third-party / additional services worth considering

To hit the quality bar, a few capabilities go beyond what GLM-5.2 alone gives us:

- **Embeddings (recommended: on-device).** Needed for project/prompt/pattern clustering and
  semantic dedup ("is this the same hole recurring?"). Two paths:
  - **On-device** via `fastembed-rs` / ONNX (e.g. `bge-small`, `nomic-embed`) + the
    **`sqlite-vec`** extension for vector search in the existing SQLite store. **This is the
    recommendation** — it preserves WARDEN's "on-device by default, egress = brain only"
    principle and adds no new data-exfil surface.
  - **Cloud** (NEAR AI / GLM embeddings if exposed, else OpenAI / Voyage / Cohere) — faster
    to wire, but a new egress surface that conflicts with the privacy stance. Fallback, not default.
- **Clustering math:** pure Rust (`linfa` — k-means / DBSCAN) for archetype + behavior clusters.
  No service.
- **Trend / time-series math:** pure Rust. No service.
- **Heavier synthesis model (maybe):** if GLM-5.2 struggles with the longitudinal reasoning
  in Layer 5, the env-swappable brain means we can point the synthesis stage at a stronger
  model without re-plumbing. Start with GLM-5.2 (the chosen brain) and only escalate if quality demands it.

---

## 6. Quality risks (call them out now)

1. **Confident-but-wrong is the #1 risk.** Profiling a person longitudinally from sparse,
   noisy signals invites plausible fabrication. Mitigation = the by-proof N-threshold guard
   (Layer 6) + sample-size gating on trends.
2. **Cost.** A window can span thousands of sessions. Mitigation = deterministic aggregation
   does the heavy lifting, per-session summaries cached once and reused, only one expensive
   synthesis call per (window, data_hash).
3. **Context limits.** Never dump raw transcripts — hierarchical summarization only (Layer 3).
4. **Recency vs identity.** Present every claim scoped to its window; never universalize.
5. **Balance.** Must surface strengths, not only holes, or it reads as a punishment machine.

---

## 7. Where it plugs into the existing codebase

- `ir.rs` — add a time `window` to `RunScope`; add `Profile` / `ProfileDimension` types.
- `featurizer.rs` (or a new `profile.rs`) — longitudinal aggregation + time-bucketing + trends.
- `brain.rs` — add a `summarize_session` (cached) stage and a `synthesize_profile` stage;
  reuse the existing OpenAI-compatible NEAR AI client.
- `store.rs` — `session_summaries` cache table + `profiles` cache table; windowed queries.
- `commands.rs` — `get_profile(window)`, `build_profile(window)` (+ progress events);
  these are *new*, distinct from the M3–M7 stubs.
- **RADAR** — supplies the orchestration-dimension signals (concurrency, delegation, hierarchy).
- **FACE** — a new **Dossier** surface in the overlay with the window toggle. The war-room
  already has a Habits/Radar instrument switcher (per the FACE visual pass), so Dossier
  becomes a third instrument/tab.

---

## 8. Open decisions for spec time

- Milestone number (DOSSIER vs Forge collision on "M4").
- Embeddings: on-device (recommended) vs cloud.
- Whether profile build is on-demand, scheduled (nightly), or both.
- Profile output schema (how structured vs narrative).
- How DOSSIER relates to M4 Forge — does the profile *recommend* artifacts Forge then applies?
  (Natural synergy: the dossier identifies the durable holes; Forge fixes them.)
