//! Model pricing and context window ground truth: a single, hand maintained data
//! table (verified against vendor pricing pages) plus the lookup used by
//! [`super::context::est_cost_usd`] and [`super::composition::max_window_for_model`].
//!
//! Lookup precedence, both for price and for context window:
//! 1. Exact model id, after stripping a trailing `[1m]` or `-1m` transcript tag
//!    (Claude Code's long context beta marker; a no-op for a model that is
//!    already 1M context by default, e.g. `claude-opus-4-8[1m]`).
//! 2. Family prefix or bare alias (`opus`, `sonnet`, `haiku`, `fable`, `mythos`,
//!    plus the narrower `sonnet-4` sub-family) resolves to that family's current
//!    flagship row, marked `approximate` for price.
//! 3. Price only: a recognized vendor prefix (`claude`, or `gpt`/`codex`/`openai`)
//!    with no family match at all falls back to that vendor's default tier, also
//!    marked `approximate`, so a model with a recognizable vendor never silently
//!    shows no cost line at all.
//! 4. Context window has NO vendor level fallback (step 3 does not apply to it):
//!    an unrecognized but claude shaped id stops at `0`, not a guessed 200K. A
//!    confidently wrong `fill_pct` is worse than an honest unknown, which is the
//!    actual defect being fixed here (the old code bucketed anything containing
//!    "claude" that was not "opus" into 200K, which is how `claude-fable-5`, a
//!    1,000,000 context model, ended up rendering a wrong 200K window).
//! 5. No recognizable vendor signature at all: `None` for price, `0` for window,
//!    honestly unknown, never fabricated.
//!
//! Cache pricing is derived, not tabulated: cache read tokens bill at
//! [`CACHE_READ_MULTIPLIER`] and cache write tokens (5 minute TTL, the default)
//! at `CACHE_WRITE_MULTIPLIER_5M` of the resolved input rate. Both are confirmed
//! identical across every current Anthropic and OpenAI row against the vendors'
//! own pricing pages (fetched 2026-07-27), so they are constants rather than a
//! per-row column.
//!
//! `claude-sonnet-5`'s row lists its INTRO price, in effect through 2026-08-31;
//! standard pricing ($3.00 / $15.00) starts 2026-09-01 and needs a manual bump to
//! this table then. There is no time-based pricing logic anywhere else in WARDEN
//! and the frozen `est_cost_usd(model, exact)` call site has no clock to hand
//! this lookup, so a hand maintained snapshot (true as of the last fetch, same as
//! every other row here) is the simpler, more honest choice over adding
//! date-aware machinery for one row's scheduled price change.

/// Dollars per 1,000,000 tokens for one priced model tier, plus its context
/// window in tokens. `context_window` is `0` when the window is genuinely
/// unconfirmed (kept distinct from an unpriced row: `fill_pct` degrades to `0.0`
/// gracefully, the same as a fully unknown model, while the cost estimate stays
/// real).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub context_window: u64,
}

/// Cache read ("cache hit") tokens bill at this fraction of the resolved input
/// rate. Verified identical for every current Claude and GPT-5.6-family row
/// against the vendors' own pricing pages (2026-07-27 fetch): a clean, universal
/// constant, not a per-row column.
pub(crate) const CACHE_READ_MULTIPLIER: f64 = 0.1;

// Cache WRITE tokens (`cache_creation`) bill at 1.25x the resolved input rate
// under the 5 minute TTL, the default, and 2.0x under the 1 hour TTL. Verified
// against every current Anthropic and GPT-5.6-family row on the vendors' own
// pricing pages (2026-07-27 fetch). `est_cost_usd` cannot apply this yet:
// `ExactComposition::fresh` merges genuinely new input with `cache_creation`
// before it reaches this module (see the note on `est_cost_usd`), so there is
// no live production call site for these two multipliers today; WARDEN's
// transcript data also carries no per-event TTL signal to pick 5-minute vs
// 1-hour even if there were. They are exercised by this module's own test
// (`cache_read_and_cache_write_multipliers_match_the_verified_constants`,
// where they are declared locally) so the formula is proven correct and ready
// for the day the `cache_creation` split is threaded through from `agent.rs`.

