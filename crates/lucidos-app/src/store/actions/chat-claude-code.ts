import {
  showToast,
  threadMap,
  markThreadAnswering,
  clearThreadAnswering,
  focusedThreadId,
} from '../store';
import { answerThreadQuestion as apiAnswerThreadQuestion, sendControlRequest, ApiError, type AnswerPlacement } from '../../api/client';
import { persistSendingMessage, forgetUnsentMessageRecord, markUnsentMessageRecordUnsent } from '../unsentMessageRecords';
import { pendingAnswers, unsentPicks } from '../pendingDecisions';
import { answerIsQuiet, askedOptions, questionAnswerText } from '../quietAnswer';
import { inputRefusalOf, sendFailureOf, withQuietRetries } from './sendRetry';
import { enterSendChain } from './sendChain';
import { knownComposeEpoch, parseComposeReport, acceptComposeReport } from '../composeSync';
import type { AnswerKind } from '../thread-events';
import { markThreadRerenderStart, clearThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { errorDetail } from '../../utils/errorDetail';
import { generateUuid } from '../../utils/uuid';
import { probeStreamAfterSend } from './send-probe';

export function answerFailureMessage(
  failure: { kind: 'conflict' } | { kind: 'error'; err: unknown },
): string {
  if (failure.kind === 'conflict') {
    return 'Could not send answer: that question is no longer waiting for one.';
  }
  const { err } = failure;
  if (err instanceof ApiError) return `Could not send answer: ${err.reason}`;
  return `Could not send answer: ${errorDetail(err)}`;
}

export type AnswerOutcome = 'sent' | 'unsent' | 'refused';

function questionAnswerRecorded(threadId: string, toolUseId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread) return false;
  for (const [seq, stored] of thread.events) {
    if (seq > 0 && stored.type === 'UserQuestionAnswered' && stored.tool_use_id === toolUseId) return true;
  }
  return false;
}

export async function answerThreadQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
  retryPlacement?: AnswerPlacement,
): Promise<AnswerOutcome> {
  const wireAnswer = structuredClone(answer);
  const composerAnswer = wireAnswer.kind === 'FreeText' || (wireAnswer.kind === 'MultiSelected' && (!!wireAnswer.text || !!wireAnswer.image_hashes?.length));
  if (focusedThreadId.value === threadId) {
    markThreadRerenderStart(threadId, { ...currentPerfBaseline(), cause: 'answer' });
  }
  if (!answerIsQuiet(answer, askedOptions(threadMap.value.get(threadId), toolUseId))) {
    markThreadAnswering(threadId);
  }
  unsentPicks.clear(toolUseId);
  const eventId = retryPlacement?.event_id ?? generateUuid();
  const slot = enterSendChain(threadId, retryPlacement ? retryPlacement.compose_epoch : knownComposeEpoch(threadId));
  let placement: AnswerPlacement | undefined = retryPlacement ? { ...retryPlacement } : undefined;
  const persistAnswer = (posting: AnswerPlacement) => {
    if (!composerAnswer) return;
    persistSendingMessage({
      eventId, threadId, body: { message: questionAnswerText(wireAnswer, askedOptions(threadMap.peek().get(threadId), toolUseId)), image_hashes: 'image_hashes' in wireAnswer ? wireAnswer.image_hashes : undefined, thread_id: threadId, event_id: eventId, mode: 'human' },
      settlement: { kind: 'follow-up' }, sentAt: new Date().toISOString(),
      questionPick: { toolUseId, answer: wireAnswer, placement: posting },
    });
  };
  persistAnswer(placement ?? { event_id: eventId, ...slot.position, compose_epoch: knownComposeEpoch(threadId) });
  const result = await (async () => {
    try {
      if (slot.waitForTurn) await slot.waitForTurn;
      const posting = placement ?? { event_id: eventId, ...slot.position, compose_epoch: slot.epochForPost() };
      placement = posting;
      persistAnswer(posting);
      return await withQuietRetries(
        () => apiAnswerThreadQuestion(threadId, toolUseId, wireAnswer, posting, (wire) => {
          if (wire) {
            acceptComposeReport(threadId, parseComposeReport(wire));
            if (!retryPlacement) slot.accepted(wire.compose_epoch ?? undefined);
          }
        }),
        { path: 'answer' },
      );
    } finally {
      slot.release();
    }
  })();
  if (result.kind === 'done' && (result.value || (!composerAnswer && result.attempts > 1))) {
    if (composerAnswer) forgetUnsentMessageRecord(eventId);
    probeStreamAfterSend(threadId, () => questionAnswerRecorded(threadId, toolUseId));
    return 'sent';
  }
  clearThreadAnswering(threadId);
  clearThreadRerenderStart(threadId);
  const refusal = result.kind === 'refused' ? inputRefusalOf(result.error) : undefined;
  if (result.kind !== 'landed' && (composerAnswer || result.kind === 'gave-up' || refusal)) {
    if (composerAnswer) markUnsentMessageRecordUnsent(eventId, 0, refusal);
    unsentPicks.set(toolUseId, { threadId, answer: wireAnswer, placement, refusal,
      ...('error' in result ? { failure: sendFailureOf(result) } : {}),
    });
    if (result.kind === 'done') probeStreamAfterSend(threadId, () => questionAnswerRecorded(threadId, toolUseId));
    return 'unsent';
  }
  if (composerAnswer) forgetUnsentMessageRecord(eventId);
  showToast(answerFailureMessage(result.kind === 'refused' ? { kind: 'error', err: result.error } : { kind: 'conflict' }), 'error');
  return 'refused';
}

export async function retryUnsentPick(toolUseId: string): Promise<AnswerOutcome | null> {
  const pick = unsentPicks.map.value.get(toolUseId);
  if (!pick) return null;
  pendingAnswers.set(toolUseId, pick.answer);
  const outcome = await answerThreadQuestion(pick.threadId, toolUseId, pick.answer, pick.refusal ? undefined : pick.placement);
  if (outcome !== 'sent') pendingAnswers.clear(toolUseId);
  return outcome;
}

export async function sendCodingAgentControl(threadId: string, request: Record<string, string>): Promise<boolean> {
  try {
    await sendControlRequest(threadId, request);
    return true;
  } catch (err) {
    const detail = err instanceof ApiError ? err.reason : errorDetail(err);
    showToast(`Could not change the model and effort: ${detail}`, 'error');
    return false;
  }
}
