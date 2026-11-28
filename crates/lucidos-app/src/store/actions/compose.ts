import { savePreference } from './preferences';
import type { CodingAgent } from '../../api/types';
import { restoreAdmissionReady } from '../../utils/restoreAdmission';
import { requestGenerationGuard } from '../../api/client/_core';

import { effect } from '@preact/signals';
import { retainSendingImageHashes } from '../pendingUploadRecords';
import { threadMap, focusedThreadId, inputMode, showToast, removeToast, setFocusedThread, selectedScope, selectedCodingAgent, preferences, repositories, type Scope } from '../store';
import { loadedOr } from '../types';
import { generateUuid } from '../../utils/uuid';
import {
  composeSelections,
  pendingComposeSelection,
  getComposeSelectionOverride,
  patchComposeSelection,
  clearComposeSelection,
  seedComposeSelection,
  takePendingComposeSelection,
  resolveUser,
  resolveScope,
  resolveCodingAgent,
  applyCodingAgentDefault,
  resolveModel,
  resolveReasoningEffort,
  resolveProvider,
  resolveCcModel,
  resolveCcReasoningEffort,
  type ComposeSelectionOverride,
} from '../composeSelections';
import { scopeRepoName, threadScope, type ComposeDestination } from '../composeDestination';
import { makeOptimisticThreadState, type ThreadMeta } from '../thread-events';
import { clearDraft, composeDrafts, draftIsEmpty, getDraft, patchDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { API, ApiError, ensureThreadStarted, putComposeOnThread, deleteThread, isTransientFetchError, type ComposePutResult } from '../../api/client';
import {
  acceptComposeReport,
  acknowledgeComposeEdits,
  composeEditSeq,
  forgetComposeSync,
  knownComposeEpoch,
  noteLocalComposeEdit,
  sameHashes,
  serverComposeOf,
  _resetComposeSyncForTesting,
  type ParsedComposeReport,
} from '../composeSync';
import { errorDetail } from '../../utils/errorDetail';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { createFailureCounter } from '../../utils/failureCounter';
import { sendMessage } from './chatSend';
import { refreshRepositories } from './repositoriesLoader';
import { forgetThreadEventsFailures } from './thread-loading';
import { changeRows, focusThread, unfocusThread, versionsBeyondCurrent } from './threads';
import { abandonRowIntent, acknowledgeRowIntent, holdRowIntent } from '../rowIntents';
import { homeThreadId } from './homeThread';
import type { ChatContext } from './chatContext';
import { revealThreadPane } from './pane';
import { markHashesAsSent } from '../../components/chat/pastedImages';
import { requestPromptCaretToEnd } from '../../components/chat/promptValueSync';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';

export type ComposeMode = 'lucidos' | 'claude_code';

function scopeEquals(a: Scope, b: Scope): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'external' && b.kind === 'external') return a.repoId === b.repoId;
  if (a.kind === 'app' && b.kind === 'app') return a.appId === b.appId;
  return true;
}

export function applyDestination(threadId: string | null, d: ComposeDestination): void {
  const mode: ComposeMode = d.kind === 'coding' ? 'claude_code' : 'lucidos';
  const user = d.kind === 'user' ? d.userId : null;
  const modeType = mode === 'claude_code' ? 'coding_agent' : 'do';
  if (inputMode.value.type !== modeType) {
    inputMode.value = mode === 'claude_code' ? { type: 'coding_agent' } : { type: 'do' };
  }
  const composing = !threadId || threadMap.value.get(threadId)?.meta.state === 'composing';
  if (!composing) return;
  if (d.kind === 'coding') {
    if (!scopeEquals(selectedScope.value, d.scope)) {
      selectedScope.value = d.scope;
    }
    const current = getComposeSelectionOverride(threadId).scope;
    if (!current || !scopeEquals(current, d.scope)) {
      patchComposeSelection(threadId, { scope: d.scope });
      if (threadId) {
        noteLocalComposeEdit(threadId);
        schedulePush(threadId);
      }
    }
  }
  if (resolveUser(threadId) !== user) {
    patchComposeSelection(threadId, { user });
    if (threadId) {
      noteLocalComposeEdit(threadId);
      schedulePush(threadId);
    }
  }
  if (threadId && getDraft(threadId).mode !== mode) {
    updateCompose(threadId, { mode });
  }
}

