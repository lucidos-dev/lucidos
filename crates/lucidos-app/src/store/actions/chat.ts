import {
  threadsLoaded,
  panelUrl,
  panelTitle,
  showToast,
  focusedThreadId,
  threadMap,
  selectedScope,
  selectedCodingAgent,
  scopeToFolder,
  cancelingThreadIds,
  removingQueuedMessageIds,
  queuedMessageRemovalKey,
  setFocusedThread,
} from '../store';
import type { ChatContext } from './chatContext';
import { retainSendingImageHashes } from '../pendingUploadRecords';
import { cancelHookInput } from '../../api/client/eventHooks';
import type { ChatRequestBody } from '../../api/types';
import { submitChat, cancelChat, stopClaudeCode, removeQueuedMessage as removeQueuedMessageRequest, ApiError, type CodingAgentModelValue, type CodingAgentReasoningEffort } from '../../api/client';
import { inputRefusalOf, sendFailureOf, withQuietRetries, type SendFailure } from './sendRetry';
import { enterSendChain, type SendSlot } from './sendChain';
import { getDeviceId } from './devices';
import { endUnsentMessage, recordUnsentMessage, takeUnsentMessage, unsentMessages, type SendSettlement } from '../unsentMessages';
import { forgetUnsentMessageRecord, markUnsentMessageRecordUnsent, persistSendingMessage, type UnsentMessageRecord } from '../unsentMessageRecords';
import { restoreRefusedSend, settleAcceptedSend } from './sendSettlement';
import { questionTheSendAnswers, unsentPicks } from '../pendingDecisions';
import { generateUuid } from '../../utils/uuid';
import { handleEvent, makeOptimisticThreadState, retireUnsentExchange, type PendingUserMessage, type StoredEvent, type QueuedMessage } from '../thread-events';
import { appendMessagesToCompose, reopenEmptyDraft } from './compose';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { bumpThreadEvents } from '../threadActivity';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';
import { formatThreadLabel } from './thread-label';
import { revealThreadPane } from './pane';
import { retireWelcomeAfterUse } from './preferences';
import { followSentMessage } from '../../components/chat/scrollStateHandle';
import { setCanceledQuestion, setCanceledWhileAwaiting } from '../canceledQuestions';
import { refreshThreadEvents, forgetThreadEventsFailures } from './thread-loading';
import { probeStreamAfterSend } from './send-probe';
import { markThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';
import { sendAddressing } from './users';
import { acceptComposeReport, knownComposeEpoch, parseComposeReport } from '../composeSync';

export const PENDING_MESSAGE_SAFETY_MS = 30_000;

type RemovedPendingMessage = {
  index: number;
  message: PendingUserMessage;
};

export function removePendingMessage(threadId: string, eventId: string): RemovedPendingMessage | null {
  const t = threadMap.value.get(threadId);
  if (!t) return null;
  const idx = t.pendingUserMessages.findIndex(m => m.eventId === eventId);
  if (idx !== -1) {
    const [message] = t.pendingUserMessages.splice(idx, 1);
    threadMap.value = new Map(threadMap.value);
    bumpThreadEvents(threadId);
    return { index: idx, message };
  }
  return null;
}

function restorePendingMessage(threadId: string, removed: RemovedPendingMessage | null): void {
  if (!removed) return;
  const t = threadMap.value.get(threadId);
  if (!t || t.pendingUserMessages.some(m => m.eventId === removed.message.eventId)) return;
  const idx = Math.max(0, Math.min(removed.index, t.pendingUserMessages.length));
  t.pendingUserMessages.splice(idx, 0, removed.message);
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(threadId);
}

export type QueuedRemovalOutcome = 'removed' | 'already-injected' | 'failed';

type QueuedRemovalResult = { outcome: QueuedRemovalOutcome; error?: unknown; restoredByStop?: boolean };

const inFlightQueuedRemovals = new Map<string, Promise<QueuedRemovalResult>>();

function retractQueuedMessage(threadId: string, messageId: string): Promise<QueuedRemovalResult> {
  const key = queuedMessageRemovalKey(threadId, messageId);
  const existing = inFlightQueuedRemovals.get(key);
  if (existing) return existing;

  const run = (async (): Promise<QueuedRemovalResult> => {
    removingQueuedMessageIds.value = new Set([...removingQueuedMessageIds.value, key]);
    const removedPending = removePendingMessage(threadId, messageId);
    try {
      const compose = await removeQueuedMessageRequest(threadId, messageId);
      if (compose) acceptComposeReport(threadId, parseComposeReport(compose));
      return { outcome: 'removed', restoredByStop: !!compose };
    } catch (err) {
      const next = new Set(removingQueuedMessageIds.value);
      next.delete(key);
      removingQueuedMessageIds.value = next;
      restorePendingMessage(threadId, removedPending);
      const alreadyInjected = err instanceof ApiError && err.httpCode === 409;
      return { outcome: alreadyInjected ? 'already-injected' : 'failed', error: err };
    } finally {
      inFlightQueuedRemovals.delete(key);
    }
  })();
  inFlightQueuedRemovals.set(key, run);
  return run;
}

export async function removeQueuedMessage(threadId: string, messageId: string): Promise<void> {
  const { outcome, error } = await retractQueuedMessage(threadId, messageId);
  if (outcome === 'removed') return;
  reportFailedRetract(threadId, 'remove', error);
}

export async function editQueuedMessage(threadId: string, message: QueuedMessage): Promise<void> {
  const { outcome, error, restoredByStop } = await retractQueuedMessage(threadId, message.id);
  if (outcome !== 'removed') {
    reportFailedRetract(threadId, 'edit', error);
    return;
  }
  if (!restoredByStop) appendMessagesToCompose(threadId, [message]);
  focusPromptNow();
}

function reportFailedRetract(threadId: string, action: 'remove' | 'edit', error: unknown): void {
  void refreshThreadEvents(threadId);
  showToast(`Failed to ${action} queued message: ${errorDetail(error)}`, 'error');
}

function markPendingUnconfirmed(threadId: string, eventId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  const pending = thread.pendingUserMessages.find(p => p.eventId === eventId);
  if (!pending || pending.unconfirmed) return;
  pending.unconfirmed = true;
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(threadId);
}

export const STALE_EXCHANGE_FOLLOWUP_MS = 60_000;

const PENDING_CLEANUP_MAX_ATTEMPTS = 3;

export function schedulePendingCleanup(threadId: string, eventId: string, attempt = 1): void {
  setTimeout(async () => {
    const thread = threadMap.value.get(threadId);
    if (!thread || !thread.pendingUserMessages.some(p => p.eventId === eventId)) return;
    const refetchOk = await refreshThreadEvents(threadId);
    if (refetchOk) {
      removePendingMessage(threadId, eventId);
    } else if (attempt < PENDING_CLEANUP_MAX_ATTEMPTS) {
      schedulePendingCleanup(threadId, eventId, attempt + 1);
    } else {
      markPendingUnconfirmed(threadId, eventId);
      console.warn(`[Chat] pending message ${eventId} unconfirmed after ${attempt} refetches that never landed; keeping the row`);
    }
  }, PENDING_MESSAGE_SAFETY_MS);

  const existing = threadMap.value.get(threadId);
  if (existing?.meta.channel === 'claude_code') {
    setTimeout(async () => {
      const thread = threadMap.value.get(threadId);
      if (!thread || thread.meta.status !== 'running') return;
      await refreshThreadEvents(threadId);
    }, STALE_EXCHANGE_FOLLOWUP_MS);
  }
}

function addPendingMessage(
  threadId: string,
  message: string,
  eventId: string,
  imageHashes?: string[],
): string | undefined {
  const target = threadMap.value.get(threadId);
  const answersQuestion = target ? questionTheSendAnswers(target) : undefined;
  if (answersQuestion) discardUnsentAnswers(threadId, answersQuestion);
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (thread) {
    thread.pendingUserMessages.push({
      text: message,
      eventId,
      created: new Date().toISOString(),
      image_hashes: imageHashes,
      ...(answersQuestion ? { answersQuestion } : {}),
    });
    if (focusedThreadId.value === threadId) {
      followSentMessage();
      markThreadRerenderStart(threadId, { ...currentPerfBaseline(), cause: 'send' });
    }
    threadMap.value = new Map(map);
    bumpThreadEvents(threadId);
  }
  return answersQuestion;
}

export function discardUnsentAnswers(threadId: string, toolUseId: string): void {
  for (const [eventId, unsent] of unsentMessages.value) {
    if (unsent.threadId === threadId && unsent.answersQuestion === toolUseId) discardUnsentMessage(eventId);
  }
}

export { loadRepositories } from './repositoriesLoader';

export type SendOutcome = 'sent' | 'shown-as-failed' | 'dropped';

export async function sendMessage(
  message: string,
  imageHashes?: string[],
  options?: {
    useCodingAgent?: boolean;
    hookArgs?: Record<string, string>;
    context?: ChatContext | null;
    threadId?: string;
    focus?: boolean;
    modelOverride?: string;
    reasoningEffortOverride?: string;
    providerOverride?: string;
    ccModelOverride?: CodingAgentModelValue | null;
    ccReasoningEffortOverride?: CodingAgentReasoningEffort | null;
    alsoMention?: string[];
    settlement?: SendSettlement;
    threadStarted?: () => Promise<void>;
  },
): Promise<SendOutcome> {
  threadsLoaded.value = true;
  const eventId = generateUuid();
  const explicitThreadId = options?.threadId;
  const shouldFocus = options?.focus ?? true;
  const isNewThread = explicitThreadId === undefined && focusedThreadId.value === null;
  const threadId = explicitThreadId || focusedThreadId.value || eventId;
  const sendSlot = enterSendChain(threadId, knownComposeEpoch(threadId));
  const releaseImages = retainSendingImageHashes(imageHashes);
  try {
    return await buildAndPostSend({ message, imageHashes, options, eventId, threadId, isNewThread, shouldFocus, sendSlot });
  } catch (error) {
    removePendingMessage(threadId, eventId);
    forgetUnsentMessageRecord(eventId);
    throw error;
  } finally {
    sendSlot.release();
    releaseImages();
  }
}

type SendMessageOptions = NonNullable<Parameters<typeof sendMessage>[2]>;

async function buildAndPostSend({ message, imageHashes, options, eventId, threadId, isNewThread, shouldFocus, sendSlot }: {
  message: string;
  imageHashes: string[] | undefined;
  options: SendMessageOptions | undefined;
  eventId: string;
  threadId: string;
  isNewThread: boolean;
  shouldFocus: boolean;
  sendSlot: SendSlot;
}): Promise<SendOutcome> {
  if (shouldFocus) {
    setFocusedThread(threadId);
    if (isNewThread) {
      pushThreadNavState({ type: 'thread', id: threadId });
      revealThreadPane();
    }
  }

  const threadBeforeSend = threadMap.value.get(threadId);

  const map = threadMap.value;
  if (!map.has(threadId)) {
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: message.slice(0, 40),
      channel: options?.useCodingAgent ? 'claude_code' : 'chat',
      initiator: 'user',
      eventsLoaded: true,
    }));
  }
  const answersQuestion = addPendingMessage(threadId, message, eventId, imageHashes);
  if (answersQuestion) unsentPicks.clear(answersQuestion);
  const body: ChatRequestBody = {
    message,
    mode: 'human',
    model: options?.modelOverride,
    device_id: getDeviceId(),
    event_id: eventId,
    thread_id: threadId,
    ...sendSlot.position,
    compose_epoch: sendSlot.intentEpoch,
    ...(options?.context ?? {}),
  };
  if (imageHashes?.length) body.image_hashes = imageHashes;
  if (options?.hookArgs && Object.keys(options.hookArgs).length) body.hook_args = options.hookArgs;
  const addressing = sendAddressing(message, options?.alsoMention);
  if (addressing) {
    if (addressing.mentions.length > 0) body.mentions = addressing.mentions;
    if (addressing.agentMentioned) body.agent_mentioned = true;
  }
  if (!threadBeforeSend) body.new_thread = true;

  const isCcThread = threadBeforeSend
    ? threadBeforeSend.meta.channel === 'claude_code'
    : !!options?.useCodingAgent;
  if (isCcThread) {
    body.use_coding_agent = true;
    const requestedAgent = threadBeforeSend
      ? threadBeforeSend.meta.codingAgent
      : selectedCodingAgent.value;
    if (requestedAgent && requestedAgent !== 'claude-code') {
      body.coding_agent = requestedAgent;
    }
    if (!threadBeforeSend) {
      const folder = scopeToFolder(selectedScope.value);
      if (folder) body.folder = folder;
    } else if (threadBeforeSend.meta.codingAgentFolder
      && threadBeforeSend.meta.codingAgentKind === 'app') {
      body.folder = `data/apps/${threadBeforeSend.meta.codingAgentFolder.split('/').pop()}`;
    } else if (threadBeforeSend.meta.repoId) {
      body.repo_id = threadBeforeSend.meta.repoId;
    }
    if (options?.ccModelOverride) body.cc_model = options.ccModelOverride;
    if (options?.ccReasoningEffortOverride) body.reasoning_effort = options.ccReasoningEffortOverride;
  } else {
    const effort = options?.reasoningEffortOverride;
    if (effort) body.reasoning_effort = effort;
    const provider = options?.providerOverride;
    if (provider) body.provider = provider;
  }

  const settlement: SendSettlement = options?.settlement ?? { kind: body.new_thread ? 'raw-new' : 'follow-up' };
  const sentAt = new Date().toISOString();
  persistSendingMessage({ eventId, threadId, body, settlement, sentAt, answersQuestion });
  if (options?.threadStarted) await options.threadStarted();

  const url = panelUrl.value;
  const fallbackTitle = panelTitle.value;
  if (url && !body.use_coding_agent) {
    let extractedTitle: string | undefined;
    let extractedContent = '';
    if (isTauri()) {
      try {
        const res = await getWebviewContent();
        extractedTitle = res.title || undefined;
        extractedContent = res.content.trim() ? res.content : '';
      } catch {  }
    }
    body.url_context = {
      url,
      title: extractedTitle || fallbackTitle || undefined,
      content: extractedContent,
    };
  }
  persistSendingMessage({ eventId, threadId, body, settlement, sentAt, answersQuestion });
  return postSend({ threadId, eventId, body, sendSlot, failedRetries: 0, settlement });
}

