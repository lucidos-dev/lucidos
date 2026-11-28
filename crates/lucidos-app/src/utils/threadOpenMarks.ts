
import type { PerfBaseline } from './renderPhaseTimers';

export type RerenderCause = 'send' | 'answer';

export interface RerenderBaseline extends PerfBaseline {
  cause: RerenderCause;
}

export interface OpenBaseline extends PerfBaseline {
  warm: boolean;
}

const openMarks = new Map<string, OpenBaseline>();
const rerenderMarks = new Map<string, RerenderBaseline>();

export function markThreadOpenStart(threadId: string, baseline: OpenBaseline): void {
  openMarks.set(threadId, baseline);
}

export function takeThreadOpenStart(threadId: string): OpenBaseline | undefined {
  const base = openMarks.get(threadId);
  if (base !== undefined) openMarks.delete(threadId);
  return base;
}

export function markThreadRerenderStart(threadId: string, baseline: RerenderBaseline): void {
  rerenderMarks.set(threadId, baseline);
}

export function takeThreadRerenderStart(threadId: string): RerenderBaseline | undefined {
  const base = rerenderMarks.get(threadId);
  if (base !== undefined) rerenderMarks.delete(threadId);
  return base;
}

export function clearThreadRerenderStart(threadId: string): void {
  rerenderMarks.delete(threadId);
}

export function _resetThreadOpenMarksForTesting(): void {
  openMarks.clear();
  rerenderMarks.clear();
}