const DEFAULT_SCOPE: Scope = { kind: 'lucidos' };

const isOnRepo = (scope: Scope | undefined, repoIds: ReadonlySet<string>): boolean =>
  scope?.kind === 'external' && repoIds.has(scope.repoId);

function unregisteredTargetRepoIds(registered: ReadonlySet<string>): Set<string> {
  const ids = new Set<string>();
  const note = (scope: Scope | undefined): void => {
    if (scope?.kind === 'external' && !registered.has(scope.repoId)) ids.add(scope.repoId);
  };
  note(selectedScope.value);
  note(pendingComposeSelection.value.scope);
  for (const [id, override] of composeSelections.value) {
    if (threadMap.value.get(id)?.meta.state === 'composing') note(override.scope);
  }
  return ids;
}

export function dropRepoTargets(goneRepoIds: ReadonlySet<string>): void {
  for (const [id, override] of composeSelections.value) {
    if (!isOnRepo(override.scope, goneRepoIds) || threadMap.value.get(id)?.meta.state !== 'composing') continue;
    updateComposeSelection(id, { scope: DEFAULT_SCOPE });
    if (getDraft(id).mode !== 'lucidos') updateCompose(id, { mode: 'lucidos' });
  }
  if (isOnRepo(resolveScope(null), goneRepoIds) && inputMode.value.type === 'coding_agent') {
    inputMode.value = { type: 'do' };
  }
  if (isOnRepo(selectedScope.value, goneRepoIds)) selectedScope.value = DEFAULT_SCOPE;
  if (isOnRepo(pendingComposeSelection.value.scope, goneRepoIds)) {
    patchComposeSelection(null, { scope: DEFAULT_SCOPE });
  }
}

export function installUnregisteredRepoTargetReset(
  refetch: () => Promise<void> = refreshRepositories,
): () => void {
  let scheduled = false;
  let confirming = false;
  const registeredIds = (): Set<string> | null => {
    const repos = repositories.peek();
    return repos.status === 'loaded' ? new Set(repos.data.map(r => r.id)) : null;
  };
  const confirmAndDrop = async (): Promise<void> => {
    const cached = registeredIds();
    if (!cached || unregisteredTargetRepoIds(cached).size === 0) return;
    confirming = true;
    try {
      await refetch();
    } finally {
      confirming = false;
    }
    const fresh = registeredIds();
    const gone = fresh ? unregisteredTargetRepoIds(fresh) : new Set<string>();
    if (gone.size > 0) dropRepoTargets(gone);
  };
  return effect(() => {
    const repos = repositories.value;
    void composeSelections.value;
    void pendingComposeSelection.value;
    void selectedScope.value;
    void threadMap.value;
    if (repos.status !== 'loaded' || scheduled || confirming) return;
    scheduled = true;
    queueMicrotask(() => {
      scheduled = false;
      void confirmAndDrop();
    });
  });
}

interface ComposePatch {
  text?: string;
  image_hashes?: string[];
  mode?: ComposeMode;
}

export function currentComposeMode(): ComposeMode {
  return inputMode.value.type === 'coding_agent' ? 'claude_code' : 'lucidos';
}

const DEBOUNCE_MS = 250;

const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

export interface ServerDraft {
  text: string;
  imageHashes: string[];
}

export function updateCompose(threadId: string, patch: ComposePatch): void {
  noteLocalComposeEdit(threadId);
  patchDraft(threadId, patch);
  const thread = threadMap.value.get(threadId);
  const clearedContent = patch.text !== undefined || patch.image_hashes !== undefined;
  if (clearedContent && thread?.meta.state === 'composing' && draftIsEmpty(getDraft(threadId))) {
    void discardCompose(threadId);
    return;
  }
  schedulePush(threadId);
}

export function updateComposeSelection(
  threadId: string | null,
  patch: ComposeSelectionOverride,
): void {
  patchComposeSelection(threadId, patch);
  if (threadId) {
    noteLocalComposeEdit(threadId);
    schedulePush(threadId);
  }
}