/// The GPT-5.6/5.5/5.4 family reprices the WHOLE request (every token, not just
/// the tokens past the threshold) once resident context exceeds this many
/// tokens. A materially different mechanic from Anthropic's flat rate, whole
/// window pricing; see `reprice_long_context`.
const OPENAI_LONG_CONTEXT_THRESHOLD: u64 = 272_000;

/// Id prefixes subject to the long context repricing above. `gpt-4o` and
/// `gpt-3.5-turbo` are legacy, sub-272K-window models and are deliberately not
/// part of this list.
const OPENAI_LONG_CONTEXT_FAMILIES: &[&str] = &["gpt-5.6", "gpt-5.5", "gpt-5.4"];

/// Exact id price and window table. One row per model id actually seen in local
/// transcripts or documented live, source commented per row (fetched 2026-07-27
/// unless noted otherwise). Matched FIRST, before any family or vendor fallback.
const EXACT_TABLE: &[(&str, ModelPricing)] = &[
    // Anthropic, current generation, 1,000,000 context by default.
    // platform.claude.com/docs/en/about-claude/pricing (fetched 2026-07-27).
    (
        "claude-fable-5",
        ModelPricing { input: 10.0, output: 50.0, context_window: 1_000_000 },
    ),
    (
        "claude-mythos-5",
        ModelPricing { input: 10.0, output: 50.0, context_window: 1_000_000 },
    ),
    (
        "claude-opus-5",
        ModelPricing { input: 5.0, output: 25.0, context_window: 1_000_000 },
    ),
    (
        "claude-opus-4-8",
        ModelPricing { input: 5.0, output: 25.0, context_window: 1_000_000 },
    ),
    (
        "claude-opus-4-7",
        ModelPricing { input: 5.0, output: 25.0, context_window: 1_000_000 },
    ),
    (
        "claude-opus-4-6",
        ModelPricing { input: 5.0, output: 25.0, context_window: 1_000_000 },
    ),
    (
        "claude-opus-4-5",
        ModelPricing { input: 5.0, output: 25.0, context_window: 1_000_000 },
    ),
    // Pre-1M-tier Opus (claude-opus-4-1 deprecated, retires 2026-08-05;
    // claude-opus-4-0 retired first-party except Bedrock/Google Cloud). Same
    // source page as above.
    (
        "claude-opus-4-1",
        ModelPricing { input: 15.0, output: 75.0, context_window: 200_000 },
    ),
    (
        "claude-opus-4-0",
        ModelPricing { input: 15.0, output: 75.0, context_window: 200_000 },
    ),
    // Sonnet 5 intro pricing, in effect through 2026-08-31 (see the module doc);
    // standard pricing from 2026-09-01 is $3.00 / $15.00, same source page.
    (
        "claude-sonnet-5",
        ModelPricing { input: 2.0, output: 10.0, context_window: 1_000_000 },
    ),
    (
        "claude-sonnet-4-6",
        ModelPricing { input: 3.0, output: 15.0, context_window: 1_000_000 },
    ),
    // Sonnet 4.5 is the last generation where 1M context is a separate beta
    // header tier rather than the default; 200K here is the STANDARD tier.
    (
        "claude-sonnet-4-5",
        ModelPricing { input: 3.0, output: 15.0, context_window: 200_000 },
    ),
    (
        "claude-sonnet-4-0",
        ModelPricing { input: 3.0, output: 15.0, context_window: 200_000 },
    ),
    // Live Haiku 4.5. platform.claude.com/docs/en/about-claude/pricing (derived
    // from the batch rate, $0.50 / $2.50 times 2).
    (
        "claude-haiku-4-5-20251001",
        ModelPricing { input: 1.0, output: 5.0, context_window: 200_000 },
    ),
    (
        "claude-haiku-4-5",
        ModelPricing { input: 1.0, output: 5.0, context_window: 200_000 },
    ),
    // Retired Haiku 3.5 (first-party retired 2026-02-19). This is the rate the
    // OLD table used for every "haiku" match, including live 4.5 traffic; kept
    // here ONLY for its own exact id now, not as the family default.
    (
        "claude-3-5-haiku-20241022",
        ModelPricing { input: 0.80, output: 4.0, context_window: 200_000 },
    ),
    // Fully retired, general knowledge figures (dropped off the live pricing
    // page entirely), not re-verified live this pass.
    (
        "claude-3-7-sonnet-20250219",
        ModelPricing { input: 3.0, output: 15.0, context_window: 200_000 },
    ),
    (
        "claude-3-5-sonnet-20241022",
        ModelPricing { input: 3.0, output: 15.0, context_window: 200_000 },
    ),
    (
        "claude-3-5-sonnet-20240620",
        ModelPricing { input: 3.0, output: 15.0, context_window: 200_000 },
    ),
    (
        "claude-3-opus-20240229",
        ModelPricing { input: 15.0, output: 75.0, context_window: 200_000 },
    ),
    (
        "claude-3-haiku-20240307",
        ModelPricing { input: 0.25, output: 1.25, context_window: 200_000 },
    ),
    // OpenAI, short context tier (resident context at or below the 272K long
    // context threshold). platform.openai.com/docs/pricing (fetched 2026-07-27).
    (
        "gpt-5.6-sol",
        ModelPricing { input: 5.0, output: 30.0, context_window: 1_050_000 },
    ),
    // Approx window: GA'd alongside Sol/Luna at 1M class context per the
    // 2026-07-09 announcement, not re-confirmed on the live page's window column.
    (
        "gpt-5.6-terra",
        ModelPricing { input: 2.5, output: 15.0, context_window: 1_050_000 },
    ),
    (
        "gpt-5.6-luna",
        ModelPricing { input: 1.0, output: 6.0, context_window: 1_050_000 },
    ),
    // Approx window: successor line to GPT-5, treated as 1M class; verify this
    // figure if it becomes load-bearing.
    (
        "gpt-5.5",
        ModelPricing { input: 5.0, output: 30.0, context_window: 1_050_000 },
    ),
    // Window not confirmed this pass; priced anyway (fill_pct degrades to 0, not
    // a guessed number, same honesty policy as a fully unrecognized id).
    (
        "gpt-5.5-pro",
        ModelPricing { input: 30.0, output: 180.0, context_window: 0 },
    ),
    (
        "gpt-5.4",
        ModelPricing { input: 2.5, output: 15.0, context_window: 0 },
    ),
    (
        "gpt-5.4-mini",
        ModelPricing { input: 0.75, output: 4.5, context_window: 0 },
    ),
    (
        "gpt-5.4-nano",
        ModelPricing { input: 0.20, output: 1.25, context_window: 0 },
    ),
    // Legacy, stable, widely documented specs (low priority: 1 and 3 local hits).
    (
        "gpt-4o",
        ModelPricing { input: 2.5, output: 10.0, context_window: 128_000 },
    ),
    (
        "gpt-3.5-turbo",
        ModelPricing { input: 0.5, output: 1.0, context_window: 16_000 },
    ),
];