async function postSend(send: {
  threadId: string;
  eventId: string;
  body: ChatRequestBody;
  sendSlot: SendSlot;
  failedRetries: number;
  settlement: SendSettlement;
  preserveWireIntent?: boolean;
}): Promise<SendOutcome> {
  const { threadId, eventId, body, sendSlot } = send;
  const answersQuestion = threadMap.value.get(threadId)?.pendingUserMessages
    .find(p => p.eventId === eventId)?.answersQuestion;
  const answerToMatch = answersQuestion ? { toolUseId: answersQuestion, text: body.message } : false;
  const accepted = (): SendOutcome => {
    forgetUnsentMessageRecord(eventId);
    schedulePendingCleanup(threadId, eventId);
    probeStreamAfterSend(threadId, () => engineRecordedMessage(threadId, eventId, answerToMatch));
    void retireWelcomeAfterUse(threadMap.value.values());
    return 'sent';
  };
  if (sendSlot.waitForTurn) await sendSlot.waitForTurn;
  if (!send.preserveWireIntent) {
    body.compose_epoch = sendSlot.epochForPost();
    persistSendingMessage({ eventId, threadId, body, settlement: send.settlement,
      sentAt: new Date().toISOString(), answersQuestion });
  }
  const result = await withQuietRetries(() => send.settlement.kind === 'first-send' ? submitChat(body, true) : submitChat(body), {
    path: 'message',
    landed: () => engineRecordedMessage(threadId, eventId, answerToMatch),
  });
  switch (result.kind) {
    case 'done':
      if (!send.preserveWireIntent) sendSlot.accepted(result.value.compose?.compose_epoch ?? undefined);
      if (result.value.compose) acceptComposeReport(threadId, parseComposeReport(result.value.compose));
      return accepted();
    case 'landed':
      return accepted();
    case 'gave-up':
      showUnsentExchange({ ...send, failure: sendFailureOf(result) });
      markUnsentMessageRecordUnsent(eventId, send.failedRetries);
      return 'shown-as-failed';
    case 'refused':
      const refusal = inputRefusalOf(result.error);
      if (refusal && (!body.new_thread || refusal.reason === 'hook_needs_input' || refusal.reason === 'hook_input_overtaken')) {
        showUnsentExchange({ ...send, refusal, copy: `Not sent: ${refusal.message}` });
        markUnsentMessageRecordUnsent(eventId, send.failedRetries, refusal);
        return 'shown-as-failed';
      }
      if (body.new_thread) {
        dropNeverMadeThread(threadId);
      } else {
        removePendingMessage(threadId, eventId);
      }
      forgetUnsentMessageRecord(eventId);
      showToast(`Failed to send message: ${errorDetail(result.error)}`, 'error');
      return 'dropped';
  }
}

