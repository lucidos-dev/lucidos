/** Main-thread stall detector: the longtask substitute WebKit leaves us.
 *
 *  `utils/perfProbe.ts` learns what blocked a frame from Long Animation Frames
 *  and from longtask. Safari has neither. On an iPhone the probe therefore has
 *  at most Event Timing, which speaks only for an interaction the user made.
 *
 *  A fixed tick measures what every engine will tell you. The tick is due at a
 *  known moment. The callback runs when the main thread is next free. The gap
 *  between the two IS the block. It names nothing, which is the honest limit
 *  here, and it does size the stall and timestamp it.
 *
 *  It runs ONLY while perf recording is on, driven by `onPerfEnabledChange`.
 *  With the flag off there is no interval at all, so default-off means doing
 *  nothing rather than merely recording nothing. Full reasoning in
 *  docs/plans/2026-09-20-the-phone-can-report-what-blocks-a-navigation.md.
 *
 *  Best-effort telemetry, under the carve-out in .claude/rules/frontend.md. */

import { onPerfEnabledChange, perfRecordingOn, recordPerfSample } from './perfQueue';

/** How often the tick is due. Short enough to catch a stall inside one
 *  navigation, long enough that the timer itself is not the cost: two clock
 *  reads and a compare, four times a second. */
const TICK_MS = 250;

/** Report at or above this overshoot.
 *
 *  Comfortably past the scheduling jitter a healthy browser shows, and past the
 *  50ms that defines a long task. A reported line therefore always means a
 *  stall the user could feel. */
const STALL_MS = 150;

/** How late did this tick run, and is that worth reporting?
 *
 *  Pure, so the decision is unit-tested without timers. `overshootMs` is the
 *  reading and `stalled` is whether it clears the bar.
 *
 *  A tick that runs EARLY reports zero rather than a negative, which a clock
 *  adjustment or a coarse timer can produce. A stall is a delay, and a negative
 *  delay is not a reading. */
export function stallOf(
  now: number,
  dueAt: number,
  threshold = STALL_MS,
): { overshootMs: number; stalled: boolean } {
  const overshootMs = Math.max(0, Math.round(now - dueAt));
  return { overshootMs, stalled: overshootMs >= threshold };
}

let timer: ReturnType<typeof setInterval> | null = null;
/** When the next tick is due, by the same clock the tick reads. */
let dueAt = 0;

/** The MONOTONIC clock, never `Date.now`.
 *
 *  Wall-clock time steps: an NTP correction or a manual change moves it, and
 *  this probe's whole reading is an elapsed-time subtraction. A forward step
 *  would read as a main thread blocked for exactly that long. */
function nowMs(): number {
  return performance.now();
}

/** Is the page hidden? A hidden one is not a page whose main thread is busy. */
function hidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden';
}

function tick(): void {
  const now = nowMs();
  // The gate can DROP with nothing announcing it: a console `removeItem`, or
  // another tab's write. Reading it converges the listeners, and an active tick
  // is the only place that reads it while the interval is the thing at stake.
  // Without this the timer outlives recording and runs for nothing, which is
  // the default-off invariant inverted.
  if (!perfRecordingOn()) { stop(); return; }
  // A BACKGROUNDED page has its timers throttled, and iOS suspends them
  // outright. The lateness is then the platform parking the tab, not a blocked
  // main thread, and an iOS PWA is backgrounded constantly. Reporting it would
  // bury every real stall under minutes-long fictions.
  if (hidden()) { dueAt = now + TICK_MS; return; }
  const { overshootMs, stalled } = stallOf(now, dueAt);
  // Re-anchor on NOW, not on the old deadline plus a tick. Anchoring on the
  // deadline would carry one stall's overshoot into the next reading, and
  // report the same block twice.
  dueAt = now + TICK_MS;
  if (stalled) recordPerfSample('main-thread-stall', { overshootMs });
}

/** Start ticking, unless already ticking. */
function start(): void {
  if (timer !== null || typeof setInterval === 'undefined') return;
  dueAt = nowMs() + TICK_MS;
  timer = setInterval(tick, TICK_MS);
}

/** Stop ticking, so the flag going off leaves nothing scheduled. */
function stop(): void {
  if (timer === null) return;
  clearInterval(timer);
  timer = null;
}

let installed = false;
/** Drops the gate subscription. Only a test ever calls it. */
let unsubscribeGate: (() => void) | null = null;

/** Follow the perf gate for the life of the page. Idempotent.
 *
 *  Called at startup beside `startPerfProbe`. It subscribes rather than polls,
 *  and starts at once when the flag was already set before this load. */
export function installMainThreadStallProbe(): void {
  if (installed) return;
  installed = true;
  unsubscribeGate = onPerfEnabledChange((on) => (on ? start() : stop()));
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', onVisible, { passive: true });
  }
  if (perfRecordingOn()) start();
}

/** Coming back to the foreground, which settles two things at once.
 *
 *  The deadline is RE-ANCHORED, so the first tick after a resume measures from
 *  the resume rather than reporting the whole background as one stall.
 *
 *  And the gate is READ, which converges it. That is what makes the module
 *  header's console path work: a developer sets the flag in devtools with the
 *  tab in the background, and coming back to the page starts the probe. */
function onVisible(): void {
  if (hidden()) return;
  dueAt = nowMs() + TICK_MS;
  if (perfRecordingOn()) start();
}

/** Test-only: tear the probe down so one test's interval cannot reach the next.
 *
 *  The UNSUBSCRIBE is the load-bearing half. Clearing `installed` alone leaves
 *  the previous test's listener on the gate, and the next test's toggle then
 *  starts a probe it never installed. That passed an assertion for the wrong
 *  reason until it was caught in review. */
export function _resetMainThreadStallForTesting(): void {
  stop();
  unsubscribeGate?.();
  unsubscribeGate = null;
  if (typeof document !== 'undefined') {
    document.removeEventListener('visibilitychange', onVisible);
  }
  installed = false;
  dueAt = 0;
}

/** Test-only: is an interval currently scheduled? The default-off invariant is
 *  about exactly this, and no other export can answer it. */
export function _stallProbeRunningForTesting(): boolean {
  return timer !== null;
}
