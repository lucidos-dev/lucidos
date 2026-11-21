import {
  showToast,
  applyingNowThreadIds,
  applyNowRequests,
  discardingCCThreadIds,
  threadMap,
  TOAST_AUTO_DISMISS_MS,
} from '../store';
import { archiveRequestInFlight } from '../rowIntents';
import { applyNow, discardCCChanges, ApiError } from '../../api/client';
import { changeToastMessage } from './changeToast';
import { focusThread } from './threads';
import { errorDetail } from '../../utils/errorDetail';

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
  // With no pending changes, the engine emits no ChangeDiscarded event.
  // autoDismissMs clears the spinner in that case. Successful discards re-key with 4s.
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
