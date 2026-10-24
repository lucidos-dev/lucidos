import {
  showToast,
  applyingNowThreadIds,
  applyNowRequests,
  discardingCCThreadIds,
  threadMap,
  markThreadAnswering,
  clearThreadAnswering,
  focusedThreadId,
  TOAST_AUTO_DISMISS_MS,
} from '../store';
import { archiveRequestInFlight } from '../rowIntents';
import { applyNow, answerThreadQuestion as apiAnswerThreadQuestion, discardCCChanges, sendControlRequest, ApiError } from '../../api/client';
import { pendingAnswers, unsentPicks } from '../pendingDecisions';
import { answerIsQuiet, askedOptions } from '../quietAnswer';
import { sendFailureOf, withQuietRetries } from './sendRetry';
import { inSendChain } from './sendChain';
import type { AnswerKind } from '../thread-events';
import { markThreadRerenderStart, clearThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { changeToastMessage } from './changeToast';
import { focusThread } from './threads';
import { errorDetail } from '../../utils/errorDetail';
import { generateUuid } from '../../utils/uuid';

/** End the click's bridge: the engine refused it, or it failed to send. */
function dropApplyNowRequest(threadId: string): void {
  const next = new Map(applyNowRequests.value);
  next.delete(threadId);
  applyNowRequests.value = next;
}

/** Refusal slugs that mean something other than an apply holds the session.
 *  The engine sends one as `reason` beside the message on an Apply Now 409. */
const NOT_APPLYING_REFUSALS = new Set([
  'discard_in_progress',
  'session_stopping',
  'question_open',
  'question_unknown',
]);

/** Whether an Apply Now 409 means an apply is genuinely running. An older
 *  engine sends no slug, and its only 409s were apply-held. */
function refusalMeansApplying(e: ApiError): boolean {
  const slug = (e.body as { reason?: unknown } | undefined)?.reason;
  return typeof slug !== 'string' || !NOT_APPLYING_REFUSALS.has(slug);
}

/** Apply a coding-agent thread's change. The engine owns the apply and
 *  projects it on the thread's row as `applyInFlight` (ADR 0437). The click
 *  shows "applying" at once, and holds until the row reaches the version the
 *  engine answered with. No timer and no other event ends it. */
export async function endClaudeCodeAndApply(threadId: string): Promise<void> {
  if (applyingNowThreadIds.value.has(threadId)) return; // Already in progress
  if (archiveRequestInFlight(threadId)) return; // Can't apply while archiving
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't apply while discarding
  // An engine that predates the version: hold until any row newer than this
  // one. Read before the request, since its frame can beat the response.
  const legacyVersion = (threadMap.value.get(threadId)?.meta.summaryVersion ?? 0) + 1;
  applyNowRequests.value = new Map(applyNowRequests.value).set(threadId, null);
  try {
    const { summary_version } = await applyNow(threadId);
    applyNowRequests.value = new Map(applyNowRequests.value).set(threadId, summary_version ?? legacyVersion);
  } catch (e) {
    dropApplyNowRequest(threadId);
    if (e instanceof ApiError && e.httpCode === 409) {
      // The engine's message names what refused the apply. Keyed under the
      // apply's result toast, which replaces it once the apply resolves. A
      // running apply already shows on the row, so nothing else is held.
      const key = `applying-${threadId}`;
      const onClick = () => focusThread(threadId);
      if (refusalMeansApplying(e)) {
        showToast(changeToastMessage('Already applying', threadId, e.reason), 'info', { key, onClick, autoDismissMs: TOAST_AUTO_DISMISS_MS });
      } else {
        // A Discard, a stop ending the session, or a question that may still
        // be waiting on the user: nothing is applying.
        showToast(changeToastMessage('Not applied', threadId, e.reason), 'warning', { key, onClick });
      }
      return;
    }
    // Every keyed error toast in this file names its thread and its cause. A
    // bare "Failed to start apply" says neither which thread failed nor why:
    // an auth refusal, a 5xx and a worktree conflict all read alike, and two
    // threads applying at once become indistinguishable.
    showToast(changeToastMessage('Failed to start apply', threadId, errorDetail(e)), 'error', {
      key: `applying-${threadId}`,
    });
  }
}

/** Discard all CC changes for a thread with optimistic state tracking.
 *  Guards against concurrent apply/dismiss — mutual exclusivity with the other actions. */
export async function handleDiscardCCChanges(threadId: string): Promise<void> {
  if (discardingCCThreadIds.value.has(threadId)) return;
  if (applyingNowThreadIds.value.has(threadId)) return;
  if (archiveRequestInFlight(threadId)) return;
  discardingCCThreadIds.value = new Set([...discardingCCThreadIds.value, threadId]);
  // autoDismissMs is a safety net for the rare zero-pending-changes case
  // where the engine emits no ChangeDiscarded event and the spinner has no
  // SSE handler to replace it. Successful discards re-key with 4s.
  showToast(changeToastMessage('Discarding changes', threadId), 'info', { key: `discarding-${threadId}`, onClick: () => focusThread(threadId), spinning: true, autoDismissMs: 30_000 });
  try {
    await discardCCChanges(threadId);
    // Success toast fires from the SSE ChangeDiscarded handler — same key replaces the spinner.
  } catch (e) {
    // Keyed, so it replaces the named spinner above. Same rule as apply.
    showToast(changeToastMessage('Failed to discard changes', threadId, errorDetail(e)), 'error', {
      key: `discarding-${threadId}`,
    });
  } finally {
    const next = new Set(discardingCCThreadIds.value);
    next.delete(threadId);
    discardingCCThreadIds.value = next;
  }
}

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
): Promise<AnswerOutcome> {
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
  const eventId = generateUuid();
  const result = await inSendChain(threadId, (position) =>
    withQuietRetries(
      () => apiAnswerThreadQuestion(threadId, toolUseId, answer, { event_id: eventId, ...position }),
      { path: 'answer' },
    ));
  // A 409 after a dropped attempt almost always means that attempt arrived:
  // the engine 409s a repeated answer. SSE shows what was recorded.
  if (result.kind === 'done' && (result.value || result.attempts > 1)) return 'sent';
  clearThreadAnswering(threadId);
  clearThreadRerenderStart(threadId); // no render coming → don't mis-fire later
  if (result.kind === 'gave-up') {
    unsentPicks.set(toolUseId, { threadId, answer, failure: sendFailureOf(result) });
    return 'unsent';
  }
  showToast(answerFailureMessage(result.kind === 'refused' ? { kind: 'error', err: result.error } : { kind: 'conflict' }), 'error');
  return 'refused';
}

/** Send an unsent pick again, sending on its card meanwhile. Resolves null
 *  when there is nothing to retry, e.g. a second press. */
export async function retryUnsentPick(toolUseId: string): Promise<AnswerOutcome | null> {
  const pick = unsentPicks.map.value.get(toolUseId);
  if (!pick) return null;
  pendingAnswers.set(toolUseId, pick.answer);
  const outcome = await answerThreadQuestion(pick.threadId, toolUseId, pick.answer);
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
