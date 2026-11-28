import { signal } from '@preact/signals';
import { restoreAdmissionReady } from '../../utils/restoreAdmission';
import { focusedThreadId, isMidTurn } from '../../store/store';
import { computeExchanges, findQuestionAnswer, hasContentEvents, isTriggerGrantSelection } from '../../store/thread-events';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { uploadsGate, type PendingUpload } from '../../store/pendingUploads';
import { matchShortcut } from '../../store/actions/keybindings';
import type { ComposerEnterKey } from '../../store/actions/preferences';
import { isImeComposingKey } from '../../utils/ime';


export const submittingThreadIds = signal<Set<string>>(new Set());

export const promptStopRequested = signal(false);

export const promptSideQuestionRequested = signal(false);

export function sideQuestionBlocker(args: {
  threadStarted: boolean;
}): string | null {
  if (!args.threadStarted) return 'Side questions need a started thread.';
  return null;
}

export function sideQuestionModeActive(args: {
  stored: boolean;
  threadStarted: boolean;
}): boolean {
  return args.stored && args.threadStarted;
}

export { queuedUploadSends, type UploadSendIntent } from '../../store/uploadSendIntents';
import { queuedUploadSends, type UploadSendIntent } from '../../store/uploadSendIntents';

export function queueUploadSend<TContext>(
  threadId: string,
  intent: UploadSendIntent<TContext>,
): void {
  const next = new Map(queuedUploadSends.value);
  next.set(threadId, intent as UploadSendIntent);
  queuedUploadSends.value = next;
  markSubmittingThread(threadId);
}

export function takeQueuedUploadSend(threadId: string): UploadSendIntent | null {
  const intent = queuedUploadSends.value.get(threadId);
  if (!intent) return null;
  const next = new Map(queuedUploadSends.value);
  next.delete(threadId);
  queuedUploadSends.value = next;
  return intent;
}

export function clearQueuedUploadSend(threadId: string): void {
  if (!queuedUploadSends.value.has(threadId)) return;
  const next = new Map(queuedUploadSends.value);
  next.delete(threadId);
  queuedUploadSends.value = next;
  clearSubmittingThread(threadId);
}

export const uploadBlockedSends = signal<Set<string>>(new Set());

export function markUploadBlockedSend(threadId: string): void {
  if (uploadBlockedSends.value.has(threadId)) return;
  uploadBlockedSends.value = new Set(uploadBlockedSends.value).add(threadId);
}

export function clearUploadBlockedSend(threadId: string): void {
  if (!uploadBlockedSends.value.has(threadId)) return;
  const next = new Set(uploadBlockedSends.value);
  next.delete(threadId);
  uploadBlockedSends.value = next;
}

export function settleQueuedUploadSends(dispatch: (threadId: string, intent: UploadSendIntent) => void): void {
  if (!restoreAdmissionReady()) return;
  for (const threadId of uploadBlockedSends.value) {
    if (uploadsGate(threadId) !== 'failed') clearUploadBlockedSend(threadId);
  }
  for (const [threadId] of queuedUploadSends.value) {
    const gate = uploadsGate(threadId);
    if (gate === 'in-flight') continue;
    if (gate === 'failed') {
      clearQueuedUploadSend(threadId);
      markUploadBlockedSend(threadId);
      continue;
    }
    const intent = takeQueuedUploadSend(threadId);
    if (intent) dispatch(threadId, intent);
  }
}

export type UploadSendNotice =
  | { kind: 'queued'; inFlight: number; asSideQuestion: boolean }
  | { kind: 'blocked'; failed: number };

export function uploadSendNotice(
  queued: UploadSendIntent | undefined,
  blocked: boolean,
  uploads: readonly PendingUpload[],
): UploadSendNotice | null {
  const failed = uploads.filter((u) => u.state.kind === 'failed').length;
  if (blocked && failed > 0) return { kind: 'blocked', failed };
  if (!queued) return null;
  return { kind: 'queued', inFlight: uploads.length - failed, asSideQuestion: queued.asSideQuestion === true };
}

