// verdict.ts — the one-line generated verdict that captions THE SCORE hero.
//
// Honest-viz: this is NOT an LLM sentence. It's a deterministic reading of the
// efficiency rubric — it names the operator's single strongest and single
// weakest family, humanized, in plain language ("strongest at delegation, you
// lose most to context hygiene"). Pure + total: any degenerate input (no
// families, non-finite scores, a null score) collapses to a safe fallback string
// so the hero never renders "undefined" or "NaN".

import type { EfficiencyFamily, EfficiencyScore } from './types';

/** Turn a snake_case rubric key into readable words; pass human text through. */
export function familyLabel(key: string): string {
  const trimmed = (key ?? '').trim();
  if (!trimmed) return '';
  return trimmed.includes('_') ? trimmed.replace(/_/g, ' ') : trimmed;
}

/** The family with the highest and the lowest finite sub_score. */
export function strongestWeakest(
  families: EfficiencyFamily[],
): { strongest: EfficiencyFamily; weakest: EfficiencyFamily } | null {
  const finite = (families ?? []).filter((f) => f && Number.isFinite(f.sub_score));
  if (finite.length === 0) return null;
  let strongest = finite[0];
  let weakest = finite[0];
  for (const f of finite) {
    if (f.sub_score > strongest.sub_score) strongest = f;
    if (f.sub_score < weakest.sub_score) weakest = f;
  }
  return { strongest, weakest };
}

/**
 * A single readable verdict sentence for THE SCORE hero, e.g.
 * "Strongest at delegation — you lose most to context hygiene."
 * Falls back to a calm, honest placeholder when there isn't enough signal.
 */
export function verdictLine(score: EfficiencyScore | null): string {
  const families = score?.families ?? [];
  const ranked = strongestWeakest(families);
  if (!ranked) {
    return 'Not enough scored sessions yet to read your strengths and leaks.';
  }
  const strong = familyLabel(ranked.strongest.key) || 'your workflow';
  const weak = familyLabel(ranked.weakest.key) || 'your workflow';
  if (ranked.strongest.key === ranked.weakest.key) {
    return `Strongest at ${strong} — one family scored so far; more signal sharpens the read.`;
  }
  return `Strongest at ${strong} — you lose most to ${weak}.`;
}