export function selectComposeCodingAgent(threadId: string | null, agent: CodingAgent): Promise<void> {
  updateComposeSelection(threadId, { codingAgent: agent });
  return savePreference('coding_agent_default', agent, () => applyCodingAgentDefault(agent));
}

function mutateThreadMeta(threadId: string, patch: Partial<ThreadMeta>): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  const next = new Map(threadMap.value);
  next.set(threadId, { ...thread, meta: { ...thread.meta, ...patch } });
  threadMap.value = next;
}


const undeliveredComposeDrafts = new Set<string>();

const COMPOSE_REJECTED_TOAST = 'compose-sync-rejected';
const COMPOSE_UNREACHABLE_TOAST = 'compose-sync-unreachable';

const composePushFailures = createFailureCounter(3, () => {
  showToast(
    'Drafts are not reaching the engine. They are kept on this device and re-sent automatically.',
    'error',
    { key: COMPOSE_UNREACHABLE_TOAST },
  );
});

function settleComposeDelivered(threadId: string): void {
  composePushFailures.recordSuccess();
  undeliveredComposeDrafts.delete(threadId);
  if (undeliveredComposeDrafts.size === 0) removeToast(COMPOSE_UNREACHABLE_TOAST);
}

function handleComposePushFailure(threadId: string, err: unknown): void {
  if (isTransientFetchError(err)) {
    undeliveredComposeDrafts.add(threadId);
    composePushFailures.recordFailure();
    return;
  }
  settleComposeDelivered(threadId);
  showToast(`Compose sync failed: ${errorDetail(err)}`, 'error', {
    key: COMPOSE_REJECTED_TOAST,
  });
}

function dropUndeliveredComposeDraft(threadId: string): void {
  if (!undeliveredComposeDrafts.delete(threadId)) return;
  if (undeliveredComposeDrafts.size === 0) removeToast(COMPOSE_UNREACHABLE_TOAST);
}

export function flushUndeliveredComposeDrafts(): void {
  if (!restoreAdmissionReady()) return;
  if (undeliveredComposeDrafts.size === 0) return;
  for (const threadId of [...undeliveredComposeDrafts]) {
    const thread = threadMap.peek().get(threadId);
    if (!thread || thread.meta.state === 'discarded') {
      dropUndeliveredComposeDraft(threadId);
      continue;
    }
    schedulePush(threadId);
  }
}

export function _undeliveredComposeDraftsForTesting(): string[] {
  return [...undeliveredComposeDrafts].sort();
}

export function _resetUndeliveredComposeDraftsForTesting(): void {
  undeliveredComposeDrafts.clear();
  threadStarts.clear();
  runningComposePushes.clear();
  owedComposePushes.clear();
  _resetComposeSyncForTesting();
  composePushFailures.recordSuccess();
}

const runningComposePushes = new Set<string>();

const owedComposePushes = new Set<string>();

const MAX_STALE_EPOCH_RETRIES = 2;

function schedulePush(threadId: string): void {
  const existing = pendingTimers.get(threadId);
  if (existing) clearTimeout(existing);
  const t = setTimeout(() => {
    pendingTimers.delete(threadId);
    runComposePushes(threadId);
  }, DEBOUNCE_MS);
  pendingTimers.set(threadId, t);
}

function runComposePushes(threadId: string): void {
  owedComposePushes.add(threadId);
  if (!restoreAdmissionReady()) { undeliveredComposeDrafts.add(threadId); return; }
  if (runningComposePushes.has(threadId)) return;
  runningComposePushes.add(threadId);
  void (async () => {
    try {
      while (owedComposePushes.delete(threadId)) {
        await pushNow(threadId);
      }
    } catch (err) {
      console.error('[compose] push outcome handling threw', err);
    } finally {
      runningComposePushes.delete(threadId);
      owedComposePushes.delete(threadId);
    }
  })();
}

function imageHashesUnchanged(threadId: string, current: string[]): boolean {
  const held = serverComposeOf(threadId)?.imageHashes;
  return held !== undefined && sameHashes(held, current);
}

