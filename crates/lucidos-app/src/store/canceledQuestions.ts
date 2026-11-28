
import { signal } from '@preact/signals';

export const canceledQuestionByThread = signal<Map<string, string>>(new Map());

export function setCanceledQuestion(threadId: string, toolUseId: string | undefined): void {
  const map = canceledQuestionByThread.value;
  if (toolUseId === undefined && !map.has(threadId)) return;
  const next = new Map(map);
  if (toolUseId === undefined) next.delete(threadId);
  else next.set(threadId, toolUseId);
  canceledQuestionByThread.value = next;
}

export const canceledWhileAwaitingByThread = signal<Set<string>>(new Set());

export function setCanceledWhileAwaiting(threadId: string, awaiting: boolean): void {
  const set = canceledWhileAwaitingByThread.value;
  if (awaiting === set.has(threadId)) return;
  const next = new Set(set);
  if (awaiting) next.add(threadId);
  else next.delete(threadId);
  canceledWhileAwaitingByThread.value = next;
}
