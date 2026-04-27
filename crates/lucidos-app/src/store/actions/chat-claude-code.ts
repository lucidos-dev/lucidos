import {
  showToast,
  applyingNowThreadIds,
  dismissingThreadIds,
  discardingCCThreadIds,
  changes,
} from '../store';
import { applyNow, applyChange, answerCCQuestion as apiAnswerCCQuestion, discardCCChanges, sendControlRequest, ApiError } from '../../api/client';
import type { AnswerKind } from '../thread-events';
import { scrollToBottom } from '../../components/chat/scrollState';
import { changeToastMessage } from './thread-sync';
import { focusThread } from './threads';
import { errorDetail } from '../../utils/errorDetail';

// Safety timers per thread — cleared when a new 409 arrives to prevent stacking.
const applyingSafetyTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Remove a thread from the optimistic Apply Now tracking map. */
function clearApplyingNow(threadId: string): void {
  const next = new Map(applyingNowThreadIds.value);
  next.delete(threadId);
  applyingNowThreadIds.value = next;
}

/** End a running Claude Code session and immediately apply its changes.
 *  The backend handles the apply flow — SSE events update the thread status.
 *  Sets optimistic "applying" state immediately so the UI responds before SSE arrives. */
export async function endClaudeCodeAndApply(threadId: string): Promise<void> {
  if (applyingNowThreadIds.value.has(threadId)) return; // Already in progress
  if (dismissingThreadIds.value.has(threadId)) return; // Can't apply while dismissing
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't apply while discarding
  // Pin to bottom before banner re-renders (height change would set scrolledUp=true)
  scrollToBottom();
  const next = new Map(applyingNowThreadIds.value);
  next.set(threadId, 'requesting');
  applyingNowThreadIds.value = next;
  showToast(changeToastMessage('Applying changes', threadId), 'info', { key: `applying-${threadId}`, onClick: () => focusThread(threadId), spinning: true });
  try {
    await applyNow(threadId);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 409) {
      showToast('Already applying', 'info', { spinning: true });
      // Don't clear immediately — apply is genuinely in progress on the backend.
      // Safety timeout: if no SSE resolution event (ChangeApplied/ChangeApplyFailed)
      // arrives within 60s (e.g., SSE reconnection gap), clear the stuck state.
      const prev = applyingSafetyTimers.get(threadId);
      if (prev) clearTimeout(prev);
      applyingSafetyTimers.set(threadId, setTimeout(() => {
        applyingSafetyTimers.delete(threadId);
        if (applyingNowThreadIds.value.has(threadId)) {
          clearApplyingNow(threadId);
        }
      }, 60_000));
      return;
    }
    if (e instanceof ApiError && e.httpCode === 404) {
      // No live CC session — fall back to applying pending changes directly
      const pending = changes.value.filter(c => c.thread_id === threadId && c.status === 'pending');
      if (pending.length > 0) {
        try {
          for (const c of pending) {
            await applyChange(c.id);
          }
          return; // SSE events will clear applyingNowThreadIds
        } catch (applyErr) {
          clearApplyingNow(threadId);
          showToast(`Failed to apply changes: ${errorDetail(applyErr)}`, 'error', { key: `applying-${threadId}` });
          return;
        }
      }
      clearApplyingNow(threadId);
      showToast('No pending changes to apply', 'warning', { key: `applying-${threadId}` });
      return;
    }
    // API failed — clear optimistic state immediately
    clearApplyingNow(threadId);
    showToast('Failed to start apply', 'error', { key: `applying-${threadId}` });
  }
}

/** Discard all CC changes for a thread with optimistic state tracking.
 *  Guards against concurrent apply/dismiss — mutual exclusivity with the other actions. */
export async function handleDiscardCCChanges(threadId: string): Promise<void> {
  if (discardingCCThreadIds.value.has(threadId)) return;
  if (applyingNowThreadIds.value.has(threadId)) return;
  if (dismissingThreadIds.value.has(threadId)) return;
  // Pin to bottom before banner re-renders (height change would set scrolledUp=true)
  scrollToBottom();
  discardingCCThreadIds.value = new Set([...discardingCCThreadIds.value, threadId]);
  try {
    await discardCCChanges(threadId);
    showToast('Changes discarded', 'success');
  } catch (e) {
    showToast(`Failed to discard changes: ${errorDetail(e)}`, 'error');
  } finally {
    const next = new Set(discardingCCThreadIds.value);
    next.delete(threadId);
    discardingCCThreadIds.value = next;
  }
}

/** Answer a CC `AskUserQuestion`. Returns true on success, false on 409
 *  (stale or duplicate). Toasts and re-throws on other errors. */
export async function answerCCQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
): Promise<boolean> {
  try {
    return await apiAnswerCCQuestion(threadId, toolUseId, answer);
  } catch (err) {
    const detail = err instanceof ApiError ? err.reason : 'unknown error';
    showToast(`Failed to send answer: ${detail}`, 'error');
    return false;
  }
}

/** Send a control request to a running Claude Code session.
 *  Generic — works with any CC control subtype (set_model, set_permission_mode, etc.). */
export async function sendCCControl(threadId: string, request: Record<string, string>): Promise<boolean> {
  try {
    await sendControlRequest(threadId, request);
    return true;
  } catch (err) {
    const detail = err instanceof ApiError ? err.reason : 'session may have ended';
    showToast(`Failed to send control request — ${detail}`, 'error');
    return false;
  }
}
