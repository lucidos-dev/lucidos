/** What a send owes once the engine decides it after the first attempt: a
 *  Retry accepted or refused, or the engine's own row arriving. The first
 *  attempt's sender settles its own outcome through the same compose actions.
 *  A settlement is data (`SendSettlement`), so a record kept across a reload
 *  settles exactly as it would have in the page that sent it. */

import type { ChatRequestBody } from '../../api/types';
import { endUnsentMessage, type SendSettlement } from '../unsentMessages';
import { appendMessagesToCompose } from './chat';
import { composeInFreshDraft, rollBackFirstSend, settleAcceptedFirstSend } from './compose';

export function settleAcceptedSend(threadId: string, settlement: SendSettlement): void {
  if (settlement.kind === 'first-send') settleAcceptedFirstSend(threadId);
}

/** Put a refused send's text and images back where they can be sent again. */
export function restoreRefusedSend(threadId: string, body: ChatRequestBody, settlement: SendSettlement): void {
  const sent = { text: body.message, imageHashes: body.image_hashes ?? [] };
  switch (settlement.kind) {
    case 'first-send':
      rollBackFirstSend(threadId, { ...sent, mode: settlement.mode });
      return;
    case 'follow-up':
      appendMessagesToCompose(threadId, [sent]);
      return;
    case 'raw-new':
      // The engine never made the thread, and its optimistic row is gone.
      composeInFreshDraft(sent.text, sent.imageHashes);
      return;
  }
}

/** The engine's own row for an unsent message arrived: its POST landed and
 *  only the answer was lost. Settle it as accepted. */
export function settleDeliveredUnsentMessage(eventId: string): void {
  const unsent = endUnsentMessage(eventId);
  if (unsent) settleAcceptedSend(unsent.threadId, unsent.settlement);
}
