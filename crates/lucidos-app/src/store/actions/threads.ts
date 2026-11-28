import { showToast, showConfirm, threadMap, applyingNowThreadIds, discardingCCThreadIds, revealOnFocus, setFocusedThread, focusedThreadId, awaitedThreadId } from '../store';
import { closeDashboardPanel } from '../dashboards';
import { revealThreadPane } from './pane';
import type { ThreadMeta } from '../thread-events';
import { deepLinkAnchorInThread, describeWaitSubscription, NO_CHANGE } from '../thread-events';
import type { ConfirmDetailGroup, ConfirmDetails } from '../types';
import { activeViewKey, orderedVisibleThreadIds } from '../drawerOrderHandle';
import { saveThread, unsaveThread, archiveThread, unarchiveThreads, type ArchiveSkippedMember } from '../../api/threads';
import { ApiError, putComposeOnThread } from '../../api/client';
import { threadQueueHolds } from './threadQueue';
import { abandonRowIntent, acknowledgeRowIntent, archiveRequestInFlight, holdRowIntent, rowRequestInFlight, type RowFields, type RowIntent } from '../rowIntents';
import { loadThreadEvents, ensureThreadByIdInMap, refreshStaleThreadEvents, threadEventsStillArriving } from './thread-loading';
import { clearDraft, draftPresentThreadIds, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { acknowledgeComposeEdits, knownComposeEpoch, noteLocalComposeEdit, type ParsedComposeReport } from '../composeSync';
import { scrollToEventAndPulse, scrollToChangeAndPulse, clearPendingEventScroll, stopFollowingBottom } from '../../components/chat/scrollStateHandle';
import { pushThreadNavState } from './thread-navigation';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { markThreadOpenStart } from '../../utils/threadOpenMarks';
import { errorDetail } from '../../utils/errorDetail';
import { pruneRecents } from './searchRecents';
import { BLOCKER_REASON, blockedRefusal, type BlockedRefusal } from './blockerCopy';
import { collectThreadFamily } from './threadFamily';
import { openListedThread } from './event-navigation';


export interface FocusThreadOptions {
  targetEventId?: string | null;
  targetChangeId?: string | null;
  revealPane?: boolean;
  source?: string;
}

export function focusThread(threadId: string, options?: FocusThreadOptions): void {
  const wasFocused = focusedThreadId.value === threadId;
  if (!wasFocused) stopFollowingBottom();
  setFocusedThread(threadId);
  closeDashboardPanel();
  rememberOpenedViewRows(threadId);
  const targetEventId = options?.targetEventId ?? null;
  const targetChangeId = targetEventId ? null : (options?.targetChangeId ?? null);
  const hasTarget = !!targetEventId || !!targetChangeId;
  if (!hasTarget) clearPendingEventScroll();

  if (!wasFocused) {
    markThreadOpenStart(threadId, {
      ...currentPerfBaseline(),
      warm: threadMap.value.get(threadId)?.eventsLoaded ?? false,
    });
  }

  loadThreadEvents(threadId);
  refreshStaleThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  if (options?.revealPane !== false) revealThreadPane();

  if (targetEventId) {
    landOnEvent(threadId, targetEventId);
  } else if (targetChangeId) {
    scrollToChangeAndPulse(targetChangeId, {
      stillArriving: () => threadEventsStillArriving(threadId),
      onUnresolved: () => showToast(
        'That change is not shown in this thread.',
        'warning',
      ),
    });
  }

}

export function landOnEvent(threadId: string, eventId: string): void {
  scrollToEventAndPulse(eventId, {
    anchorFor: () => deepLinkAnchorInThread(threadMap.value.get(threadId), eventId),
    stillArriving: () => threadEventsStillArriving(threadId),
    onUnresolved: () => showToast(
      'That event is not shown in this thread.',
      'warning',
    ),
  });
}

export function focusSpawnedThread(threadId: string, options?: FocusThreadOptions): void {
  awaitedThreadId.value = threadId;
  focusThread(threadId, options);
}

export type FocusBootstrapOutcome =
  | { kind: 'focused' }
  | { kind: 'awaiting' }
  | { kind: 'not-found' }
  | { kind: 'failed'; error: unknown };

export async function focusThreadOrBootstrapResult(
  threadId: string,
  options?: FocusThreadOptions,
): Promise<FocusBootstrapOutcome> {
  if (threadMap.value.has(threadId)) {
    focusThread(threadId, options);
    return { kind: 'focused' };
  }
  const previousFocus = focusedThreadId.value;
  awaitedThreadId.value = threadId;
  setFocusedThread(threadId);
  if (previousFocus !== threadId) stopFollowingBottom();
  if (options?.revealPane !== false) revealThreadPane();
  let found: boolean;
  try {
    found = await ensureThreadByIdInMap(threadId);
  } catch (error) {
    releaseAwait(threadId, previousFocus);
    return { kind: 'failed', error };
  }
  if (!found) {
    try {
      if (await threadQueueHolds(threadId)) return { kind: 'awaiting' };
      found = await ensureThreadByIdInMap(threadId);
    } catch (error) {
      releaseAwait(threadId, previousFocus);
      return { kind: 'failed', error };
    }
  }
  if (!found) {
    releaseAwait(threadId, previousFocus);
    pruneRecents(threadId, 'threads');
    return { kind: 'not-found' };
  }
  if (awaitedThreadId.value === threadId) awaitedThreadId.value = null;
  focusThread(threadId, options);
  return { kind: 'focused' };
}

function releaseAwait(threadId: string, previousFocus: string | null): void {
  if (awaitedThreadId.value !== threadId) return;
  awaitedThreadId.value = null;
  if (focusedThreadId.value === threadId) setFocusedThread(previousFocus);
}

export function focusThreadOrBootstrap(threadId: string, options?: FocusThreadOptions): void {
  const from = options?.source ? ` (requested by ${options.source})` : '';
  void focusThreadOrBootstrapResult(threadId, options).then(outcome => {
    if (outcome.kind === 'not-found') {
      showToast(`Thread "${threadId}" no longer exists${from}`, 'error');
    } else if (outcome.kind === 'failed') {
      showToast(`Couldn't open thread "${threadId}"${from}: ${errorDetail(outcome.error)}`, 'error');
    }
  }).catch(err => {
    showToast(`Couldn't open thread "${threadId}"${from}: ${errorDetail(err)}`, 'error');
  });
}

export function unfocusThread(opts?: { revealPane?: boolean }): void {
  setFocusedThread(null);
  openedViewRows = null;
  revealOnFocus.value = false;
  stopFollowingBottom();
  if (opts?.revealPane !== false) revealThreadPane();
}


export function showBlockedToast(title: string, refusal: BlockedRefusal): void {
  const subThreadId = refusal.subThreadId;
  showToast(BLOCKER_REASON[refusal.blocker], 'error', {
    title,
    action: subThreadId ? { label: 'Show sub-thread', onClick: () => focusThread(subThreadId) } : undefined,
  });
}

export function showArchiveError(err: unknown, threadId: string): void {
  const refusal = blockedRefusal(err, threadId);
  if (refusal) {
    showBlockedToast("Can't archive yet", refusal);
    return;
  }
  showToast(formatArchiveErrorToast(err), 'error');
}

function formatArchiveErrorToast(err: unknown): string {
  if (err instanceof ApiError && err.httpCode === 409 && err.body && typeof err.body === 'object') {
    const body = err.body as Record<string, unknown>;
    if (CHANGE_CLAIM_REFUSALS.has(String(body.reason)) && typeof body.message === 'string') {
      return `Can't archive yet: ${body.message}`;
    }
  }
  return `Failed to archive thread: ${errorDetail(err)}`;
}

const CHANGE_CLAIM_REFUSALS = new Set(['apply_in_progress', 'discard_in_progress']);

export function formatArchiveSkippedToast(skipped: ArchiveSkippedMember[]): string {
  const lines = skipped.map((m) => {
    const title = threadMap.value.get(m.thread_id)?.meta.title || 'Untitled thread';
    return `${title}: ${m.message}`;
  });
  const head = skipped.length === 1
    ? 'One thread was not archived.'
    : `${skipped.length} threads were not archived.`;
  return `${head} ${lines.join(' ')} Archive again once that is done.`;
}

export function changeRows(threadIds: Iterable<string>, change: (meta: ThreadMeta) => boolean): void {
  const map = new Map(threadMap.value);
  let changed = false;
  for (const id of threadIds) {
    const thread = map.get(id);
    if (!thread) continue;
    const meta = { ...thread.meta };
    if (change(meta)) {
      map.set(id, { ...thread, meta });
      changed = true;
    }
  }
  if (changed) threadMap.value = map;
}

export function versionsBeyondCurrent(threadIds: Iterable<string>): Map<string, number> {
  const versions = new Map<string, number>();
  for (const id of threadIds) {
    const thread = threadMap.value.get(id);
    if (thread) versions.set(id, thread.meta.summaryVersion + 1);
  }
  return versions;
}

function holdRows(threadIds: Iterable<string>, kind: RowIntent['kind'], fields: RowFields): void {
  changeRows(threadIds, (meta) => {
    holdRowIntent(meta, kind, fields);
    return true;
  });
}

const ROW_WRITE_VERBS: Record<RowIntent['kind'], string> = { archive: 'archiving', discard: 'discarding', pin: 'pinning', unpin: 'unpinning', move: 'moving' };

export function refuseWhileRowWriteInFlight(threadId: string): boolean {
  const inFlight = rowRequestInFlight(threadId);
  if (inFlight) showToast(`Still ${ROW_WRITE_VERBS[inFlight]} this thread. Try again in a moment.`, 'info');
  return inFlight !== null;
}

type PinWriteKind = 'pin' | 'unpin' | 'move';

export async function writePin(
  threadId: string,
  kind: PinWriteKind,
  fields: RowFields,
  write: () => Promise<{ summary_version?: number }>,
): Promise<boolean> {
  if (refuseWhileRowWriteInFlight(threadId)) return false;
  const legacyVersion = versionsBeyondCurrent([threadId]).get(threadId);
  holdRows([threadId], kind, fields);
  try {
    const { summary_version } = await write();
    const version = summary_version ?? legacyVersion;
    changeRows([threadId], (meta) => version === undefined
      ? abandonRowIntent(meta, kind)
      : acknowledgeRowIntent(meta, kind, version));
    return true;
  } catch (e) {
    changeRows([threadId], (meta) => abandonRowIntent(meta, kind));
    showToast(`Failed to ${kind} thread: ${errorDetail(e)}`, 'error');
    return false;
  }
}

export async function handleSaveThread(threadId: string, folder?: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.saved) return;
  await writePin(threadId, 'pin', { saved: true, section: 'inbox', pinFolder: folder ?? null },
    () => saveThread(threadId, folder));
}