/// Family prefix fallback: an id that is not an exact table hit but clearly names
/// one of these families (substring match, checked in order, first match wins)
/// resolves to that family's row. Covers a bare alias (`"opus"`, 141 local hits)
/// and an unrecognized specific id in a known family alike. `sonnet-4` is checked
/// before the broader `sonnet` so an unrecognized 4.x-shaped id (e.g. a dated
/// snapshot) resolves to Sonnet 4.5's tier, not Sonnet 5's; a bare `sonnet` still
/// resolves to the current (5) flagship, matching the doc's own reading of the
/// bare alias ("ambiguous, likely current default Sonnet").
const FAMILY_FALLBACK: &[(&str, &str)] = &[
    ("fable", "claude-fable-5"),
    ("mythos", "claude-mythos-5"),
    ("opus", "claude-opus-5"),
    ("sonnet-4", "claude-sonnet-4-5"),
    ("sonnet", "claude-sonnet-5"),
    ("haiku", "claude-haiku-4-5"),
];

fn exact_row(id: &str) -> Option<ModelPricing> {
    EXACT_TABLE.iter().find(|(k, _)| *k == id).map(|(_, p)| *p)
}

fn family_row(id: &str) -> Option<ModelPricing> {
    FAMILY_FALLBACK
        .iter()
        .find(|(needle, _)| id.contains(needle))
        .map(|(_, key)| table_row(key))
}

