/** Client-side perf telemetry queue. Instrumentation (thread open / answer
 *  latency) calls `recordPerfSample`; samples accumulate in a bounded in-memory
 *  buffer and flush as a BATCH to `POST /api/v1/internal/client-logs` — one
 *  request for many samples instead of one POST per sample — which the engine
 *  appends to `engine.log` as `[Client/perf] …` lines. Greppable, no DB / no
 *  events-table rows (the bloat we deliberately avoid).
 *
 *  Strictly fire-and-forget: a flush failure, an unreachable engine, or a
 *  teardown mid-flight must never throw into or block the app — telemetry is a
 *  diagnostic, not a feature path.
 *
 *  DEFAULT OFF. `recordPerfSample` is a no-op unless perf recording is enabled,
 *  so a shipping build emits no `[Client/perf]` log lines and does no batching /
 *  network. Flip it on to measure a reported lag, off the rest of the time.
 *
 *  ENABLE IT FROM Settings → System → Debugging, which is the only path that
 *  arms everything at once. It calls `setPerfEnabled`, which converges the gate
 *  and starts the probes that own a timer (`utils/mainThreadStall.ts`).
 *
 *  `localStorage.setItem('lucidos:perf','1')` from a console still works, and
 *  is a step behind. The gate is read live, so samples start at once. A
 *  timer-owning probe only learns on the next gate read, or on the next return
 *  to the foreground. Nothing can observe a same-document storage write: the
 *  `storage` event fires only for OTHER documents. Prefer the toggle. */

// Straight from `basePath` rather than through the `api/client` barrel. This
// module is on the boot path (`utils/perfProbe` takes it at startup) and needs
// one constant, which `api/client/_core.ts` only re-exports from here anyway.
// Same reasoning as `utils/clientLog.ts`, which stays a leaf for it.
import { API } from './basePath';

interface PerfSample {
  message: string;
  data: Record<string, unknown>;
}

/** Flush once the buffer reaches this many samples (keeps latency between an
 *  event and its log line short without a request per sample). */
const FLUSH_SIZE = 20;
/** Periodic flush so low-volume samples still reach the log within ~10s. */
const FLUSH_INTERVAL_MS = 10_000;
/** Hard buffer cap — when flushes fail (offline), drop the OLDEST beyond this so
 *  the buffer can't grow without bound. Matches the backend batch cap
 *  (`CLIENT_LOG_MAX_BATCH` in `api/internal.rs`) so a full-buffer flush is never
 *  rejected for length; a drift just yields a 400 that fire-and-forget swallows. */
const HARD_CAP = 100;

let buffer: PerfSample[] = [];
let timer: ReturnType<typeof setInterval> | null = null;

/** localStorage flag that gates recording. `'1'` = on. Default off. The single
 *  source of truth for the key — the Settings → System → Debugging toggle imports
 *  this rather than re-typing the string, so the toggle and the gate can't drift. */
export const PERF_FLAG_KEY = 'lucidos:perf';
/** Test override for the gate (null = consult localStorage; bool = forced). Lets
 *  unit tests enable/disable recording without depending on a localStorage env. */
let perfEnabledOverride: boolean | null = null;

/** Whether the perf flag is set in localStorage (`'1'` = on). Pure live read so a
 *  same-tab flip takes effect on the next sample without a reload; the try/catch
 *  keeps it safe where storage is unavailable (privacy mode / SSR) → false. This
 *  is the flag read both the recording gate and the Settings toggle go through. */
export function isPerfEnabled(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(PERF_FLAG_KEY) === '1';
  } catch {
    return false;
  }
}

/** Who wants telling when the gate flips. See `onPerfEnabledChange`. */
const gateListeners = new Set<(on: boolean) => void>();

/** Run something whenever perf recording is switched on or off.
 *
 *  Every OTHER reader of the gate is pull-based: `recordPerfSample` asks on each
 *  call, which costs one `localStorage` read and needs no notification. A probe
 *  that owns a TIMER cannot work that way. Polling for the flip means running
 *  the very timer the gate exists to keep unscheduled, so default-off would mean
 *  "recording nothing" rather than "doing nothing".
 *
 *  Returns its own unsubscribe. A listener that throws is swallowed, on the
 *  fire-and-forget contract above: a diagnostic must not break the toggle. */
export function onPerfEnabledChange(listener: (on: boolean) => void): () => void {
  gateListeners.add(listener);
  return () => { gateListeners.delete(listener); };
}

/** Turn perf recording on (`'1'`) or off (remove the key) for THIS device. Live —
 *  a same-tab flip takes effect on the next sample, no reload. Safe (and a no-op)
 *  where storage is unavailable; toggling diagnostics must never throw. */
export function setPerfEnabled(on: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') {
      if (on) localStorage.setItem(PERF_FLAG_KEY, '1');
      else localStorage.removeItem(PERF_FLAG_KEY);
    }
  } catch {
    /* storage unavailable — toggling perf telemetry must never throw */
  }
  // Converge on what PERSISTED, never on what was asked for. A `setItem` that
  // throws, on blocked storage or a full quota, would otherwise announce "on"
  // while the gate every reader consults stays off. The stall probe would then
  // hold an interval that can record nothing.
  perfRecordingOn();
}

