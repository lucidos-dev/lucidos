import {
  showToast,
  applyingNowThreadIds,
  archivingThreadIds,
  discardingCCThreadIds,
  changes,
  markThreadAnswering,
  clearThreadAnswering,
  focusedThreadId,
  TOAST_AUTO_DISMISS_MS,
} from '../store';
import { refreshChangesState } from './chat-changes';
import { applyNow, applyChange, answerThreadQuestion as apiAnswerThreadQuestion, discardCCChanges, sendControlRequest, ApiError } from '../../api/client';
import { pendingAnswers, unsentPicks } from '../pendingDecisions';
import { sendFailureOf, withQuietRetries } from './sendRetry';
import { inSendChain } from './sendChain';
import type { AnswerKind } from '../thread-events';
import { markThreadRerenderStart, clearThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { changeToastMessage } from './changeToast';
import { focusThread } from './threads';
import { errorDetail } from '../../utils/errorDetail';

// Safety timers per thread — cleared on re-arm (409 or 404-fallback) to prevent stacking.
const applyingSafetyTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Remove a thread from the optimistic Apply Now tracking map. */
function clearApplyingNow(threadId: string): void {
  const next = new Map(applyingNowThreadIds.value);
  next.delete(threadId);
  applyingNowThreadIds.value = next;
}

/** Arm a 60s safety timeout that clears the optimistic "applying" state if no
 *  SSE resolution event (ChangeApplied/ChangeApplyFailed) arrives — e.g. an SSE
 *  reconnection gap. Without it the thread is stuck in the optimistic phase
 *  forever. Re-arming clears any prior timer so it never stacks; the timer is a
 *  no-op if SSE already cleared the thread by the time it fires. */
function armApplyingSafetyTimeout(threadId: string): void {
  const prev = applyingSafetyTimers.get(threadId);
  if (prev) clearTimeout(prev);
  applyingSafetyTimers.set(threadId, setTimeout(() => {
    applyingSafetyTimers.delete(threadId);
    if (applyingNowThreadIds.value.has(threadId)) {
      clearApplyingNow(threadId);
    }
  }, 60_000));
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

/** End a running Claude Code session and immediately apply its changes.
 *  The backend handles the apply flow — SSE events update the thread status.
 *  Sets optimistic "applying" state immediately so the UI responds before SSE arrives. */
export async function endClaudeCodeAndApply(threadId: string): Promise<void> {
  if (applyingNowThreadIds.value.has(threadId)) return; // Already in progress
  if (archivingThreadIds.value.has(threadId)) return; // Can't apply while archiving
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't apply while discarding
  const next = new Map(applyingNowThreadIds.value);
  next.set(threadId, 'requesting');
  applyingNowThreadIds.value = next;
  try {
    await applyNow(threadId);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 409) {
      // The engine's message names what refused the apply. Keyed under the
      // apply's result toast, which replaces it once the apply resolves.
      const key = `applying-${threadId}`;
      const onClick = () => focusThread(threadId);
      if (refusalMeansApplying(e)) {
        showToast(changeToastMessage('Already applying', threadId, e.reason), 'info', { key, onClick, autoDismissMs: TOAST_AUTO_DISMISS_MS });
        // An apply really is running, so its SSE resolution will clear this.
        // The safety timeout covers an SSE gap dropping that resolution.
        armApplyingSafetyTimeout(threadId);
      } else {
        // A Discard, a stop ending the session, or a question that may still
        // be waiting on the user: nothing is applying.
        clearApplyingNow(threadId);
        showToast(changeToastMessage('Not applied', threadId, e.reason), 'warning', { key, onClick });
      }
      return;
    }
    if (e instanceof ApiError && e.httpCode === 404) {
      // No live session, so apply the thread's pending changes one by one. The
      // cached list can lag a proposal, or hold a failed load. So re-read it
      // before naming what is pending, and never call a change absent unread.
      // A landed read always writes a new `loaded` value, so the same object
      // means the read did not land.
      const cached = changes.value;
      await refreshChangesState();
      const fresh = changes.value;
      if (fresh === cached || fresh.status !== 'loaded') {
        clearApplyingNow(threadId);
        showToast(changeToastMessage('Not applied', threadId, 'the pending changes could not be read. Try again'), 'error', {
          key: `applying-${threadId}`,
        });
        return;
      }
      const pending = fresh.data.filter(
        c => c.thread_id === threadId && c.status === 'pending',
      );
      if (pending.length > 0) {
        try {
          for (const c of pending) {
            await applyChange(c.id);
          }
          // SSE ChangeApplied/ChangeApplyFailed events clear applyingNowThreadIds;
          // arm the same 60s safety timeout the 409 path uses so an SSE
          // reconnection gap can't strand the thread in the optimistic phase.
          armApplyingSafetyTimeout(threadId);
          return;
        } catch (applyErr) {
          clearApplyingNow(threadId);
          showToast(changeToastMessage('Failed to apply changes', threadId, errorDetail(applyErr)), 'error', {
            key: `applying-${threadId}`,
          });
          return;
        }
      }
      clearApplyingNow(threadId);
      showToast('No pending changes to apply', 'warning', { key: `applying-${threadId}` });
      return;
    }
    // API failed, so clear the optimistic state immediately. Every keyed error
    // toast in this file names its thread and its cause. A bare "Failed to
    // start apply" says neither which thread failed nor why: an
    // auth refusal, a 5xx and a worktree conflict all read alike, and two
    // threads applying at once become indistinguishable.
    clearApplyingNow(threadId);
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
  if (archivingThreadIds.value.has(threadId)) return;
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
  // no resume is coming. Held through every quiet retry.
  markThreadAnswering(threadId);
  // A new answer replaces a pick that was not sent.
  unsentPicks.clear(toolUseId);
  // In the thread's send chain, so a message typed after the tap cannot reach
  // the engine first and be taken as the card's answer.
  const result = await inSendChain(threadId, () =>
    withQuietRetries(() => apiAnswerThreadQuestion(threadId, toolUseId, answer), { path: 'answer' }));
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

/** What a control request did, as `sendCodingAgentControl` reports it.
 *  - `ok`: the live session takes it now.
 *  - `next-turn`: recorded, and it takes effect from the session's next turn.
 *  - `pending`: a 404, so no live session. The caller falls back to the
 *    pending pick, with no toast, so a benign race never looks like an error.
 *  - `error`: a hard failure, already toasted. */
export type ControlSendResult = 'ok' | 'next-turn' | 'pending' | 'error';

/** Whether the live session recorded the change, now or for its next turn. */
export function sessionRecorded(result: ControlSendResult): boolean {
  return result === 'ok' || result === 'next-turn';
}

/** Send a control request to a running coding-agent session. Generic: works
 *  with any control subtype the engine serves (set_model, set_reasoning_effort). */
export async function sendCodingAgentControl(threadId: string, request: Record<string, string>): Promise<ControlSendResult> {
  try {
    const res = await sendControlRequest(threadId, request);
    return res.takes_effect === 'next-turn' ? 'next-turn' : 'ok';
  } catch (err) {
    if (err instanceof ApiError && err.httpCode === 404) {
      return 'pending';
    }
    const detail = err instanceof ApiError ? err.reason : 'session may have ended';
    showToast(`Failed to send control request: ${detail}`, 'error');
    return 'error';
  }
}