function composeModeForWrite(
  threadId: string,
  state: ThreadMeta['state'],
  draft: ComposeDraft,
): ComposeMode | null {
  if (state !== 'composing') return null;
  const held = serverComposeOf(threadId);
  if (held && held.mode === draft.mode) return null;
  return draft.mode;
}

async function pushNow(threadId: string, staleRetries = 0): Promise<void> {
  try {
    await awaitThreadStarted(threadId);
  } catch {
    return;
  }
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.state === 'discarded') return;
  const seq = composeEditSeq(threadId);
  const draft = getDraft(threadId);
  const wireHashes: string[] | null = imageHashesUnchanged(threadId, draft.image_hashes)
    ? null
    : [...draft.image_hashes];
  const selectionOverride = getComposeSelectionOverride(threadId);
  const selectionForPut: ComposeSelectionOverride | undefined =
    Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
  const wireMode = composeModeForWrite(threadId, thread.meta.state, draft);
  let result: ComposePutResult;
  try {
    result = await putComposeOnThread(
      threadId,
      draft.text,
      wireHashes,
      wireMode,
      selectionForPut,
      knownComposeEpoch(threadId),
    );
  } catch (err) {
    handleComposePushFailure(threadId, err);
    return;
  }
  if (result.status === 'stale') {
    acceptComposeReport(threadId, result.report);
    if (staleRetries < MAX_STALE_EPOCH_RETRIES) {
      await pushNow(threadId, staleRetries + 1);
      return;
    }
    undeliveredComposeDrafts.add(threadId);
    return;
  }
  const held = serverComposeOf(threadId);
  const report: ParsedComposeReport = result.report ?? {
    revision: null,
    epoch: null,
    text: draft.text,
    imageHashes: [...draft.image_hashes],
    mode: wireMode ?? held?.mode ?? null,
    selection: selectionForPut ?? held?.selection ?? null,
  };
  acknowledgeComposeEdits(threadId, seq, report);
  if (composeEditSeq(threadId) > seq) owedComposePushes.add(threadId);
  settleComposeDelivered(threadId);
}

export function forgetComposeState(threadId: string): void {
  cancelPendingPush(threadId);
  clearDraft(threadId);
  clearComposeSelection(threadId);
  forgetComposeSync(threadId);
}

function cancelPendingPush(threadId: string): void {
  const t = pendingTimers.get(threadId);
  if (t) {
    clearTimeout(t);
    pendingTimers.delete(threadId);
  }
  owedComposePushes.delete(threadId);
  dropUndeliveredComposeDraft(threadId);
}

export async function startComposeIfNeeded(threadId: string, mode: ComposeMode): Promise<void> {
  if (threadMap.value.has(threadId)) return;
  const next = new Map(threadMap.value);
  next.set(threadId, makeOptimisticThreadState({
    id: threadId,
    title: '',
    channel: mode === 'claude_code' ? 'claude_code' : 'chat',
    initiator: 'user',
    eventsLoaded: true,
    state: 'composing',
    status: 'idle',
  }));
  threadMap.value = next;
  setDraft(threadId, { text: '', image_hashes: [], mode });
  await startThreadOnEngine(threadId, mode);
}

type ThreadStart =
  | { kind: 'in-flight'; promise: Promise<void> }
  | { kind: 'owed'; mode: ComposeMode };

const threadStarts = new Map<string, ThreadStart>();

function startThreadOnEngine(threadId: string, mode: ComposeMode): Promise<void> {
  const current = threadStarts.get(threadId);
  if (current?.kind === 'in-flight') return current.promise;
  const promise = (async () => {
    let started: ParsedComposeReport | null;
    try {
      started = await ensureThreadStarted(threadId, mode);
    } catch (err) {
      const thread = threadMap.value.get(threadId);
      const live = thread !== undefined && thread.meta.state !== 'discarded';
      if (live && isTransientFetchError(err)) {
        threadStarts.set(threadId, { kind: 'owed', mode });
        undeliveredComposeDrafts.add(threadId);
        composePushFailures.recordFailure();
        throw err;
      }
      threadStarts.delete(threadId);
      rollbackOptimistic(threadId);
      if (live) reportRefusedThreadStart(threadId, err);
      throw err;
    }
    threadStarts.delete(threadId);
    acceptComposeReport(threadId, started ?? {
      revision: null, epoch: null, text: '', imageHashes: [], mode, selection: null,
    });
  })();
  threadStarts.set(threadId, { kind: 'in-flight', promise });
  return promise;
}