function dropNeverMadeThread(threadId: string): void {
  const next = new Map(threadMap.value);
  next.delete(threadId);
  threadMap.value = next;
  forgetThreadEventsFailures(threadId);
  removeThreadNavEntries(threadId);
  if (focusedThreadId.value === threadId) setFocusedThread(null);
}

export function engineRecordedMessage(
  threadId: string,
  eventId: string,
  answer?: { toolUseId: string; text: string } | false,
): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread) return false;
  for (const [seq, stored] of thread.events) {
    if (seq <= 0) continue;
    if (stored._eventId === eventId) return true;
    if (answer && stored.type === 'UserQuestionAnswered' && stored.tool_use_id === answer.toolUseId
      && stored.answer.kind === 'FreeText' && stored.answer.text === answer.text) return true;
  }
  return false;
}

export function unsentMessageCopy(failedRetries: number): string {
  return failedRetries === 0
    ? 'Lucidos did not answer, so this message was not sent.'
    : 'Lucidos still did not answer. Try again in a moment.';
}

export const UNSENT_AFTER_RELOAD_COPY = 'The page reloaded before Lucidos answered, so this message may not have been sent.';

let nextUnsentSeq = -1;

function showUnsentExchange(send: {
  threadId: string;
  eventId: string;
  body: ChatRequestBody;
  failedRetries: number;
  settlement: SendSettlement;
  sentAt?: string;
  copy?: string;
  answersQuestion?: string;
  failure?: SendFailure;
  refusal?: UnsentMessageRecord['refusal'];
}): void {
  const { threadId, eventId, body } = send;
  const failedSeq = nextUnsentSeq;
  const messageSeq = failedSeq - 1;
  nextUnsentSeq -= 2;
  const now = send.sentAt ?? new Date().toISOString();
  const answersQuestion = threadMap.value.get(threadId)?.pendingUserMessages
    .find(p => p.eventId === eventId)?.answersQuestion ?? send.answersQuestion;
  handleEvent(threadMap.value, threadId, messageSeq, {
    type: 'MessageReceived',
    text: body.message,
    user_image_hashes: body.image_hashes,
    _unsent: true,
    ...(answersQuestion ? { _answersQuestion: answersQuestion } : {}),
  } as StoredEvent, now, eventId);
  handleEvent(threadMap.value, threadId, failedSeq, {
    type: 'ResponseFailed',
    error: send.copy ?? unsentMessageCopy(send.failedRetries),
    _unsent: true,
  } as StoredEvent, now);
  const thread = threadMap.value.get(threadId);
  if (thread) (thread.unsentMessageSeqs ??= new Map()).set(eventId, messageSeq);
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(threadId);
  recordUnsentMessage(eventId, {
    threadId,
    body,
    failedRetries: send.failedRetries,
    settlement: send.settlement,
    ...(answersQuestion ? { answersQuestion } : {}),
    ...(send.failure ? { failure: send.failure } : {}),
    ...(send.refusal ? { refusal: send.refusal } : {}),
  });
}