/** Tell the timer-owning probes the gate moved. Never throws. */
function notifyGate(on: boolean): void {
  for (const listener of gateListeners) {
    try {
      listener(on);
    } catch {
      /* a diagnostic listener must not break the toggle that called it */
    }
  }
}

/** Whether perf recording is on. Honors the test override, else the live flag.
 *
 *  THE gate every recording path asks, and the reason it is exported: a caller
 *  that skips work when recording is off must agree with `recordPerfSample`
 *  about whether it is. `isPerfEnabled` is the raw storage read, and it ignores
 *  the test override. A caller reaching for that one is untestable, and can
 *  disagree with the queue it feeds. */
export function perfRecordingOn(): boolean {
  const on = perfEnabledOverride !== null ? perfEnabledOverride : isPerfEnabled();
  // Reading the gate is also where a CHANGE is noticed, because the flag moves
  // by routes that call nothing. The module header documents a bare
  // `localStorage.setItem` from a console, and another tab's write lands here
  // as storage alone. `announcedGate` is updated before the listeners run, so a
  // listener that records a sample re-enters to no edge and stops.
  if (announcedGate !== on) {
    announcedGate = on;
    notifyGate(on);
  }
  return on;
}

/** The gate value the listeners were last told about, or null before any read. */
let announcedGate: boolean | null = null;

/** Test-only: force the gate on/off, or `null` to defer to localStorage.
 *
 *  Notifies the gate listeners for a forced boolean, so a timer-owning probe can
 *  be driven from a test without a localStorage environment. `null` hands the
 *  decision back and announces nothing, having settled nothing. */
export function _setPerfEnabledForTesting(value: boolean | null): void {
  perfEnabledOverride = value;
  perfRecordingOn();
}

/** Keep only the newest `cap` entries (drop the oldest). Pure — unit-tested. */
export function trimToCap<T>(buf: T[], cap: number): T[] {
  return buf.length > cap ? buf.slice(buf.length - cap) : buf;
}

/** Test-only: clear the buffer AND the flush interval, so neither a re-buffered
 *  sample nor a live timer leaks from one test into the next. Not part of the
 *  runtime surface. */
export function _resetPerfQueueForTesting(): void {
  // The announced gate goes too, or the next test's first read sees no edge and
  // never tells the probes.
  announcedGate = null;
  buffer = [];
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

function ensureTimer(): void {
  if (timer !== null || typeof setInterval === 'undefined') return;
  timer = setInterval(() => flushPerfQueue(), FLUSH_INTERVAL_MS);
}

/** Buffer one perf sample (category is always `perf`). Flushes immediately once
 *  the buffer hits `FLUSH_SIZE`; otherwise the timer / page-hide flush picks it
 *  up. Never throws — the whole call is best-effort telemetry. */
export function recordPerfSample(message: string, data: Record<string, unknown>): void {
  // Default-off gate (single chokepoint for every mark) — no buffer, timer, or
  // flush unless perf recording is enabled. See the module header for how to flip
  // it on. Kept inside the fire-and-forget contract: the gate itself never throws.
  if (!perfRecordingOn()) return;
  try {
    buffer.push({ message, data });
    if (buffer.length > HARD_CAP) buffer = trimToCap(buffer, HARD_CAP);
    ensureTimer();
    if (buffer.length >= FLUSH_SIZE) flushPerfQueue();
  } catch {
    /* telemetry must never break the app */
  }
}

/** Re-buffer a failed flush's samples (ahead of any that arrived meanwhile) so a
 *  transient offline / engine-restart window doesn't silently drop them; bounded
 *  by HARD_CAP (drops the oldest under sustained failure). Only network failures
 *  re-buffer — a 4xx means the server rejected the batch, so retrying it can't
 *  help and the samples are dropped. */
function requeueFailed(failed: PerfSample[]): void {
  buffer = trimToCap([...failed, ...buffer], HARD_CAP);
}

/** Flush the buffered samples as one batched request. `keepalive` is set for the
 *  page-hide flush so an unloading document still delivers. No-op on an empty
 *  buffer. Fire-and-forget — the fetch is not awaited and never throws into app
 *  code; a network failure re-buffers for the next flush (the engine may just be
 *  restarting), a 4xx drops the batch. */
export function flushPerfQueue(keepalive = false): void {
  if (buffer.length === 0) return;
  const batch = buffer;
  buffer = [];
  const samples = batch.map((s) => ({ category: 'perf', message: s.message, data: s.data }));
  try {
    fetch(`${API}/internal/client-logs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(samples),
      keepalive,
    })
      // Only a NETWORK failure re-buffers. A non-ok response means the server
      // rejected the batch (a 400 over the length cap, say), so retrying the
      // same bytes cannot help and the samples are dropped.
      .catch(() => requeueFailed(batch));
  } catch {
    // Synchronous fetch throw (e.g. fetch undefined) — re-buffer like a network
    // failure so nothing is lost.
    requeueFailed(batch);
  }
}

// Final flush when the page is backgrounded / unloaded so in-flight samples
// aren't lost. visibilitychange:hidden is the reliable signal on iOS (pagehide
// is a backstop for other browsers).
if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') flushPerfQueue(true);
  });
}
if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', () => flushPerfQueue(true));
}