function reportRefusedThreadStart(threadId: string, err: unknown): void {
  removeThreadNavEntries(threadId);
  if (focusedThreadId.value === threadId) setFocusedThread(null);
  showToast(`Failed to start compose: ${errorDetail(err)}`, 'error');
}

export function isThreadStartPending(threadId: string): boolean {
  return threadStarts.has(threadId);
}

function isThreadStartOwed(threadId: string): boolean {
  return threadStarts.get(threadId)?.kind === 'owed';
}

function rollbackOptimistic(threadId: string): void {
  if (!threadMap.value.has(threadId)) return;
  const next = new Map(threadMap.value);
  next.delete(threadId);
  threadMap.value = next;
  forgetThreadEventsFailures(threadId);
  clearDraft(threadId);
  forgetComposeSync(threadId);
  clearComposeSelection(threadId);
}

export async function awaitThreadStarted(threadId: string): Promise<void> {
  const start = threadStarts.get(threadId);
  if (!start) return;
  if (start.kind === 'in-flight') return start.promise;
  return startThreadOnEngine(threadId, start.mode);
}

export function ensureFocusedComposeThread(): string {
  let id = focusedThreadId.value;
  if (id) return id;
  id = generateUuid();
  setFocusedThread(id);
  seedComposeSelection(id, {
    scope: selectedScope.value,
    ...(preferences.value.status === 'loaded' ? { codingAgent: selectedCodingAgent.value } : {}),
    ...takePendingComposeSelection(),
  });
  pushThreadNavState({ type: 'thread', id });
  startComposeIfNeeded(id, currentComposeMode()).catch(() => {
  });
  noteLocalComposeEdit(id);
  schedulePush(id);
  return id;
}

export function prefillCompose(text: string): string {
  const threadId = ensureFocusedComposeThread();
  updateCompose(threadId, { text, image_hashes: [] });
  return threadId;
}

function dropNonComposingFocus(): void {
  const id = focusedThreadId.value;
  if (!id) return;
  if (threadMap.value.get(id)?.meta.state === 'composing') return;
  unfocusThread();
}

export function seedSuggestion(text: string): string {
  stepOffDraftInProgress();
  applyDestination(focusedThreadId.value, { kind: 'lucidos-agent' });
  const threadId = prefillCompose(text);
  requestPromptCaretToEnd();
  return threadId;
}

export function composeInFreshDraft(text: string, imageHashes: string[]): void {
  stepOffDraftInProgress();
  const threadId = prefillCompose(text);
  if (imageHashes.length > 0) updateCompose(threadId, { image_hashes: imageHashes });
  requestPromptCaretToEnd();
}

export function startFreshDraft(source: ThreadMeta): void {
  if (focusedThreadId.value !== source.id) return;
  const { text, image_hashes: imageHashes } = getDraft(source.id);
  const carried = text.trim() !== '' || imageHashes.length > 0;
  if (carried) updateCompose(source.id, { text: '', image_hashes: [] });
  unfocusThread();
  const scope = threadScope(source);
  if (scope) applyDestination(null, { kind: 'coding', scope });
  updateComposeSelection(null, { codingAgent: source.codingAgent ?? 'claude-code' });
  if (!carried) return;
  const threadId = prefillCompose(text);
  if (imageHashes.length > 0) updateCompose(threadId, { image_hashes: imageHashes });
  requestPromptCaretToEnd();
}

function stepOffDraftInProgress(): void {
  dropNonComposingFocus();
  const existingId = focusedThreadId.value;
  if (existingId && !draftIsEmpty(getDraft(existingId))) unfocusThread();
}

export const SETUP_INTERVIEW_PROMPT =
  'Help me get the most out of Lucidos: interview me about my work and my life, '
  + 'then build me the apps and automations that fit, here in my workspace.';