/// Look up a hardcoded fallback key in `EXACT_TABLE`. A miss is a programmer
/// error (a `FAMILY_FALLBACK`/vendor default key that does not match one of its
/// own rows), a true invariant, hence `.expect`, not a `Result`.
fn table_row(key: &str) -> ModelPricing {
    exact_row(key).expect("internal fallback key must exist in EXACT_TABLE")
}

/// Strip a trailing `[1m]` beta tag or `-1m` suffix (Claude Code's long context
/// flag marker) before a table lookup. A no-op for any id that does not end in
/// one of these two exact forms.
fn strip_context_tag(id: &str) -> &str {
    id.strip_suffix("[1m]").or_else(|| id.strip_suffix("-1m")).unwrap_or(id)
}

fn is_openai_long_context_family(id: &str) -> bool {
    OPENAI_LONG_CONTEXT_FAMILIES.iter().any(|f| id.starts_with(f))
}

/// Reprice a short context OpenAI rate for the long context tier: input doubles,
/// output rises 1.5x, a FULL REQUEST repricing (every token in the request, not
/// just the tokens past the threshold). Verified 2x / 1.5x against every GPT-5.6
/// row on the live pricing page: Sol $5 to $10 input, $30 to $45 output, and so
/// on for Terra and Luna.
fn reprice_long_context(base: ModelPricing) -> ModelPricing {
    ModelPricing {
        input: base.input * 2.0,
        output: base.output * 1.5,
        context_window: base.context_window,
    }
}

/// One resolved price: the (possibly long context repriced) per token rates, and
/// whether this was an exact hit or a same tier fallback approximation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PriceLookup {
    pub pricing: ModelPricing,
    pub approximate: bool,
}

/// Resolve a model id to its priced tier. `resident_tokens` is the turn's total
/// resident context (`cache_read + fresh`), the trigger for the OpenAI long
/// context whole request repricing (otherwise identical to the short context
/// tier). `None` only when the id carries no recognizable vendor signature at
/// all (`claude`, `gpt`, `codex`, `openai`): a true unknown, never fabricated.
pub(crate) fn price_for_model(model: &str, resident_tokens: u64) -> Option<PriceLookup> {
    let lower = model.to_ascii_lowercase();
    let cleaned = strip_context_tag(&lower);
    let long_context =
        is_openai_long_context_family(cleaned) && resident_tokens > OPENAI_LONG_CONTEXT_THRESHOLD;
    let finish = |pricing: ModelPricing, approximate: bool| {
        let pricing = if long_context { reprice_long_context(pricing) } else { pricing };
        Some(PriceLookup { pricing, approximate })
    };

    if let Some(p) = exact_row(cleaned) {
        return finish(p, false);
    }
    if let Some(p) = family_row(cleaned) {
        return finish(p, true);
    }
    // Vendor prefix fallback, PRICE ONLY (see the module doc): a recognized
    // vendor with no family match at all still gets a directionally useful
    // estimate rather than silently vanishing. Anthropic defaults to the Sonnet
    // tier (its own "balanced default" positioning); OpenAI to Terra, the middle
    // of the Sol/Terra/Luna trio. Both are the least precise tier this module
    // returns and are always marked `approximate`.
    if cleaned.starts_with("claude") {
        return finish(table_row("claude-sonnet-5"), true);
    }
    if cleaned.starts_with("gpt")
        || cleaned.contains("codex")
        || cleaned.contains("openai")
        || cleaned.contains("o200k")
    {
        return finish(table_row("gpt-5.6-terra"), true);
    }
    None
}

