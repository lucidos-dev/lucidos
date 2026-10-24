/** The page health log's reporter: one `[Client/page-health]` line every 30 s,
 *  sampled from the counters `pageHealthLog.ts` installed at boot. A temporary
 *  measure: see `docs/temporary-measures.md` § Page health log. */

import { postClientLog } from './clientLog';
import { pageHealthCounters } from './pageHealthLog';
import { keyboardCloseState } from '../components/layout/keyboardCloseRelayout';

const REPORT_INTERVAL_MS = 30_000;
const LAG_TICK_MS = 500;
const FPS_WINDOW_MS = 1000;

/** The largest listener counts, so a growing type stands out. */
export function topListeners(listeners: Map<string, number>, n = 6): Record<string, number> {
  return Object.fromEntries(Array.from(listeners).filter(([, c]) => c > 0).sort((a, b) => b[1] - a[1]).slice(0, n));
}

function framesPerSecond(): Promise<number> {
  return new Promise((resolve) => {
    let frames = 0;
    const end = performance.now() + FPS_WINDOW_MS;
    const tick = (t: number) => {
      frames += 1;
      if (t < end) requestAnimationFrame(tick);
      else resolve(frames);
    };
    requestAnimationFrame(tick);
  });
}

let started = false;

/** Start the 30 s reporter. Needs the counters, so a desktop starts nothing.
 *  Idempotent. Returns a teardown for a hot update. */
export function startPageHealthLog(): () => void {
  const counters = pageHealthCounters();
  if (!counters || started) return () => {};
  started = true;
  const startedAt = performance.now();
  let lagMax = 0;
  let lagSum = 0;
  let lagTicks = 0;
  let dueAt = performance.now() + LAG_TICK_MS;
  let lastRafCalls = counters.rafCalls;
  let lastReportAt = performance.now();
  let resumes = 0;
  // A suspended page runs no tick, so a resume restarts every window. The
  // suspension would otherwise read as one long main-thread stall.
  const onVisibilityChange = () => {
    if (document.visibilityState !== 'visible') return;
    resumes += 1;
    dueAt = performance.now() + LAG_TICK_MS;
    lastRafCalls = counters.rafCalls;
    lastReportAt = performance.now();
  };
  document.addEventListener('visibilitychange', onVisibilityChange);
  const lagTimer = setInterval(() => {
    const now = performance.now();
    const lag = Math.max(0, now - dueAt);
    dueAt = now + LAG_TICK_MS;
    // A hidden page is throttled, so its lag says nothing about the main thread.
    if (document.visibilityState !== 'visible') return;
    lagMax = Math.max(lagMax, lag);
    lagSum += lag;
    lagTicks += 1;
  }, LAG_TICK_MS);
  const reportTimer = setInterval(() => {
    if (document.visibilityState !== 'visible') return;
    // Read before the fps window, whose own frame requests would count.
    const rafCalls = counters.rafCalls;
    const windowStart = performance.now();
    const rafPerSec = Math.round((rafCalls - lastRafCalls) / Math.max(1, (windowStart - lastReportAt) / 1000));
    framesPerSecond().then((fps) => {
      try {
        const now = performance.now();
        const listeners = Array.from(counters.listeners.values()).reduce((a, b) => a + b, 0);
        postClientLog('page-health', 'sample', {
          uptimeMin: Math.round((now - startedAt) / 60_000),
          lagMaxMs: Math.round(lagMax),
          lagMeanMs: lagTicks ? Math.round(lagSum / lagTicks) : null,
          fps,
          rafPerSec,
          nodes: document.getElementsByTagName('*').length,
          animations: document.getAnimations?.().length ?? null,
          iframes: document.getElementsByTagName('iframe').length,
          listeners,
          topListeners: topListeners(counters.listeners),
          intervals: counters.intervals,
          observers: counters.observers,
          resumes,
          keyboardCloses: keyboardCloseState().closes,
          focused: document.activeElement?.tagName.toLowerCase() ?? null,
        });
      } finally {
        lagMax = 0;
        lagSum = 0;
        lagTicks = 0;
        lastRafCalls = counters.rafCalls;
        lastReportAt = performance.now();
      }
    }).catch(() => {
      // Best-effort telemetry with no user intent behind it: a failed sample
      // costs one line, and the next tick takes a fresh one.
    });
  }, REPORT_INTERVAL_MS);
  return () => {
    document.removeEventListener('visibilitychange', onVisibilityChange);
    clearInterval(lagTimer);
    clearInterval(reportTimer);
    started = false;
  };
}
