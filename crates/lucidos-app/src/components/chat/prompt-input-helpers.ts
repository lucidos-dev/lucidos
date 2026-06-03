import { signal } from '@preact/signals';
import { focusedThreadId, isMidTurn } from '../../store/store';
import { computeExchanges, findQuestionAnswer } from '../../store/thread-events';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import type { BannerState } from './WaitingBanner';

// Pure prompt-input logic + the optimistic-send signal. Extracted from
// PromptInput.tsx (re-exported there); imported directly by *.test.ts.

/** Thread IDs where Send was just clicked but the thread hasn't reached
 *  running/waiting_for_user_answer yet. Drives the optimistic Send→Cancel
 *  morph so the action slot doesn't flash empty during the request gap.
 *  Cleared when the thread becomes cancellable (via the effect below) or
 *  on send failure (via the catch handler in submit). */
export const submittingThreadIds = signal<Set<string>>(new Set());

/** For a thread whose Cancel was clicked while a question was on screen, the
 *  `tool_use_id` of the question that was pending at click time. The cleanup
 *  effect (PromptInput) keys the optimistic `cancelingThreadIds` release off
 *  this: once the targeted question is no longer the thread's latest pending
 *  one — it resolved (as Canceled) and the agent either idled or re-asked —
 *  the flag drops so the morph button stops sticking in disabled "Cancel...".
 *  Without it, a cancel the agent answers by re-asking leaves the thread
 *  mid-turn forever (waiting → running → waiting) and the not-mid-turn release
 *  never fires. A running-turn cancel records no entry (no question to key on)
 *  and falls back to the not-mid-turn release. */
export const canceledQuestionByThread = signal<Map<string, string>>(new Map());

/** Record (or clear) the question a thread's Cancel targeted. Pass `undefined`
 *  for a running-turn cancel so any stale entry is dropped rather than
 *  mis-keying the next release. */
export function setCanceledQuestion(threadId: string, toolUseId: string | undefined): void {
  const map = canceledQuestionByThread.value;
  if (toolUseId === undefined && !map.has(threadId)) return;
  const next = new Map(map);
  if (toolUseId === undefined) next.delete(threadId);
  else next.set(threadId, toolUseId);
  canceledQuestionByThread.value = next;
}

export function composeHasContent(
  hasText: boolean,
  attachedImagesCount: number,
  pendingUploadsCount: number,
): boolean {
  return hasText || attachedImagesCount > 0 || pendingUploadsCount > 0;
}

// The button is always rendered EXCEPT in 'hidden' mode so Send↔Cancel keeps
// its color morph without a DOM swap; the leave path snap-unmounts like the
// sibling section buttons — no fade-out, no position:absolute jump.
//   send        — visible, blue, click=submit
//   cancel      — visible, red,  click=cancel exchange
//   canceling   — visible, red,  disabled, label "Cancel..."
//   placeholder — invisible (visibility:hidden, takes space) to keep row height
//   hidden      — not rendered; banner or section buttons own the slot
type MorphMode = 'send' | 'cancel' | 'canceling' | 'placeholder' | 'hidden';

export function computeMorphMode(args: {
  hasContent: boolean;
  cancelTargetId: string | null;
  isCanceling: boolean;
  hasBannerOrSectionButtons: boolean;
}): MorphMode {
  if (args.hasContent) return 'send';
  if (args.cancelTargetId !== null) return args.isCanceling ? 'canceling' : 'cancel';
  if (args.hasBannerOrSectionButtons) return 'hidden';
  return 'placeholder';
}

// Stamp cancelTargetId BEFORE invoking send. sendCompose's sync prefix
// clears the draft and flips state→'active' (section buttons appear); if
// cancelTargetId is still null at that render, morphMode resolves to
// 'hidden', the button unmounts, and Send→Cancel blinks instead of morphing.
// Raw new sends (threadId null) have no prior button to preserve and pick up
// the new id from focusedThreadId after send's sync prefix runs setFocusedThread.
export function dispatchSend(
  threadId: string | null,
  send: () => Promise<void>,
): { promise: Promise<void>; submittedId: string | null } {
  if (threadId) {
    const next = new Set(submittingThreadIds.value);
    next.add(threadId);
    submittingThreadIds.value = next;
  }
  const promise = send();
  const submittedId = threadId ?? focusedThreadId.value;
  if (!threadId && submittedId) {
    const next = new Set(submittingThreadIds.value);
    next.add(submittedId);
    submittingThreadIds.value = next;
  }
  return { promise, submittedId };
}

