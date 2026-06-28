// types.ts — DOSSIER ("Profile by Proof") frontend contract.
//
// These interfaces mirror the EXACT JSON shapes emitted by the Rust backend
// commands (`get_profile`, `build_profile`, `get_efficiency_score`,
// `get_activity_heatmap`). Keys are snake_case because they come straight off
// serde — do NOT camelCase them; the wire is the source of truth. This is the
// honest seam: every field here traces to a real column/computation in BRAIN +
// RADAR, never a UI invention.

/** The five time-window presets. EXACT strings the backend matches on. */
export type ProfileWindow = 'all-time' | '6mo' | '3mo' | '30d' | '2wk';

export const PROFILE_WINDOWS: ProfileWindow[] = ['all-time', '6mo', '3mo', '30d', '2wk'];

/** Human label for each window (display only). */
export const WINDOW_LABELS: Record<ProfileWindow, string> = {
  'all-time': 'All time',
  '6mo': '6 months',
  '3mo': '3 months',
  '30d': '30 days',
  '2wk': '2 weeks',
};

/** A pointer back to the raw evidence a claim/leak rests on. */
export interface EvidenceRef {
  session_id: string;
  turn_id: string | null;
  event_id: string | null;
  quote: string | null;
  source_path: string | null;
}

/** One asserted/emerging statement inside a dimension, with its proof. */
export interface Claim {
  text: string;
  confidence: number;
  status: 'asserted' | 'emerging';
  evidence: EvidenceRef[];
}

/** One of the 7 profile dimensions (orchestration, patterns, holes, …). */
export interface ProfileDimension {
  key: string;
  title: string;
  narrative: string;
  claims: Claim[];
}

/** A ranked, cost-estimated workflow leak. */
export interface Leak {
  rank: number;
  title: string;
  est_cost_tokens: number;
  est_cost_minutes: number;
  evidence: EvidenceRef[];
}

/** A cluster of projects sharing an archetype (greenfield, debugging, …). */
export interface ProjectArchetype {
  archetype: string;
  projects: string[];
  session_count: number;
  note: string;
}

/** A single point on a trait's trajectory. */
export interface TraitPoint {
  bucket: string;
  value: number;
}

/** How one trait is trending across time buckets. */
export interface TraitTrend {
  trait_key: string;
  direction: 'improving' | 'regressing' | 'plateaued' | 'insufficient';
  points: TraitPoint[];
  confidence: number;
}

/** One sub-family of the efficiency rubric (7 total). */
export interface EfficiencyFamily {
  key: string;
  sub_score: number;
  weight: number;
}

/** The composite efficiency score + its rubric breakdown. */
export interface EfficiencyScore {
  headline: number; // 0..1
  rubric_version: string;
  families: EfficiencyFamily[];
  session_count: number;
}

/** Per-harness slice of a day's activity. */
export interface HarnessActivity {
  harness: string;
  tokens: number;
  sessions: number;
}

/** One calendar day in the activity heatmap. */
export interface ActivityCell {
  date: string; // YYYY-MM-DD
  total_tokens: number;
  session_count: number;
  by_harness: HarnessActivity[];
}

/** The full longitudinal profile (the LLM-synthesized — or detector-only — readout). */
export interface Profile {
  window: string;
  generated_at: string;
  data_hash: string;
  rubric_version: string;
  efficiency: EfficiencyScore;
  dimensions: ProfileDimension[];
  ranked_leaks: Leak[];
  archetypes: ProjectArchetype[];
  trajectory: TraitTrend[];
  session_count: number;
  detector_only: boolean;
}

/** Progress event payload (`dossier_progress`) while `build_profile` runs. */
export interface DossierProgress {
  stage: string;
  status: string;
  [k: string]: unknown;
}
