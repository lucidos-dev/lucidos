/** Durations from clock arithmetic. No timezone and no preference, which is
 *  why these live apart from `formatTime.ts`: the store reads them, and that
 *  module would come into the entry chunk with them. */

/** How long something has been running: "8s", "2m 14s", "1h 03m".
 *
 *  A DURATION, not a point in time, so it takes milliseconds rather than a
 *  `Date` and never touches the user's timezone. Seconds stay in the minute
 *  range, where the value reads as a live counter. The status toast's build
 *  timer ticks once a second, and a counter that changed once a minute would
 *  read as frozen. Past an hour seconds are noise, so the hour form zero-pads
 *  minutes instead and the string stops changing every second.
 *
 *  A negative or non-finite input clamps to "0s": the caller derives this from
 *  clock arithmetic, and a "-3s" build age is worse than a momentarily stalled
 *  one. */
export function formatElapsed(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '0s';
  const totalSeconds = Math.floor(ms / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

/** Whole seconds between two readings of the browser clock, never negative.
 *
 *  For advancing a span the SERVER measured, without ever subtracting a server
 *  instant from a local one (ADR 0053). Both arguments are readings of the one
 *  local clock: when an answer landed, and now.
 *
 *  The clamp matters because that clock can move backwards, and a fault must
 *  not read as younger than the engine measured it. Used by the ingress and
 *  refusal selectors, which both age a standing fault forward while it stands. */
export function elapsedSeconds(since: number, now: number): number {
  return Math.max(0, Math.floor((now - since) / 1000));
}