export async function startSetupInterview(): Promise<boolean> {
  const home = homeThreadId.value;
  if (!home) return sendSeededPrompt(SETUP_INTERVIEW_PROMPT, 'start the setup interview');
  focusThread(home);
  try {
    return (await sendMessage(SETUP_INTERVIEW_PROMPT, undefined, { threadId: home })) === 'sent';
  } catch (err) {
    showToast(`Failed to start the setup interview: ${errorDetail(err)}`, 'error');
    return false;
  }
}

export async function sendSeededPrompt(text: string, what: string): Promise<boolean> {
  const threadId = seedSuggestion(text);
  revealThreadPane();
  try {
    return await sendCompose(threadId, { focus: true });
  } catch (err) {
    showToast(`Failed to ${what}: ${errorDetail(err)}`, 'error');
    return false;
  }
}

export async function discardCompose(threadId: string): Promise<void> {
  if (focusedThreadId.value === threadId) setFocusedThread(null);
  const restoreDraft = snapshotDraft(threadId);
  const restoreSelection: ComposeSelectionOverride = { ...getComposeSelectionOverride(threadId) };
  changeRows([threadId], (meta) => {
    holdRowIntent(meta, 'discard', { state: 'discarded' });
    return true;
  });
  noteLocalComposeEdit(threadId);
  cancelPendingPush(threadId);
  clearDraft(threadId);
  clearComposeSelection(threadId);
  removeThreadNavEntries(threadId);
  if (isThreadStartOwed(threadId)) {
    threadStarts.delete(threadId);
    changeRows([threadId], (meta) => abandonRowIntent(meta, 'discard'));
    rollbackOptimistic(threadId);
    return;
  }
  const legacyVersion = versionsBeyondCurrent([threadId]).get(threadId);
  const settleDiscard = (version: number | undefined) => changeRows([threadId], (meta) => version === undefined
    ? abandonRowIntent(meta, 'discard')
    : acknowledgeRowIntent(meta, 'discard', version));
  try {
    settleDiscard((await deleteThread(threadId)) ?? legacyVersion);
    forgetComposeState(threadId);
  } catch (err) {
    if (isAlreadyGone(err)) {
      settleDiscard(legacyVersion);
      forgetComposeState(threadId);
      return;
    }
    changeRows([threadId], (meta) => abandonRowIntent(meta, 'discard'));
    noteLocalComposeEdit(threadId);
    if (restoreDraft) setDraft(threadId, restoreDraft);
    seedComposeSelection(threadId, restoreSelection);
    schedulePush(threadId);
    showToast(`Discard failed: ${errorDetail(err)}`, 'error');
  }
}

function snapshotDraft(threadId: string): ComposeDraft | undefined {
  const draft = composeDrafts.value.get(threadId);
  return draft && { ...draft, image_hashes: [...draft.image_hashes] };
}

function isAlreadyGone(err: unknown): boolean {
  return err instanceof ApiError && (err.httpCode === 404 || err.httpCode === 410);
}

function pushClearedComposeAfterSend(threadId: string): void {
  schedulePush(threadId);
}

export function reopenEmptyDraft(threadId: string): void {
  mutateThreadMeta(threadId, { state: 'composing' });
}

export function resumeUnsentFirstSend(threadId: string, engineDraftAtSend: ServerDraft | undefined): void {
  if (threadMap.value.get(threadId)?.meta.state === 'composing') mutateThreadMeta(threadId, { state: 'active' });
  const onEngine = serverComposeOf(threadId);
  const local = getDraft(threadId);
  const stale = engineDraftAtSend !== undefined && onEngine !== undefined
    && sameServerDraft(onEngine, engineDraftAtSend)
    && sameServerDraft(onEngine, { text: local.text, imageHashes: local.image_hashes });
  if (stale && !draftIsEmpty(local)) updateCompose(threadId, { text: '', image_hashes: [] });
}

function sameServerDraft(a: ServerDraft, b: ServerDraft): boolean {
  return a.text === b.text && sameHashes(a.imageHashes, b.imageHashes);
}