// Toggled options + the textarea's custom answer each count as one selection.
// Whitespace-only text is dropped to mirror submitMultiAnswer's text.trim().
export function computeSubmitMultiCount(toggledCount: number, customAnswerText: string): number {
  return toggledCount + (customAnswerText.trim().length > 0 ? 1 : 0);
}

/** Latest unanswered `UserQuestionAsked` on the thread (single OR multi) —
 *  each pending question lives in its own divider exchange (the
 *  `UserQuestionAsked` is the exchange's `userEvent`). Returns `null` when the
 *  latest question is already answered: the engine serializes questions (one
 *  pending at a time via `walk_question_batch`), so an answered latest means
 *  nothing is pending. Callers must gate by status; this walks every exchange
 *  and is too expensive to run on every keystroke otherwise. */
export function findLatestPendingQuestion(
  thread: ThreadState | undefined,
): { toolUseId: string; multiSelect: boolean } | null {
  if (!thread) return null;
  const exchanges = computeExchanges(thread);
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    const ue = ex.userEvent;
    if (ue.type !== 'UserQuestionAsked') continue;
    if (findQuestionAnswer(ex, ue.tool_use_id)) return null;
    return { toolUseId: ue.tool_use_id, multiSelect: !!ue.multi_select };
  }
  return null;
}

/** Pending multi-select question, if the latest pending one is multi-select.
 *  Single-select questions answer through the card directly (no prompt-row
 *  Submit), so the multi-submit path ignores them. */
export function findPendingMultiSelectQuestion(
  thread: ThreadState | undefined,
): { toolUseId: string } | null {
  const q = findLatestPendingQuestion(thread);
  return q && q.multiSelect ? { toolUseId: q.toolUseId } : null;
}

/** Whether the optimistic `cancelingThreadIds` flag should be released.
 *
 *  The flag bridges the click→SSE gap after Cancel so the morph button reads
 *  "Cancel..." (disabled) and a double-tap can't re-fire. It must drop once the
 *  cancel has landed, or the button sticks disabled until reload. Two release
 *  conditions:
 *
 *   - the thread left every mid-turn status (the turn ended — nothing left to
 *     cancel); OR
 *   - the cancel targeted a question (`canceledQuestionId` set) that is no
 *     longer the thread's latest pending one (`latestPendingQuestionId`
 *     differs) — it resolved as Canceled and the agent idled or re-asked. This
 *     is the re-ask case the not-mid-turn check misses: status stays mid-turn
 *     the whole time (waiting_for_user_answer → running → waiting_for_user_answer).
 *
 *  A running-turn cancel records no `canceledQuestionId`, so only the
 *  not-mid-turn condition releases it — "Cancel..." persists until the turn
 *  actually terminates. */
export function shouldClearCanceling(
  status: ThreadStatus,
  canceledQuestionId: string | undefined,
  latestPendingQuestionId: string | undefined,
): boolean {
  if (!isMidTurn(status)) return true;
  return canceledQuestionId !== undefined && latestPendingQuestionId !== canceledQuestionId;
}

// Apply & Restart is the only case where the bottom sub-row
// [Save][Discard][Apply & Restart] still overflows a phone-width
// .prompt-actions-subrow (no flex-wrap) after Diff lifts. Lift Save too so
// [Discard][Apply & Restart] stays on a row that fits.
export function shouldLiftSectionButtons(
  isStacked: boolean,
  bannerState: BannerState | null,
): boolean {
  return Boolean(
    isStacked
      && bannerState?.type === 'actions'
      // "Apply & Restart" is the only label wide enough to overflow the
      // phone-width sub-row after Diff lifts. The restart state is carried on
      // the Apply TaggedAction's label (single-sourced from the selector).
      && bannerState.actions.some((a) => a.kind === 'apply' && a.label === 'Apply & Restart'),
  );
}