export async function handleUnsaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.meta.saved) return;

  if (!await showConfirm('Remove this thread from the Pinned section?', 'Remove', { variant: 'default' })) {
    return;
  }
  await writePin(threadId, 'unpin', { saved: false, pinFolder: null }, () => unsaveThread(threadId));
}


let openedViewRows: { threadId: string; view: string; ids: string[] } | null = null;

function rememberOpenedViewRows(threadId: string): void {
  const ids = orderedVisibleThreadIds();
  if (!ids.includes(threadId) && openedViewRows?.threadId === threadId) return;
  openedViewRows = { threadId, view: activeViewKey(), ids };
}

export function rememberFocusedViewRows(): void {
  const threadId = focusedThreadId.value;
  if (threadId) rememberOpenedViewRows(threadId);
}

function neighboursOf(ids: readonly string[], idx: number): string[] {
  return [...ids.slice(idx + 1), ...ids.slice(0, idx).reverse()];
}

export function visibleCandidatesAround(aroundId: string): string[] {
  const ordered = orderedVisibleThreadIds();
  const idx = ordered.indexOf(aroundId);
  if (idx >= 0) return neighboursOf(ordered, idx);
  const opened = openedViewRows;
  const openedIdx = opened?.threadId === aroundId && opened.view === activeViewKey()
    ? opened.ids.indexOf(aroundId)
    : -1;
  if (!opened || openedIdx < 0) return ordered;
  const visible = new Set(ordered);
  const nearest = neighboursOf(opened.ids, openedIdx).filter(id => visible.has(id));
  const placed = new Set(nearest);
  return [...nearest, ...ordered.filter(id => !placed.has(id))];
}