export function settleAcceptedFirstSend(threadId: string): void {
  clearComposeSelection(threadId);
  pushClearedComposeAfterSend(threadId);
}

export function rollBackFirstSend(
  threadId: string,
  sent: { text: string; imageHashes: string[]; mode: ComposeMode | null },
): void {
  const thread = threadMap.value.get(threadId);
  if (thread && thread.meta.messageCount > 0) {
    appendMessagesToCompose(threadId, [sent]);
    return;
  }
  mutateThreadMeta(threadId, { state: 'composing' });
  const current = getDraft(threadId);
  const typedMeanwhile = current.text.trim().length > 0;
  const restore: Partial<ComposeDraft> = {};
  if (!typedMeanwhile) restore.text = sent.text;
  else if (sent.text.length > 0) restore.text = `${current.text}\n\n${sent.text}`;
  const missingImages = sent.imageHashes.filter(h => !current.image_hashes.includes(h));
  if (missingImages.length > 0) restore.image_hashes = [...current.image_hashes, ...missingImages];
  if (current.mode === null) restore.mode = sent.mode;
  if (Object.keys(restore).length === 0) return;
  patchDraft(threadId, restore);
  if (current.text.length > 0 && restore.text !== undefined && !typedMeanwhile) requestPromptCaretToEnd();
  noteLocalComposeEdit(threadId);
  schedulePush(threadId);
}

