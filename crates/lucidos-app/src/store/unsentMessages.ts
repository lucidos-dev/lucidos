import { signal } from '@preact/signals';
import type { ChatRequestBody } from '../api/types';
import type { ComposeMode, ServerDraft } from './actions/compose';
import { forgetUnsentMessageRecord } from './unsentMessageRecords';

/** Which kind of send this was, which decides what is owed once the engine
 *  takes or refuses it (`settleAcceptedSend`, `restoreRefusedSend`). Plain
 *  data, so a record kept across a reload settles exactly as it would have. */
export type SendSettlement =
  /** A compose draft's first message. Accepted consumes the draft's picks;
   *  refused rolls the draft back, mode included. `engineDraftAtSend` is the
   *  engine's draft as it stood at the send, when this device knew it. */
  | { kind: 'first-send'; mode: ComposeMode | null; engineDraftAtSend?: ServerDraft }
  /** A message to a thread the engine has. Refused takes it into the draft. */
  | { kind: 'follow-up' }
  /** A message that creates its thread. Refused starts a fresh draft. */
  | { kind: 'raw-new' };

/** A send whose POST never got an answer, so the engine may never have seen it.
 *  The thread shows it as an *unsent message* with a Retry button
 *  (`actions/chat.ts`). Its stored copy (`unsentMessageRecords.ts`) brings it
 *  back after a reload. */
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

/** Take the record and end its stored copy: the send was decided after the
 *  first attempt, or the user discarded it. */
export function endUnsentMessage(eventId: string): UnsentMessage | undefined {
  forgetUnsentMessageRecord(eventId);
  return takeUnsentMessage(eventId);
}

/** A deleted thread's unsent messages go with it. */
export function forgetUnsentMessagesOfThread(threadId: string): void {
  for (const [eventId, message] of unsentMessages.value) {
    if (message.threadId === threadId) endUnsentMessage(eventId);
  }
}