export function subscriptionsStoppedByArchive(
  cascade: Set<string>,
  rootId: string,
): { message: string; details: ConfirmDetails } | null {
  const groups: ConfirmDetailGroup[] = [];
  let named = 0;
  let unnamed = 0;
  for (const id of cascade) {
    const t = threadMap.value.get(id);
    if (t === undefined || t.meta.liveEventWaitCount === 0) continue;
    const waits = t.meta.liveEventWaits;
    if (waits.length > 0) {
      named += waits.length;
      groups.push({
        header: id === rootId ? 'This thread' : t.meta.title || 'Sub-thread',
        items: waits.map((w) => `${w.reason} (${describeWaitSubscription(w.on)})`),
      });
    }
    unnamed += Math.max(0, t.meta.liveEventWaitCount - waits.length);
  }
  const count = named + unnamed;
  if (count === 0) return null;
  if (unnamed > 0) {
    groups.push({
      header: `${unnamed} more on sub-threads`,
      items: [],
    });
  }
  return {
    message:
      count === 1
        ? 'Archiving stops what this thread is waiting for. It will not fire.'
        : `Archiving stops waiting for ${count} events. They will not fire.`,
    details: { groups },
  };
}

function discardThreadDraft(threadId: string): void {
  const prior = getDraft(threadId);
  const restore: ComposeDraft = { ...prior, image_hashes: [...prior.image_hashes] };
  const seq = noteLocalComposeEdit(threadId);
  clearDraft(threadId);
  void putComposeOnThread(threadId, '', [], null, undefined, knownComposeEpoch(threadId))
    .then((result) => {
      const fallback: ParsedComposeReport = { revision: null, epoch: null, text: '', imageHashes: [], mode: null, selection: null };
      acknowledgeComposeEdits(threadId, seq, result.report ?? fallback);
    })
    .catch((e) => {
      setDraft(threadId, restore);
      showToast(`Couldn't discard draft: ${errorDetail(e)}`, 'error');
    });
}