export function uploadSendNoticeText(notice: UploadSendNotice): string {
  if (notice.kind === 'blocked') {
    return notice.failed === 1
      ? 'Not sent: an image failed to upload. Retry it or remove it, then send.'
      : `Not sent: ${notice.failed} images failed to upload. Retry or remove them, then send.`;
  }
  const verb = notice.asSideQuestion ? 'Asks' : 'Sends';
  return notice.inFlight > 1
    ? `${verb} when the ${notice.inFlight} images finish uploading`
    : `${verb} when the image finishes uploading`;
}

function markSubmittingThread(threadId: string): void {
  const next = new Set(submittingThreadIds.value);
  next.add(threadId);
  submittingThreadIds.value = next;
}

export function clearSubmittingThread(threadId: string): void {
  if (!submittingThreadIds.value.has(threadId)) return;
  const next = new Set(submittingThreadIds.value);
  next.delete(threadId);
  submittingThreadIds.value = next;
}

export const CANCEL_SETTLE_MS = 1200;

const cancelSettleUntil = signal(0);
let cancelSettleTimer: ReturnType<typeof setTimeout> | null = null;

export function armCancelSettle(now: number = Date.now()): void {
  cancelSettleUntil.value = now + CANCEL_SETTLE_MS;
  if (cancelSettleTimer) clearTimeout(cancelSettleTimer);
  cancelSettleTimer = setTimeout(() => {
    cancelSettleUntil.value = 0;
    cancelSettleTimer = null;
  }, CANCEL_SETTLE_MS);
}

export function isCancelSettling(now: number = Date.now()): boolean {
  const until = cancelSettleUntil.value;
  return until !== 0 && now < until;
}

export type PromptEscapeAction = 'leave-side-question' | 'cancel' | 'blur' | 'ignore';

export function computePromptEscapeAction(
  hasCancelTarget: boolean,
  settling: boolean,
  sideQuestionMode: boolean,
): PromptEscapeAction {
  if (sideQuestionMode) return 'leave-side-question';
  if (!hasCancelTarget) return 'blur';
  return settling ? 'ignore' : 'cancel';
}

export function composeHasContent(
  text: string,
  attachedImagesCount: number,
  uploadInFlight: boolean,
): boolean {
  return text.trim().length > 0 || attachedImagesCount > 0 || uploadInFlight;
}

export function shouldClearSubmitting(status: ThreadStatus, uploadSendQueued: boolean): boolean {
  if (isMidTurn(status)) return true;
  return !uploadSendQueued;
}

type MorphMode = 'send' | 'cancel' | 'canceling' | 'empty' | 'hidden';

export function computeMorphMode(args: {
  hasContent: boolean;
  cancelTargetId: string | null;
  isCanceling: boolean;
  hasBannerOrSectionButtons: boolean;
}): MorphMode {
  if (args.hasContent) return 'send';
  if (args.cancelTargetId !== null) return args.isCanceling ? 'canceling' : 'cancel';
  if (args.hasBannerOrSectionButtons) return 'hidden';
  return 'empty';
}

export type AnswerActionMode = 'canceling' | 'submit' | 'cancel';

export function computeAnswerActionMode(args: {
  isCanceling: boolean;
  hasContent: boolean;
  multi: { pickedCount: number; text: string; imageCount: number } | null;
}): AnswerActionMode {
  if (args.isCanceling) return 'canceling';
  const { multi } = args;
  const ready = multi
    ? computeSubmitMultiCount(multi.pickedCount, multi.text) > 0 || multi.imageCount > 0
    : args.hasContent;
  return ready ? 'submit' : 'cancel';
}

