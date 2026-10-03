import { signal } from '@preact/signals';
import type { ChatRequestBody } from '../api/types';

/** What a sender still owes once its send is decided after the first attempt.
 *  The first attempt reports its own outcome, so these run only for a later
 *  decision. */
export interface SendSettlement {
  /** The engine took the send: a Retry was accepted, or its own row arrived. */
  onAccepted?: () => void;
  /** The engine refused a Retry. Puts the text back where it can be sent. */
  onRefused?: () => void;
}

/** A send whose POST never got an answer, so the engine may never have seen it.
 *  The thread shows it as an *unsent message* with a Retry button
 *  (`actions/chat.ts`). Page memory only: a reload drops it. */
export interface UnsentMessage {
  threadId: string;
  /** The exact request the send built. Retry re-posts it, event id included,
   *  and the engine acks a repeat without running it twice. */
  body: ChatRequestBody;
  /** Retries that also got no answer. */
  failedRetries: number;
  settlement: SendSettlement;
}

/** Keyed by the send's client event id, which is also the starter id of the
 *  exchange that draws it. */
export const unsentMessages = signal<ReadonlyMap<string, UnsentMessage>>(new Map());

export function recordUnsentMessage(eventId: string, message: UnsentMessage): void {
  const next = new Map(unsentMessages.value);
  next.set(eventId, message);
  unsentMessages.value = next;
}

/** Remove and return the record, so a second press finds nothing to send. */
export function takeUnsentMessage(eventId: string): UnsentMessage | undefined {
  const message = unsentMessages.value.get(eventId);
  if (!message) return undefined;
  const next = new Map(unsentMessages.value);
  next.delete(eventId);
  unsentMessages.value = next;
  return message;
}

/** The engine's own row for an unsent message arrived: its POST landed and
 *  only the answer was lost. Settle it as accepted. */
export function settleDeliveredUnsentMessage(eventId: string): void {
  takeUnsentMessage(eventId)?.settlement.onAccepted?.();
}
