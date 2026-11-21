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

/** Why the engine refused an answer, said once and naming the cause.
 *
 *  The two submit sites roll their optimistic state back and stay quiet, so
 *  this string is the whole account the user gets. One failed tap used to raise
 *  two toasts, and the pair a user reported read "Could not send answer. Please
 *  try again." over "Failed to send answer: unknown error". Between them they
 *  named neither the cause nor a way out.
 *
 *  A conflict is a question nobody is waiting on any more, so it says that
 *  rather than asking for a retry that would 409 forever. An answer that got
 *  no reply at all never reaches here: its card says Not sent instead. */
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

/** What became of an answer to a question card.
 *   - 'sent': the engine took it.
 *   - 'unsent': no attempt got a reply. The card shows the pick as Not sent
 *     (`unsentPicks`), with a Retry.
 *   - 'refused': the engine said no, and a toast says why. */
export type AnswerOutcome = 'sent' | 'unsent' | 'refused';

/** Does the thread already hold the engine's `UserQuestionAnswered` for this card? */
function questionAnswerRecorded(threadId: string, toolUseId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread) return false;
  for (const [seq, stored] of thread.events) {
    if (seq > 0 && stored.type === 'UserQuestionAnswered' && stored.tool_use_id === toolUseId) return true;
  }
  return false;
}

/** Answer a pending question card on a thread, with the quiet retries every
 *  send gets. Never throws.
 *
 *  This owns the failure surface for every outcome (see
 *  `answerFailureMessage`). A caller adding its own is how one failure came to
 *  say two things.
 *
 *  Used for both CC's `AskUserQuestion` and the chat agent's
 *  `ask_user_question`: the QuestionCard component is agent-agnostic and
 *  the backend dispatches on the originating event's channel. */
export async function answerThreadQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
  retryPlacement?: AnswerPlacement,
): Promise<AnswerOutcome> {
  const wireAnswer = structuredClone(answer);
  const composerAnswer = wireAnswer.kind === 'FreeText' || (wireAnswer.kind === 'MultiSelected' && (!!wireAnswer.text || !!wireAnswer.image_hashes?.length));
  // Perf: stamp the re-render span for the `thread-rerender` mark — answering a
  // question on the focused thread flips `answeringThreadIds`, busting every
  // exchange memo → full re-render. ThreadView fires once on the next render.
  // Focused-only; fire-and-forget telemetry (utils/threadOpenMarks.ts +
  // utils/renderPhaseTimers.ts).
  if (focusedThreadId.value === threadId) {
    markThreadRerenderStart(threadId, { ...currentPerfBaseline(), cause: 'answer' });
  }
  // Optimistically mark the thread as resuming so the answered question-divider
  // doesn't settle as "Done" while the client's status still reads
  // `waiting_for_user_answer` (see `isRenderedThreadIdle`). Cleared by the
  // PromptInput effect once the real status leaves that state, or below when
  // no resume is coming. Held through every quiet retry. A quiet pick wakes
  // nobody, so marking it would draw a resume that never comes.
  if (!answerIsQuiet(answer, askedOptions(threadMap.value.get(threadId), toolUseId))) {
    markThreadAnswering(threadId);
  }
  // A new answer replaces a pick that was not sent.
  unsentPicks.clear(toolUseId);
  // In the thread's send chain, so a message typed after the tap cannot reach
  // the engine first and be taken as the card's answer. Every quiet retry is
  // the same input, so it resends the same id and place.
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
  // A 409 after a dropped attempt almost always means that attempt arrived:
  // the engine 409s a repeated answer. SSE shows what was recorded.
  if (result.kind === 'done' && (result.value || (!composerAnswer && result.attempts > 1))) {
    if (composerAnswer) forgetUnsentMessageRecord(eventId);
    probeStreamAfterSend(threadId, () => questionAnswerRecorded(threadId, toolUseId));
    return 'sent';
  }
  clearThreadAnswering(threadId);
  clearThreadRerenderStart(threadId); // no render coming → don't mis-fire later
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

/** Send an unsent pick again, sending on its card meanwhile. Resolves null
 *  when there is nothing to retry, e.g. a second press. */
export async function retryUnsentPick(toolUseId: string): Promise<AnswerOutcome | null> {
  const pick = unsentPicks.map.value.get(toolUseId);
  if (!pick) return null;
  pendingAnswers.set(toolUseId, pick.answer);
  const outcome = await answerThreadQuestion(pick.threadId, toolUseId, pick.answer, pick.refusal ? undefined : pick.placement);
  if (outcome !== 'sent') pendingAnswers.clear(toolUseId);
  return outcome;
}

/** Record a model or effort pick for a started coding-agent thread, which
 *  its next turn runs on. Resolves false on a failure, already toasted. The
 *  engine refuses a pick while a turn runs; the toast carries its reason. */
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
