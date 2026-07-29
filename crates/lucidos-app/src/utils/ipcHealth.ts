/** Durable "the Tauri IPC bridge is broken" signal.
 *
 *  A total IPC failure — every `invoke` rejected by the ACL after the tauri
 *  2.11 bump — ran for a month without a single trace anywhere a user or
 *  maintainer would look. Nothing was hiding it on purpose; it was just that
 *  every call site swallowed its own rejection (`invoke('heartbeat').catch(() =>
 *  {})`, `console.warn` in the native-push handlers), and a packaged `.app`
 *  launched from Finder has no console to warn into. The only symptom that
 *  escaped was a webview reload every 60s, which reads as a WKWebView crash.
 *
 *  So the signal is taken at the ONE place every command passes through
 *  (`invoke` in utils/tauri) rather than at each call site, and it goes to the
 *  engine log, which survives the process and is readable without a debugger:
 *  grep engine.log for `[Client/ipc]`.
 *
 *  Telemetry carve-out (.claude/rules/frontend.md): reporting is log-only, never
 *  a toast. These failures are observed on background work with no user intent
 *  behind it — the 15s heartbeat, an SSE-driven native banner — and the call
 *  sites that DO have user intent still surface their own errors as they always
 *  did. Rate-limited rather than unbounded (see REPORT_INTERVAL_MS): a dead
 *  bridge fails several times a minute, and drowning engine.log would hide the
 *  problem just as effectively as saying nothing.
 *
 *  Leaf module by design — it reaches the engine through utils/clientLog, not
 *  utils/liveness, so that utils/tauri does not end up importing the store. */

import { postClientLog } from './clientLog';

/** After a command's first failure is reported, at most one more line per this
 *  window until that command recovers. Five minutes keeps a permanently-denied
 *  command at ~12 lines/hour: impossible to miss when scanning, impossible to
 *  drown in. The commands actually invoked on a loop are few (the 15s heartbeat,
 *  the update poll), so even a totally dead bridge stays in that order. */
export const REPORT_INTERVAL_MS = 5 * 60 * 1000;

/** Longest error text carried into a breadcrumb. The engine caps the serialized
 *  `data` at 4KB and answers 400 over it, which would lose the report entirely. */
const MAX_ERROR_LEN = 200;

/** `readonly` because [`healthyIpc`] is a shared sentinel handed back as the
 *  reset state: the folds below always build a NEW object, and an in-place
 *  `health.failures++` added later would corrupt that sentinel for every caller
 *  at once. The type makes that unwritable rather than merely unwritten. */
export interface IpcHealth {
  /** Failures since the last success. */
  readonly failures: number;
  /** When a failure was last reported, epoch ms; `null` while healthy. */
  readonly reportedAt: number | null;
}

export const healthyIpc: IpcHealth = { failures: 0, reportedAt: null };

/** What the caller should write to the log, if anything. Pure — decided
 *  separately from the writing so the rate limiting is testable without a fetch
 *  or a clock. */
export type IpcReport = 'failing' | 'recovered' | null;

/** Fold a failed `invoke` into the health state. Reports the FIRST failure
 *  immediately (a broken bridge shows up in the log within one heartbeat of
 *  load), then at most once per [`REPORT_INTERVAL_MS`]. */
export function onIpcFailure(health: IpcHealth, nowMs: number): { health: IpcHealth; report: IpcReport } {
  const failures = health.failures + 1;
  const due = health.reportedAt === null || nowMs - health.reportedAt >= REPORT_INTERVAL_MS;
  return due
    ? { health: { failures, reportedAt: nowMs }, report: 'failing' }
    : { health: { failures, reportedAt: health.reportedAt }, report: null };
}

/** Fold a successful `invoke` into the health state. Reports recovery exactly
 *  once, and only if a failure was reported — so the log always shows a matched
 *  broken/recovered pair rather than a dangling "failing" line. */
export function onIpcSuccess(health: IpcHealth): { health: IpcHealth; report: IpcReport } {
  // `reportedAt === null` means nothing was ever written for this outage (either
  // it was always healthy, or the one failure never got past the bar), so there
  // is nothing to close out.
  return { health: healthyIpc, report: health.reportedAt === null ? null : 'recovered' };
}

/** Trim an unknown throwable to a short, loggable string. */
export function describeIpcError(error: unknown): string {
  const text =
    error instanceof Error ? error.message : typeof error === 'string' ? error : String(error);
  return text.slice(0, MAX_ERROR_LEN);
}

/** Health PER COMMAND, not one shared state — an entry exists only while that
 *  command is failing.
 *
 *  Sharing it across commands looks fine for the total-bridge-failure case this
 *  was built for, and is wrong for the far more likely partial one: a single
 *  command denied (one missing ACL permission) while the 15s heartbeat keeps
 *  succeeding. Each heartbeat would reset the shared state and log a
 *  `invoke-recovered` for something that never recovered, and the reset would
 *  re-arm the "first failure reports immediately" branch — so the denied command
 *  would log a failure EVERY time instead of once per window, defeating the rate
 *  limit exactly when it matters. Keyed by command, both properties hold, and the
 *  log names which commands are actually broken.
 *
 *  Bounded by the command vocabulary (~30, from `generate_handler!`), and healthy
 *  commands are evicted, so the map holds only the currently-failing set. */
const health = new Map<string, IpcHealth>();

/** Record the outcome of one `invoke`, emitting a `[Client/ipc]` breadcrumb when
 *  the state machine says it is worth a line. Called from utils/tauri for every
 *  command; must never throw, since it runs inside the IPC wrapper. */
export function recordIpcOutcome(command: string, error?: unknown): void {
  const current = health.get(command) ?? healthyIpc;
  const { health: next, report } =
    error === undefined ? onIpcSuccess(current) : onIpcFailure(current, Date.now());
  if (next.failures === 0) {
    health.delete(command);
  } else {
    health.set(command, next);
  }
  if (report === 'failing') {
    postClientLog('ipc', 'invoke-failed', {
      command,
      failures: next.failures,
      error: describeIpcError(error),
    });
  } else if (report === 'recovered') {
    postClientLog('ipc', 'invoke-recovered', { command, after_failures: current.failures });
  }
}