export function showRestoredUnsentMessage(record: UnsentMessageRecord): void {
  showUnsentExchange({
    ...record,
    copy: record.phase === 'sending' ? UNSENT_AFTER_RELOAD_COPY
      : record.refusal ? `Not sent: ${record.refusal.message}` : unsentMessageCopy(record.failedRetries),
  });
}

export async function retryUnsentMessage(
  eventId: string,
  hookAnswer?: { kind: 'pick' | 'create'; value: string },
): Promise<SendOutcome | null> {
  const original = unsentMessages.peek().get(eventId);
  if (!original || original.refusal?.reason === 'hook_input_overtaken') return null;
  const hookInput = original.refusal?.reason === 'hook_needs_input' ? original.refusal.input : undefined;
  if (Boolean(hookInput) !== Boolean(hookAnswer)) return null;
  if (hookAnswer?.kind === 'create' && !hookInput?.creation) return null;
  const thread = threadMap.value.get(original.threadId);
  if (!thread) return null;
  const releaseImages = retainSendingImageHashes(original.body.image_hashes);
  try {
    const unsent = takeUnsentMessage(eventId);
    if (!unsent) return null;
    const { threadId } = unsent;
    const preserveWireIntent = !unsent.refusal;
    const sendSlot = enterSendChain(threadId, preserveWireIntent ? unsent.body.compose_epoch : knownComposeEpoch(threadId));
    const body = preserveWireIntent ? { ...unsent.body }
      : { ...unsent.body, ...sendSlot.position, compose_epoch: sendSlot.intentEpoch };
    if (hookInput && hookAnswer) {
      const creationKey = hookInput.creation?.key;
      const args = { ...body.hook_args };
      if (creationKey) delete args[creationKey];
      delete args[hookInput.argument];
      const selectedKey = hookAnswer.kind === 'create' && creationKey ? creationKey : hookInput.argument;
      args[selectedKey] = hookAnswer.value;
      body.hook_args = args;
    }
    persistSendingMessage({ eventId, threadId, body, settlement: original.settlement,
      sentAt: new Date().toISOString(), answersQuestion: original.answersQuestion });
    let outcome: SendOutcome;
    try {
      retireUnsentExchange(thread, eventId);
      addPendingMessage(threadId, body.message, eventId, body.image_hashes);
      outcome = await postSend({
        threadId,
        eventId,
        body,
        sendSlot,
        preserveWireIntent,
        failedRetries: unsent.failedRetries + 1,
        settlement: unsent.settlement,
      });
    } finally {
      sendSlot.release();
    }
    if (outcome === 'sent') settleAcceptedSend(threadId, unsent.settlement);
    if (outcome === 'dropped') restoreRefusedSend(threadId, body, unsent.settlement);
    return outcome;
  } finally {
    releaseImages();
  }
}

