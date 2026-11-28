
import { postClientLog } from './clientLog';

export const REPORT_INTERVAL_MS = 5 * 60 * 1000;

const MAX_ERROR_LEN = 200;

export interface IpcHealth {
  readonly failures: number;
  readonly reportedAt: number | null;
}

export const healthyIpc: IpcHealth = { failures: 0, reportedAt: null };

export type IpcReport = 'failing' | 'recovered' | null;

export function onIpcFailure(health: IpcHealth, nowMs: number): { health: IpcHealth; report: IpcReport } {
  const failures = health.failures + 1;
  const due = health.reportedAt === null || nowMs - health.reportedAt >= REPORT_INTERVAL_MS;
  return due
    ? { health: { failures, reportedAt: nowMs }, report: 'failing' }
    : { health: { failures, reportedAt: health.reportedAt }, report: null };
}

export function onIpcSuccess(health: IpcHealth): { health: IpcHealth; report: IpcReport } {
  return { health: healthyIpc, report: health.reportedAt === null ? null : 'recovered' };
}

export function describeIpcError(error: unknown): string {
  const text =
    error instanceof Error ? error.message : typeof error === 'string' ? error : String(error);
  return text.slice(0, MAX_ERROR_LEN);
}

const health = new Map<string, IpcHealth>();

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