export async function sendCompose(
  threadId: string,
  opts: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<boolean> {
  const thread = threadMap.value.get(threadId);
  if (!thread) {
    showToast('Could not send: that draft is no longer open.', 'error');
    return false;
  }
  const draft = getDraft(threadId);
  const text = draft.text;
  const wireHashes = draft.image_hashes;
  const mode = draft.mode;
  const onEngineAtSend = serverComposeOf(threadId);
  const engineDraftAtSend: ServerDraft | undefined = onEngineAtSend
    && { text: onEngineAtSend.text, imageHashes: [...onEngineAtSend.imageHashes] };
  if (!text.trim() && wireHashes.length === 0) {
    showToast('Could not send: the saved draft was empty.', 'error');
    return false;
  }

  cancelPendingPush(threadId);
  const scope = resolveScope(threadId);
  const boundRepoId = opts.useCodingAgent && scope.kind === 'external' ? scope.repoId : undefined;
  const boundCodingAgentKind = opts.useCodingAgent ? scope.kind : undefined;
  const boundCodingAgentFolder = opts.useCodingAgent && scope.kind === 'app'
    ? `data/apps/${scope.appId}`
    : undefined;
  const boundChannel: ThreadMeta['channel'] = opts.useCodingAgent ? 'claude_code' : 'chat';
  const boundCodingAgent = opts.useCodingAgent ? resolveCodingAgent(threadId) : undefined;
  const boundRepoName = opts.useCodingAgent
    ? scopeRepoName(scope, loadedOr(repositories.value, []))
    : undefined;
  mutateThreadMeta(threadId, {
    state: 'active',
    channel: boundChannel,
    repoId: boundRepoId,
    repoName: boundRepoName,
    codingAgentKind: boundCodingAgentKind,
    codingAgentFolder: boundCodingAgentFolder,
    codingAgent: boundCodingAgent,
  });
  const modelOverride = resolveModel(threadId);
  const reasoningEffortOverride = resolveReasoningEffort(threadId) ?? undefined;
  const providerOverride = resolveProvider(threadId) ?? undefined;
  const ccModelOverride = resolveCcModel(threadId);
  const ccReasoningEffortOverride = resolveCcReasoningEffort(threadId);
  const user = opts.useCodingAgent ? null : resolveUser(threadId);
  if (wireHashes.length > 0) markHashesAsSent(wireHashes);
  const releaseImages = retainSendingImageHashes(wireHashes);
  noteLocalComposeEdit(threadId);
  clearDraft(threadId);
  const shouldFocus = opts.focus ?? true;
  if (shouldFocus) setFocusedThread(threadId);
  const rollBack = () => rollBackFirstSend(threadId, { text, imageHashes: wireHashes, mode });
  try {
    const outcome = await sendMessage(text, wireHashes.length > 0 ? wireHashes : undefined, {
      useCodingAgent: opts.useCodingAgent,
      context: opts.context,
      threadId,
      focus: shouldFocus,
      modelOverride,
      reasoningEffortOverride,
      providerOverride,
      ccModelOverride,
      ccReasoningEffortOverride,
      alsoMention: user ? [user] : undefined,
      hookArgs: getComposeSelectionOverride(threadId).hookArgs,
      threadStarted: () => awaitThreadStarted(threadId),
      settlement: { kind: 'first-send', mode, engineDraftAtSend },
    });
    if (outcome === 'dropped') {
      rollBack();
      return false;
    }
    if (outcome === 'sent') settleAcceptedFirstSend(threadId);
    else pushClearedComposeAfterSend(threadId);
    return true;
  } catch (err) {
    rollBack();
    throw err;
  } finally {
    releaseImages();
  }
}

export async function sendFollowup(
  threadId: string,
  text: string,
  imageHashes?: string[],
  opts?: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<void> {
  if (imageHashes?.length) markHashesAsSent(imageHashes);
  const releaseImages = retainSendingImageHashes(imageHashes);
  try {
    updateCompose(threadId, { text: '', image_hashes: [] });
    const outcome = await sendMessage(text, imageHashes, {
      ...opts,
      threadId,
      focus: opts?.focus ?? true,
    });
    if (outcome === 'dropped') appendMessagesToCompose(threadId, [{ text, imageHashes: imageHashes ?? [] }]);
  } finally {
    releaseImages();
  }
}

function flushAllPending(): void {
  const headers: Record<string, string> = { 'Content-Type': 'application/json', ...deviceIdHeader(),
    ...requestGenerationGuard().headers };

  const owed = new Set([...pendingTimers.keys(), ...owedComposePushes]);
  for (const timer of pendingTimers.values()) clearTimeout(timer);
  for (const threadId of owed) {
    const thread = threadMap.value.get(threadId);
    if (!thread) continue;
    const draft = getDraft(threadId);
    const selectionOverride = getComposeSelectionOverride(threadId);
    const selectionForFlush = Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
    const body = JSON.stringify({
      text: draft.text,
      image_hashes: draft.image_hashes,
      mode: composeModeForWrite(threadId, thread.meta.state, draft),
      selection: selectionForFlush,
      compose_epoch: knownComposeEpoch(threadId),
    });
    if (body.length > 64 * 1024) {
      // BEST-EFFORT: the tab is unloading (telemetry carve-out in
      // .claude/rules/frontend.md), so any toast would never render.
      // The next foreground page load re-PUTs
      // this draft via the normal debounce path. No data is lost, the draft
      // still being in `composeDrafts` and retried on the next keystroke.
      console.warn(`[compose] keepalive body exceeds 64KB (${body.length}B) for ${threadId}; will retry on next foreground push`);
      continue;
    }
    fetch(`${API}/threads/${encodeURIComponent(threadId)}/compose`, {
      method: 'PUT',
      headers,
      body,
      keepalive: true,
    }).catch(() => {  });
  }
  pendingTimers.clear();
  owedComposePushes.clear();
  for (const threadId of owed) undeliveredComposeDrafts.add(threadId);
}

export function _flushAllPendingForTesting(): void {
  flushAllPending();
}

if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', flushAllPending);
  window.addEventListener('pagehide', flushAllPending);
}

export function appendMessagesToCompose(threadId: string, messages: ReadonlyArray<{ text: string; imageHashes: readonly string[] }>): void {
  const draftImages = getDraft(threadId).image_hashes;
  const images = [...new Set(messages.flatMap(m => m.imageHashes))].filter(h => !draftImages.includes(h));
  if (images.length > 0) {
    updateCompose(threadId, { image_hashes: [...draftImages, ...images] });
  }
  const texts = messages.map(m => m.text).filter(t => t.length > 0);
  if (texts.length === 0) return;
  const existing = getDraft(threadId).text;
  const addition = texts.join('\n\n');
  const combined = existing.trim().length > 0 ? `${existing}\n\n${addition}` : addition;
  updateCompose(threadId, { text: combined });
}
