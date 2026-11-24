
import type { ChatRequestBody } from '../../api/types';
import { endUnsentMessage, type SendSettlement } from '../unsentMessages';
import { appendMessagesToCompose, composeInFreshDraft, rollBackFirstSend, settleAcceptedFirstSend } from './compose';

export function settleAcceptedSend(threadId: string, settlement: SendSettlement): void {
  if (settlement.kind === 'first-send') settleAcceptedFirstSend(threadId);
}

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
      composeInFreshDraft(sent.text, sent.imageHashes);
      return;
  }
}

export function settleDeliveredUnsentMessage(eventId: string): void {
  const unsent = endUnsentMessage(eventId);
  if (unsent) settleAcceptedSend(unsent.threadId, unsent.settlement);
}