export const PLACEHOLDER_NEW_THREAD = 'What can I help with?';
export const PLACEHOLDER_FOLLOW_UP = 'Post a follow up…';
export const PLACEHOLDER_ANSWERING = 'Type custom answer here…';
export const PLACEHOLDER_SIDE_QUESTION = 'Ask a side question…';

export function threadHasConversation(threadId: string | null, thread: ThreadState | undefined): boolean {
  if (threadId === null) return false;
  if (!thread) return true;
  if (thread.meta.home || thread.meta.state === 'composing') return false;
  if (!thread.eventsLoaded) return true;
  return hasContentEvents(thread.events) || thread.pendingUserMessages.length > 0;
}

export const PLACEHOLDER_AGENT_PAUSED = 'Agent paused. Mention @agent to wake it';

export function promptPlaceholder(
  hasConversation: boolean,
  answeringQuestionCard: boolean,
  sideQuestionMode: boolean,
  agentPaused = false,
): string {
  if (sideQuestionMode) return PLACEHOLDER_SIDE_QUESTION;
  if (answeringQuestionCard) return PLACEHOLDER_ANSWERING;
  if (agentPaused) return PLACEHOLDER_AGENT_PAUSED;
  return hasConversation ? PLACEHOLDER_FOLLOW_UP : PLACEHOLDER_NEW_THREAD;
}

type ComposerKeyEvent = Pick<
  KeyboardEvent,
  'key' | 'shiftKey' | 'metaKey' | 'ctrlKey' | 'altKey' | 'isComposing' | 'keyCode'
>;

export function composerEnterSends(e: ComposerKeyEvent, mode: ComposerEnterKey, mobile: boolean): boolean {
  if (e.key !== 'Enter' || isImeComposingKey(e) || mobile) return false;
  if (matchShortcut(e) !== null) return false;
  if (mode === 'send') return !e.shiftKey;
  return e.shiftKey || e.metaKey || e.ctrlKey;
}

export function dispatchSend(
  threadId: string | null,
  send: () => Promise<unknown>,
): { promise: Promise<void>; submittedId: string | null } {
  if (threadId) {
    markSubmittingThread(threadId);
  }
  const promise = send().then(() => undefined);
  const submittedId = threadId ?? focusedThreadId.value;
  if (!threadId && submittedId) {
    markSubmittingThread(submittedId);
  }
  return { promise, submittedId };
}

export function computeSubmitMultiCount(toggledCount: number, customAnswerText: string): number {
  return toggledCount + (customAnswerText.trim().length > 0 ? 1 : 0);
}

export function recoverableAnswerDraft(args: {
  sentIds: string[];
  sentText: string;
  currentIds: string[];
  currentDraft: string;
}): { ids: string[] | null; text: string | null } {
  const idsFree = args.currentIds.length === 0;
  const textFree = args.currentDraft.length === 0;
  return {
    ids: args.sentIds.length > 0 && idsFree ? args.sentIds : null,
    text: args.sentText.length > 0 && textFree ? args.sentText : null,
  };
}

export function findLatestPendingQuestion(
  thread: ThreadState | undefined,
): { toolUseId: string; multiSelect: boolean } | null {
  if (!thread) return null;
  const exchanges = computeExchanges(thread);
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    const ue = ex.userEvent;
    if (ue.type !== 'UserQuestionAsked') continue;
    if (findQuestionAnswer(ex, ue.tool_use_id) || isTriggerGrantSelection(ue)) return null;
    return { toolUseId: ue.tool_use_id, multiSelect: !!ue.multi_select };
  }
  return null;
}

export function shouldClearCanceling(
  status: ThreadStatus,
  canceledQuestionId: string | undefined,
  latestPendingQuestionId: string | undefined,
  canceledWhileAwaitingAnswer = false,
): boolean {
  if (canceledQuestionId !== undefined) {
    if (!isMidTurn(status)) return true;
    return latestPendingQuestionId !== canceledQuestionId;
  }
  if (canceledWhileAwaitingAnswer) return !isMidTurn(status);
  return status !== 'running';
}
