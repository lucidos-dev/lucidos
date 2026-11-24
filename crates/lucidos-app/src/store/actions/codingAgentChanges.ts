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

function dropApplyNowRequest(threadId: string): void {
  const next = new Map(applyNowRequests.value);
  next.delete(threadId);
  applyNowRequests.value = next;
}

const NOT_APPLYING_REFUSALS = new Set([
  'discard_in_progress',
  'session_stopping',
  'question_open',
  'question_unknown',
]);

function refusalMeansApplying(e: ApiError): boolean {
  const slug = (e.body as { reason?: unknown } | undefined)?.reason;
  return typeof slug !== 'string' || !NOT_APPLYING_REFUSALS.has(slug);
}

export async function endClaudeCodeAndApply(threadId: string): Promise<void> {
  if (applyingNowThreadIds.value.has(threadId)) return;
  if (archiveRequestInFlight(threadId)) return;
  if (discardingCCThreadIds.value.has(threadId)) return;
  const legacyVersion = (threadMap.value.get(threadId)?.meta.summaryVersion ?? 0) + 1;
  applyNowRequests.value = new Map(applyNowRequests.value).set(threadId, null);
  try {
    const { summary_version } = await applyNow(threadId);
    applyNowRequests.value = new Map(applyNowRequests.value).set(threadId, summary_version ?? legacyVersion);
  } catch (e) {
    dropApplyNowRequest(threadId);
    if (e instanceof ApiError && e.httpCode === 409) {
      const key = `applying-${threadId}`;
      const onClick = () => focusThread(threadId);
      if (refusalMeansApplying(e)) {
        showToast(changeToastMessage('Already applying', threadId, e.reason), 'info', { key, onClick, autoDismissMs: TOAST_AUTO_DISMISS_MS });
      } else {
        showToast(changeToastMessage('Not applied', threadId, e.reason), 'warning', { key, onClick });
      }
      return;
    }
    showToast(changeToastMessage('Failed to start apply', threadId, errorDetail(e)), 'error', {
      key: `applying-${threadId}`,
    });
  }
}

export async function handleDiscardCCChanges(threadId: string): Promise<void> {
  if (discardingCCThreadIds.value.has(threadId)) return;
  if (applyingNowThreadIds.value.has(threadId)) return;
  if (archiveRequestInFlight(threadId)) return;
  discardingCCThreadIds.value = new Set([...discardingCCThreadIds.value, threadId]);
  showToast(changeToastMessage('Discarding changes', threadId), 'info', { key: `discarding-${threadId}`, onClick: () => focusThread(threadId), spinning: true, autoDismissMs: 30_000 });
  try {
    await discardCCChanges(threadId);
  } catch (e) {
    showToast(changeToastMessage('Failed to discard changes', threadId, errorDetail(e)), 'error', {
      key: `discarding-${threadId}`,
    });
  } finally {
    const next = new Set(discardingCCThreadIds.value);
    next.delete(threadId);
    discardingCCThreadIds.value = next;
  }
}
