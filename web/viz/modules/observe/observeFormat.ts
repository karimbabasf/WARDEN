// observeFormat.ts: small pure formatters shared by the share menu, the peers panel and
// the observed-agent view. Kept local to this module (not shared/) since nothing outside
// remote observation needs them, and importing across `modules/radar` would violate the
// no-sibling-import rule.

/** Compact duration for a second count: "5s", "3m", "2h", "1d". Never negative, never NaN. */
export function humanizeSecs(totalSecs: number): string {
  const secs = Number.isFinite(totalSecs) ? Math.max(0, Math.round(totalSecs)) : 0;
  if (secs < 60) return `${secs}s`;
  const min = Math.floor(secs / 60);
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  return `${Math.floor(hr / 24)}d`;
}

/**
 * A rfc3339 timestamp relative to `now`: "5m ago" for the past, "in 5m" for the future.
 * Returns "" for an unparseable stamp so a bad payload renders no time rather than NaN.
 */
export function isoRelative(ts: string, now: number = Date.now()): string {
  const t = Date.parse(ts);
  if (!Number.isFinite(t)) return '';
  const diffSecs = Math.round((t - now) / 1000);
  if (diffSecs >= 0) return diffSecs < 5 ? 'now' : `in ${humanizeSecs(diffSecs)}`;
  return `${humanizeSecs(-diffSecs)} ago`;
}

/** True when an rfc3339 stamp is in the past relative to `now`. Unparseable is treated
 * as expired (the safest reading for a TTL check). */
export function isPast(ts: string, now: number = Date.now()): boolean {
  const t = Date.parse(ts);
  return !Number.isFinite(t) || t <= now;
}
