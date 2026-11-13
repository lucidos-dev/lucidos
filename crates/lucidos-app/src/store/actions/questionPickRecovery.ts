import { unsentPicks, type UnsentPick } from '../pendingDecisions';
import { askedOptions, questionAnswerText } from '../quietAnswer';
import { showToast, threadMap } from '../store';
import { composeInFreshDraft } from './compose';
import { engineRecordedMessage } from './chat';

/** A resolved or missing card cannot hide a composer answer with an unknown outcome. */
export function recoverQuestionPick(toolUseId: string, pick: UnsentPick, storedText?: string): void {
  const eventId = pick.placement?.event_id;
  const recorded = eventId && engineRecordedMessage(pick.threadId, eventId);
  if (!recorded) {
    const images = 'image_hashes' in pick.answer ? pick.answer.image_hashes ?? [] : [];
    const options = askedOptions(threadMap.peek().get(pick.threadId), toolUseId);
    composeInFreshDraft(storedText ?? questionAnswerText(pick.answer, options), images);
    showToast('An unconfirmed answer is back in a new draft. It may already have been sent.', 'warning');
  }
  unsentPicks.clear(toolUseId);
}