export async function cancelUnsentHookInput(eventId: string): Promise<void> {
  const held = unsentMessages.peek().get(eventId);
  if (held?.refusal?.reason !== 'hook_needs_input' && held?.refusal?.reason !== 'hook_input_overtaken') return;
  const settlement = held.refusal.reason === 'hook_input_overtaken' ? { kind: 'follow-up' } as const : held.settlement;
  await cancelHookInput(eventId);
  discardUnsentMessage(eventId);
  restoreRefusedSend(held.threadId, held.body, settlement);
}

export function discardUnsentMessage(eventId: string): void {
  const unsent = endUnsentMessage(eventId);
  if (!unsent) return;
  const thread = threadMap.value.get(unsent.threadId);
  if (!thread || !retireUnsentExchange(thread, eventId)) return;
  const nothingElseSent = unsent.refusal?.reason !== 'hook_input_overtaken'
    && thread.meta.messageCount === 0 && thread.events.size === 0 && thread.pendingUserMessages.length === 0;
  if (unsent.settlement.kind === 'raw-new' && nothingElseSent) {
    dropNeverMadeThread(unsent.threadId);
    return;
  }
  if (unsent.settlement.kind === 'first-send' && nothingElseSent) reopenEmptyDraft(unsent.threadId);
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(unsent.threadId);
}