export async function handleUnarchiveThread(threadId: string): Promise<void> {
  try {
    await unarchiveThreads([threadId], { withSubThreads: true });
  } catch (e) {
    showToast(`Could not move the thread to Current: ${errorDetail(e)}`, 'error');
  }
}

export const ARCHIVED_ROW_FIELDS: RowFields = { section: 'archived', saved: false, pinFolder: null, codingAgentChangeState: NO_CHANGE };

export function holdArchivedRows(cascade: Iterable<string>): void {
  holdRows(cascade, 'archive', ARCHIVED_ROW_FIELDS);
}

export function settleArchive(
  cascade: Iterable<string>,
  result: Awaited<ReturnType<typeof archiveThread>>,
  legacyVersions: ReadonlyMap<string, number>,
): { skipped: ArchiveSkippedMember[]; skippedIds: Set<string> } {
  const { skipped = [], summary_versions: versions } = result;
  const skippedIds = new Set(skipped.map((m) => m.thread_id));
  changeRows(cascade, (meta) => {
    const version = versions ? versions[meta.id] : legacyVersions.get(meta.id);
    return skippedIds.has(meta.id) || version === undefined
      ? abandonRowIntent(meta, 'archive')
      : acknowledgeRowIntent(meta, 'archive', version);
  });
  return { skipped, skippedIds };
}

