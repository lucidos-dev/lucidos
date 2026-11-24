import { signal } from '@preact/signals';
import type { ChatRequestBody } from '../api/types';
import type { ComposeMode, ServerDraft } from './actions/compose';
import { forgetUnsentMessageRecord } from './unsentMessageRecords';
import type { SendFailure } from './actions/sendRetry';

export type SendSettlement =
  | { kind: 'first-send'; mode: ComposeMode | null; engineDraftAtSend?: ServerDraft }
  | { kind: 'follow-up' }
  | { kind: 'raw-new' };

export interface UnsentMessage {
  threadId: string;
  body: ChatRequestBody;
  failedRetries: number;
  settlement: SendSettlement;
  failure?: SendFailure;
  refusal?: { reason: 'overtaken' | 'compose_snapshot_stale' | 'compose_fence_required'; message: string };
  answersQuestion?: string;
}

export const unsentMessages = signal<ReadonlyMap<string, UnsentMessage>>(new Map());

export function recordUnsentMessage(eventId: string, message: UnsentMessage): void {
  const next = new Map(unsentMessages.value);
  next.set(eventId, message);
  unsentMessages.value = next;
}

export function takeUnsentMessage(eventId: string): UnsentMessage | undefined {
  const message = unsentMessages.value.get(eventId);
  if (!message) return undefined;
  const next = new Map(unsentMessages.value);
  next.delete(eventId);
  unsentMessages.value = next;
  return message;
}

export function endUnsentMessage(eventId: string): UnsentMessage | undefined {
  forgetUnsentMessageRecord(eventId);
  return takeUnsentMessage(eventId);
}

export function forgetUnsentMessagesOfThread(threadId: string): void {
  for (const [eventId, message] of unsentMessages.value) {
    if (message.threadId === threadId) endUnsentMessage(eventId);
  }
}