/// Resolve a model id's context window in tokens. `0` when genuinely
/// unrecognized. Unlike price, this has NO vendor level fallback tier: an
/// unrecognized but claude/gpt shaped id still returns `0`, not a guessed number
/// (see the module doc, point 4: this is the actual fix for the old "contains
/// claude" bug).
pub(crate) fn context_window_for_model(model: &str) -> u64 {
    let lower = model.to_ascii_lowercase();
    // Explicit 1M context Sonnet tag: only the two real advertised forms match (a
    // bare "1m" substring that is not one of them, e.g. a stray date fragment,
    // must NOT).
    if lower.contains("sonnet") && (lower.contains("-1m") || lower.contains("[1m]")) {
        return 1_000_000;
    }
    let cleaned = strip_context_tag(&lower);

    if let Some(p) = exact_row(cleaned) {
        return p.context_window;
    }
    if let Some(p) = family_row(cleaned) {
        return p.context_window;
    }
    // Legacy Codex provider fallback: a bare provider or harness token with no
    // specific model id at all. This is the on-disk `model_context_window` Codex
    // itself reports for its wrapped GPT-5-class sessions today, kept as its own
    // constant, distinct from the API level GPT-5.6 table windows above, since
    // Codex CLI reserves a narrower slice of the underlying model's true window.
    if cleaned.contains("codex") || cleaned.contains("openai") || cleaned.contains("o200k") {
        return 258_400;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(id: &str) -> ModelPricing {
        price_for_model(id, 0).expect("recognized id").pricing
    }

    /// The full table, spot-checked family by family against the verified doc:
    /// every listed id resolves as an EXACT hit (not a fallback approximation),
    /// with the right price and the right context window.
    #[test]
    fn every_family_representative_matches_the_verified_doc_table() {
        let cases: [(&str, f64, f64, u64); 15] = [
            ("claude-fable-5", 10.0, 50.0, 1_000_000),
            ("claude-mythos-5", 10.0, 50.0, 1_000_000),
            ("claude-opus-5", 5.0, 25.0, 1_000_000),
            ("claude-opus-4-8", 5.0, 25.0, 1_000_000),
            ("claude-sonnet-5", 2.0, 10.0, 1_000_000),
            ("claude-sonnet-4-6", 3.0, 15.0, 1_000_000),
            ("claude-sonnet-4-5", 3.0, 15.0, 200_000),
            ("claude-haiku-4-5-20251001", 1.0, 5.0, 200_000),
            ("claude-3-5-haiku-20241022", 0.80, 4.0, 200_000),
            ("gpt-5.6-sol", 5.0, 30.0, 1_050_000),
            ("gpt-5.6-terra", 2.5, 15.0, 1_050_000),
            ("gpt-5.6-luna", 1.0, 6.0, 1_050_000),
            ("gpt-5.5", 5.0, 30.0, 1_050_000),
            ("gpt-4o", 2.5, 10.0, 128_000),
            ("gpt-3.5-turbo", 0.5, 1.0, 16_000),
        ];
        for (id, input, output, window) in cases {
            let lookup = price_for_model(id, 0).expect("must be a recognized exact hit");
            assert!((lookup.pricing.input - input).abs() < 1e-9, "{id} input rate");
            assert!((lookup.pricing.output - output).abs() < 1e-9, "{id} output rate");
            assert_eq!(context_window_for_model(id), window, "{id} context window");
            assert!(!lookup.approximate, "{id} is an exact table hit, not a fallback");
        }
    }

    /// Defect: claude-fable-5 (5,618 local occurrences) used to return NO cost
    /// estimate at all ("fable" matched none of the four old branches) and the
    /// wrong 200K window (it contains "claude"). Both are wrong: Fable 5 is
    /// priced and is a 1,000,000 context model.
    #[test]
    fn fable_5_is_priced_and_gets_its_own_1m_window() {
        let p = price_for_model("claude-fable-5", 0).expect("fable must be priced");
        assert!((p.pricing.input - 10.0).abs() < 1e-9);
        assert!((p.pricing.output - 50.0).abs() < 1e-9);
        assert_eq!(
            context_window_for_model("claude-fable-5"),
            1_000_000,
            "must not fall into the generic 200K 'contains claude' bucket"
        );
    }

    /// Defect: bare claude-sonnet-5 (8,203 local occurrences) and
    /// claude-sonnet-4-6 (1,213) are 1,000,000 context by default now, no "-1m"
    /// or "[1m]" suffix required. Sonnet 4.5 is still 200K, the last generation
    /// where 1M is a separate beta tier.
    #[test]
    fn bare_sonnet_5_and_sonnet_4_6_default_to_1m_context() {
        assert_eq!(context_window_for_model("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-5"), 200_000);
    }

    /// Defect: "haiku" used to bill at the retired Haiku 3.5 rate everywhere.
    /// Live Haiku 4.5 traffic (the exact id, its dated form, and the bare alias)
    /// must bill at the live 4.5 rate; the explicit retired id keeps its own,
    /// lower, historical rate.
    #[test]
    fn haiku_bills_live_4_5_rate_not_the_retired_3_5_rate() {
        for id in ["haiku", "claude-haiku-4-5", "claude-haiku-4-5-20251001"] {
            let p = price(id);
            assert!((p.input - 1.0).abs() < 1e-9, "{id} should bill live Haiku 4.5 ($1.00)");
            assert!((p.output - 5.0).abs() < 1e-9, "{id} should bill live Haiku 4.5 ($5.00)");
        }
        let retired = price("claude-3-5-haiku-20241022");
        assert!((retired.input - 0.80).abs() < 1e-9, "the explicit retired id keeps its own rate");
        assert!((retired.output - 4.0).abs() < 1e-9);
    }

    /// Cache read is always 0.1x the resolved input rate; cache write (5 minute
    /// TTL, the default) is 1.25x, and the 1 hour TTL rate is 2.0x. Verified
    /// against Opus 5's row ($5 input gives $0.50 / $6.25 / $10.00). The write
    /// multipliers have no production call site yet (see the module doc), so
    /// they are declared locally here rather than as unused top-level consts.
    #[test]
    fn cache_read_and_cache_write_multipliers_match_the_verified_constants() {
        const CACHE_WRITE_MULTIPLIER_5M: f64 = 1.25;
        const CACHE_WRITE_MULTIPLIER_1H: f64 = 2.0;

        assert!((CACHE_READ_MULTIPLIER - 0.1).abs() < 1e-12);

        let opus = price("claude-opus-5");
        assert!((opus.input * CACHE_READ_MULTIPLIER - 0.50).abs() < 1e-9);
        assert!((opus.input * CACHE_WRITE_MULTIPLIER_5M - 6.25).abs() < 1e-9);
        assert!((opus.input * CACHE_WRITE_MULTIPLIER_1H - 10.0).abs() < 1e-9);
    }

    /// GPT-5.6 Sol reprices the WHOLE request once resident context exceeds
    /// 272,000 tokens, not just the tokens past the threshold: input doubles
    /// ($5 to $10), output rises 1.5x ($30 to $45). Exactly at the threshold is
    /// still the short context tier ("over 272K" is strictly greater than).
    #[test]
    fn gpt_5_6_sol_reprices_the_whole_request_over_272k() {
        let short = price_for_model("gpt-5.6-sol", 272_000).unwrap().pricing;
        assert!((short.input - 5.0).abs() < 1e-9, "at the threshold, still short context");
        assert!((short.output - 30.0).abs() < 1e-9);

        let long = price_for_model("gpt-5.6-sol", 272_001).unwrap().pricing;
        assert!((long.input - 10.0).abs() < 1e-9, "over the threshold, the whole request reprices");
        assert!((long.output - 45.0).abs() < 1e-9);

        assert_eq!(short.context_window, long.context_window, "the window itself does not change");
    }

    /// gpt-4o and gpt-3.5-turbo are legacy, sub-272K-window models: no long
    /// context tier applies even at an (unrealistic) huge resident count.
    #[test]
    fn legacy_gpt_models_do_not_reprice() {
        let p = price_for_model("gpt-4o", 500_000).unwrap().pricing;
        assert!((p.input - 2.5).abs() < 1e-9);
        assert!((p.output - 10.0).abs() < 1e-9);
    }

    /// Unknown-model policy, pricing side: a specific id in a KNOWN family that
    /// is not itself in the table (a hypothetical next Opus release) falls back
    /// to that family's current rate rather than vanishing, marked approximate.
    #[test]
    fn unrecognized_family_id_falls_back_to_the_nearest_same_tier_rate() {
        let lookup = price_for_model("claude-opus-4-9", 0).expect("recognized family");
        assert!((lookup.pricing.input - 5.0).abs() < 1e-9);
        assert!((lookup.pricing.output - 25.0).abs() < 1e-9);
        assert!(lookup.approximate, "a family fallback must be marked approximate");
    }

    /// Unknown-model policy, the deliberate asymmetry: a recognized VENDOR
    /// prefix with no family match at all still gets a fallback PRICE (never
    /// silently None for a recognizable vendor), but the context WINDOW stays
    /// the honest zero rather than a guessed number, since a wrong window is a
    /// confidently wrong `fill_pct`, worse than admitting the window is unknown.
    #[test]
    fn vendor_only_match_gets_a_fallback_price_but_an_honest_zero_window() {
        let lookup = price_for_model("claude-nova-9", 0).expect("recognized claude vendor prefix");
        assert!(lookup.approximate);
        assert_eq!(
            context_window_for_model("claude-nova-9"),
            0,
            "no family match means no window guess, even though the vendor is known"
        );
    }

    /// A truly unrecognized id (no claude/gpt/codex/openai signature at all) has
    /// no price and no window: honestly unknown, matching the existing
    /// `assemble.rs` expectation for "mystery".
    #[test]
    fn truly_unknown_id_has_no_price_and_zero_window() {
        assert_eq!(price_for_model("mystery", 0), None);
        assert_eq!(price_for_model("mystery-model", 0), None);
        assert_eq!(context_window_for_model("mystery-model"), 0);
    }

    /// `<synthetic>` is WARDEN's own internal placeholder marker (31 local
    /// occurrences), not a real model: it carries no vendor signature at all, so
    /// it naturally falls through to the true unknown path, no special case
    /// needed, same as any other unrecognized id.
    #[test]
    fn synthetic_placeholder_is_treated_as_unknown_not_fabricated() {
        assert_eq!(price_for_model("<synthetic>", 0), None);
        assert_eq!(context_window_for_model("<synthetic>"), 0);
    }

    /// claude-opus-4-8[1m] (112 local occurrences): the "[1m]" beta tag is a
    /// Claude Code transcript marker, not a separately priced or windowed
    /// variant, since Opus is already 1,000,000 context by default. It must
    /// resolve to the exact same row as bare claude-opus-4-8.
    #[test]
    fn opus_4_8_1m_tag_is_a_price_and_window_no_op() {
        let bare = price_for_model("claude-opus-4-8", 0).unwrap();
        let tagged = price_for_model("claude-opus-4-8[1m]", 0).unwrap();
        assert_eq!(bare, tagged);
        assert!(!tagged.approximate, "the tag normalizes to an exact table hit");
        assert_eq!(
            context_window_for_model("claude-opus-4-8[1m]"),
            context_window_for_model("claude-opus-4-8")
        );
    }

    /// The 1M context Sonnet window matches ONLY the two explicit advertised
    /// forms ("-1m" suffix, "[1m]" beta tag); a bare "1m" substring inside an
    /// unrelated fragment (e.g. a date like "21mar") must not widen the window.
    /// It resolves via the "sonnet-4" family fallback to Sonnet 4.5's 200K, not
    /// Sonnet 5's 1M, since it is a 4.x shaped id.
    #[test]
    fn sonnet_1m_tag_matches_only_explicit_forms() {
        assert_eq!(context_window_for_model("claude-sonnet-4-5-1m"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-5[1m]"), 1_000_000);
        assert_eq!(
            context_window_for_model("claude-sonnet-4-5-21mar"),
            200_000,
            "a stray '1m' inside '21mar' must not widen to 1M"
        );
    }

    /// The Codex provider id ("openai", from `session_meta.model_provider`) and
    /// the bare "codex" token keep the legacy, empirically confirmed 258,400
    /// window: Codex CLI reserves a narrower slice of the underlying model's true
    /// window than the raw API figures in this table.
    #[test]
    fn codex_and_openai_provider_tokens_keep_the_legacy_window() {
        assert_eq!(context_window_for_model("openai"), 258_400);
        assert_eq!(context_window_for_model("OpenAI"), 258_400, "case-insensitive");
        assert_eq!(context_window_for_model("gpt-5-codex"), 258_400);
    }

    /// Defect: real Codex transcript ids (gpt-5.6-sol, 166 local occurrences;
    /// gpt-5.5, 130) used to be unpriced or priced at the old flat GPT-5
    /// launch-day rate ($1.25 / $10.00 for everything). They must now resolve to
    /// their own, distinct, verified rates.
    #[test]
    fn gpt_5_6_and_gpt_5_5_get_real_rates_not_the_old_flat_guess() {
        let sol = price("gpt-5.6-sol");
        let terra = price("gpt-5.6-terra");
        assert!((sol.input - 1.25).abs() > 1e-9, "must not be the old flat GPT-5 rate");
        assert_ne!(sol, terra, "Sol and Terra are priced differently, not flattened to one constant");
    }
}
