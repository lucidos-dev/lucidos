
import { onPerfEnabledChange, perfRecordingOn, recordPerfSample } from './perfQueue';

export const TICK_MS = 250;

export const STALL_MS = 150;

export function stallOf(
  now: number,
  dueAt: number,
  threshold = STALL_MS,
): { overshootMs: number; stalled: boolean } {
  const overshootMs = Math.max(0, Math.round(now - dueAt));
  return { overshootMs, stalled: overshootMs >= threshold };
}

let timer: ReturnType<typeof setInterval> | null = null;
let dueAt = 0;

function nowMs(): number {
  return performance.now();
}

function hidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden';
}

function tick(): void {
  const now = nowMs();
  if (!perfRecordingOn()) { stop(); return; }
  if (hidden()) { dueAt = now + TICK_MS; return; }
  const { overshootMs, stalled } = stallOf(now, dueAt);
  dueAt = now + TICK_MS;
  if (stalled) recordPerfSample('main-thread-stall', { overshootMs });
}

function start(): void {
  if (timer !== null || typeof setInterval === 'undefined') return;
  dueAt = nowMs() + TICK_MS;
  timer = setInterval(tick, TICK_MS);
}

function stop(): void {
  if (timer === null) return;
  clearInterval(timer);
  timer = null;
}

let installed = false;
let unsubscribeGate: (() => void) | null = null;

export function installMainThreadStallProbe(): void {
  if (installed) return;
  installed = true;
  unsubscribeGate = onPerfEnabledChange((on) => (on ? start() : stop()));
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', onVisible, { passive: true });
  }
  if (perfRecordingOn()) start();
}

function onVisible(): void {
  if (hidden()) return;
  dueAt = nowMs() + TICK_MS;
  if (perfRecordingOn()) start();
}

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

export function _stallProbeRunningForTesting(): boolean {
  return timer !== null;
}