export type CancelOutcome = 'canceled' | 'noop' | 'failed';

export { appendMessagesToCompose } from './compose';

export async function cancelCurrentExchange(threadId?: string): Promise<CancelOutcome> {
  const tid = threadId ?? focusedThreadId.value ?? undefined;
  try {
    const thread = tid ? threadMap.value.get(tid) : undefined;
    if (thread?.meta.channel === 'claude_code') {
      const canceled = await stopClaudeCode(undefined, tid);
      return canceled ? 'canceled' : 'noop';
    }
    const result = await cancelChat(tid);
    for (const message of result.taken_back_messages) {
      removePendingMessage(message.thread_id, message.id);
      acceptComposeReport(message.thread_id, parseComposeReport(message.compose));
    }
    if (result.failed_take_back_ids.length > 0) {
      const count = result.failed_take_back_ids.length;
      showToast(
        `Stopped, but ${count} queued message${count > 1 ? 's' : ''} could not be retracted `
        + `in ${tid ? formatThreadLabel(tid) : 'this workspace'}. They may still run.`,
        'error', { key: `queued-retract-${tid ?? 'all'}` },
      );
    }
    return result.canceled ? 'canceled' : 'noop';
  } catch (err) {
    showToast(`Failed to cancel: ${errorDetail(err)}`, 'error');
    return 'failed';
  }
}

export async function handleCancelExchange(threadId: string): Promise<void> {
  const next = new Set(cancelingThreadIds.value);
  next.add(threadId);
  cancelingThreadIds.value = next;
  const outcome = await cancelCurrentExchange(threadId);
  if (outcome === 'canceled') return;
  const rollback = new Set(cancelingThreadIds.value);
  rollback.delete(threadId);
  cancelingThreadIds.value = rollback;
  setCanceledQuestion(threadId, undefined);
  setCanceledWhileAwaiting(threadId, false);
  if (outcome === 'noop') {
    void refreshThreadEvents(threadId);
  }
}