export function focusOffCascade(candidates: readonly string[], cascade: ReadonlySet<string>): string | null {
  const nextId = candidates.find(id => !cascade.has(id)) ?? null;
  if (nextId) {
    revealOnFocus.value = true;
    openListedThread(nextId, { revealPane: false });
  } else {
    unfocusThread({ revealPane: false });
  }
  return nextId;
}

export function abandonArchive(cascade: Iterable<string>): void {
  changeRows(cascade, (meta) => abandonRowIntent(meta, 'archive'));
}

export const ARCHIVE_PINNED_CONFIRM ='Archiving unpins this thread. Move it to the archive?';

export async function handleArchiveThread(threadId: string): Promise<void> {
  if (archiveRequestInFlight(threadId)) return;
  if (discardingCCThreadIds.value.has(threadId)) return;
  if (applyingNowThreadIds.value.has(threadId)) {
    showToast('This thread is applying its change. Archive it once the apply ends.', 'info');
    return;
  }

  const thread = threadMap.value.get(threadId);
  if (thread?.meta.saved) {
    if (!await showConfirm(
      ARCHIVE_PINNED_CONFIRM,
      'Archive',
      { variant: 'default' },
    )) {
      return;
    }
  }

  const cascade = collectThreadFamily(threadId);

  const draftedIds = [...cascade].filter((id) => draftPresentThreadIds.value.has(id));
  let discardDrafts = false;
  if (draftedIds.length > 0) {
    const many = draftedIds.length > 1;
    let discardChosen = false;
    const keep = await showConfirm(
      many
        ? 'These threads have unsent drafts. Discard them too?'
        : 'This thread has an unsent draft. Discard it too?',
      many ? 'Keep drafts' : 'Keep draft',
      {
        variant: 'default',
        cancelLabel: 'Cancel',
        extraAction: {
          label: many ? 'Discard drafts' : 'Discard draft',
          onClick: () => { discardChosen = true; },
        },
      },
    );
    if (!keep && !discardChosen) return;
    discardDrafts = discardChosen;
  }

  const stopping = subscriptionsStoppedByArchive(cascade, threadId);
  if (stopping) {
    const proceed = await showConfirm(stopping.message, 'Archive', {
      variant: 'default',
      cancelLabel: 'Cancel',
      details: stopping.details,
    });
    if (!proceed) return;
  }

  if (refuseWhileRowWriteInFlight(threadId)) return;

  const candidates = visibleCandidatesAround(threadId);

  const legacyVersions = versionsBeyondCurrent(cascade);
  holdArchivedRows(cascade);
  const nextId = focusOffCascade(candidates, cascade);

  try {
    const { skipped, skippedIds } = settleArchive(cascade, await archiveThread(threadId), legacyVersions);
    if (skipped.length > 0) {
      if (skippedIds.has(threadId) && focusedThreadId.value === nextId) {
        focusThread(threadId, { revealPane: false });
      }
      showToast(formatArchiveSkippedToast(skipped), 'warning');
    }
    if (discardDrafts) {
      for (const id of draftedIds) {
        if (!skippedIds.has(id)) discardThreadDraft(id);
      }
    }
  } catch (e) {
    abandonArchive(cascade);
    const stillOnAutoFocus = focusedThreadId.value === nextId;
    if (stillOnAutoFocus && threadMap.value.has(threadId)) {
      focusThread(threadId, { revealPane: false });
    }
    showArchiveError(e, threadId);
  }
}
