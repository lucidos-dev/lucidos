import {
  showToast,
  applyingNowThreadIds,
  archivingThreadIds,
  discardingCCThreadIds,
  changes,
  markThreadAnswering,
  clearThreadAnswering,
  focusedThreadId,
} from '../store';
import { loadedOr } from '../types';
import { applyNow, applyChange, answerThreadQuestion as apiAnswerThreadQuestion, discardCCChanges, sendControlRequest, ApiError } from '../../api/client';
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
  showToast(changeToastMessage('Applying changes', threadId), 'info', { key: `applying-${threadId}`, onClick: () => focusThread(threadId), spinning: true });
  try {
    await applyNow(threadId);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 409) {
      // Re-key under the existing applying toast so the new info replaces the
      // initial spinner instead of stacking a second one beside it.
      showToast('Already applying', 'info', { key: `applying-${threadId}`, spinning: true });
      // Don't clear immediately — apply is genuinely in progress on the backend.
      // The safety timeout covers an SSE reconnection gap dropping the resolution.
      armApplyingSafetyTimeout(threadId);
      return;
    }
    if (e instanceof ApiError && e.httpCode === 404) {
      // No live Claude Code session — fall back to applying pending changes directly.
      // `loadedOr([])` treats not-loaded / loading / failed as "no rows to act
      // on"; the follow-up SSE refresh will re-render with the right state.
      const pending = loadedOr(changes.value, []).filter(
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
    showToast(`Failed to discard changes: ${errorDetail(e)}`, 'error', { key: `discarding-${threadId}` });
  } finally {
    const next = new Set(discardingCCThreadIds.value);
    next.delete(threadId);
    discardingCCThreadIds.value = next;
  }
}

/** Answer a pending question card on a thread. Returns true on success,
 *  false on 409 (stale or duplicate). Toasts and returns false on other
 *  errors — callers branch on the boolean, none rely on a throw.
 *  Used for both CC's `AskUserQuestion` and the chat agent's
 *  `ask_user_question` — the QuestionCard component is agent-agnostic and
 *  the backend dispatches on the originating event's channel. */
export async function answerThreadQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
): Promise<boolean> {
  // Perf: stamp the re-render span for the `thread-rerender` mark — answering a
  // question on the focused thread flips `answeringThreadIds`, busting every
  // exchange memo → full re-render. ThreadView fires once on the next render.
  // Focused-only; fire-and-forget telemetry (utils/threadOpenMarks.ts +
  // utils/renderPhaseTimers.ts).
  if (focusedThreadId.value === threadId) {
    markThreadRerenderStart(threadId, { ...currentPerfBaseline(), cause: 'answer' });
  }
  // Optimistically mark the thread as resuming so the answered question-divider
  // doesn't flash "Aborted" while the client's status still reads
  // `waiting_for_user_answer` (see `isRenderedThreadIdle`). Cleared by the
  // PromptInput effect once the real status leaves that state, or below on a
  // 409 / failure (no resume is coming).
  markThreadAnswering(threadId);
  try {
    const ok = await apiAnswerThreadQuestion(threadId, toolUseId, answer);
    if (!ok) {
      clearThreadAnswering(threadId); // 409 — stale/duplicate, no resume
      clearThreadRerenderStart(threadId); // no render coming → don't mis-fire later
    }
    return ok;
  } catch (err) {
    clearThreadAnswering(threadId);
    clearThreadRerenderStart(threadId);
    const detail = err instanceof ApiError ? err.reason : 'unknown error';
    showToast(`Failed to send answer: ${detail}`, 'error');
    return false;
  }
}

/** Send a control request to a running Claude Code session.
 *  Generic — works with any CC control subtype (set_model, set_permission_mode, etc.).
 *  Returns:
 *    'ok'      — applied to the live session
 *    'pending' — 404 (no live session, caller falls back to pending preference);
 *                no toast shown so a benign race doesn't look like a UX error
 *    'error'   — hard failure (toast already shown) */
export async function sendCodingAgentControl(threadId: string, request: Record<string, string>): Promise<'ok' | 'pending' | 'error'> {
  try {
    await sendControlRequest(threadId, request);
    return 'ok';
  } catch (err) {
    if (err instanceof ApiError && err.httpCode === 404) {
      return 'pending';
    }
    const detail = err instanceof ApiError ? err.reason : 'session may have ended';
    showToast(`Failed to send control request — ${detail}`, 'error');
    return 'error';
  }
}
