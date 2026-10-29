import type { RadarAgent } from '@/viz/shared/types/radarTypes';

/** The name a builder goes by: its team name, then the names the radar shows. */
export function nameOfAgent(a: RadarAgent): string {
  return a.team?.memberName || a.nickname || a.title || a.label || a.id.slice(0, 8);
}
