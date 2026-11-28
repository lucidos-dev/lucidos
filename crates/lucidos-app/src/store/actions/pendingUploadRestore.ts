
import { isHistoricalRestoreSource } from '../../utils/restoreSourceGeneration';
import { effect } from '@preact/signals';
import { waitForRestoreAdmission, restoreSourcePreserved, queuedRestoreSourceId } from '../../utils/restoreAdmission';
import { readUnsentMessageStore } from '../unsentMessageRecords';
import { rerunWhenPageShows } from '../deviceDatabase';

import { queueUploadSend, queuedUploadSends } from '../../components/chat/prompt-input-helpers';
import { composeDrafts, getDraft } from '../composeDrafts';
import { getPendingUploads, pendingUploads } from '../pendingUploads';
import {
  PENDING_UPLOAD_MAX_AGE_MS,
  adoptPendingUploadRecord,
  adoptQueuedUploadSendRecord,
  forgetPendingUploadRecord,
  forgetQueuedUploadSend,
  notePendingUploadsOwnedElsewhere,
  ownedPendingUploadRecords,
  ownedQueuedUploadSends,
  persistQueuedUploadSend,
  readPendingUploadStore,
  type PendingUploadRecord,
  type QueuedUploadSendSnapshot,
} from '../pendingUploadRecords';
import { holdPageOwnerLock, liveOwnerIds, ownedByAnotherLiveTab, pageOwnerId } from '../pageOwner';
import { isConnected, showToast, threadListFetched, threadMap } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { resumePendingUpload } from './imageUploads';
import { ensureThreadByIdInMap } from './thread-loading';

export interface RestoreOutcome {
  restored: number;
  gone: number;
  expired: number;
  unsentQueuedUploadSends: number;
  unresolved: number;
  unresolvedReason?: string;
}

export function droppedImagesMessage(
  outcome: Pick<RestoreOutcome, 'gone' | 'expired' | 'unsentQueuedUploadSends' | 'unresolved' | 'unresolvedReason'>,
): string | null {
  const parts = [
    droppedImagesSentence(outcome.gone, outcome.expired),
    unsentQueuedUploadSendsSentence(outcome.unsentQueuedUploadSends),
    unresolvedSentence(outcome.unresolved, outcome.unresolvedReason),
  ];
  return parts.filter(Boolean).join(' ') || null;
}

function droppedImagesSentence(gone: number, expired: number): string {
  const total = gone + expired;
  if (total === 0) return '';
  const one = total === 1;
  const head = one
    ? 'An image attached before the reload was dropped'
    : `${total} images attached before the reload were dropped`;
  let why: string;
  if (gone > 0 && expired > 0) why = 'their drafts are gone or they waited more than 7 days';
  else if (gone > 0) why = one ? 'its draft no longer exists' : 'their drafts no longer exist';
  else why = one ? 'it waited more than 7 days' : 'they waited more than 7 days';
  return `${head}: ${why}.`;
}

function unsentQueuedUploadSendsSentence(count: number): string {
  if (count === 0) return '';
  const head = count === 1 ? 'A send queued before the reload was' : `${count} sends queued before the reload were`;
  return `${head} not sent: an image is lost or the draft changed. Check the draft, then send.`;
}

function unresolvedSentence(count: number, reason: string | undefined): string {
  if (count === 0) return '';
  const head = count === 1
    ? 'An image attached before the reload could not be brought back yet'
    : `${count} images attached before the reload could not be brought back yet`;
  return `${head} (${reason ?? 'unknown error'}). Lucidos tries again on reconnect and on the next reload.`;
}

type DraftVerdict = 'live' | 'gone';

async function draftVerdict(threadId: string): Promise<DraftVerdict> {
  const found = await ensureThreadByIdInMap(threadId);
  return found && draftIsLive(threadId) ? 'live' : 'gone';
}

function draftIsLive(threadId: string): boolean {
  const thread = threadMap.peek().get(threadId);
  return thread !== undefined && thread.meta.state !== 'discarded';
}

const RESTORE_TOAST_KEY = 'pending-upload-restore';

let restoring: Promise<RestoreOutcome> | null = null;

export function restorePendingUploads(now: number = Date.now()): Promise<RestoreOutcome> {
  restoring ??= restorePass(now).finally(() => {
    restoring = null;
  });
  return restoring;
}

async function restorePass(now: number): Promise<RestoreOutcome> {
  await waitForRestoreAdmission();
  const outcome: RestoreOutcome = { restored: 0, gone: 0, expired: 0, unsentQueuedUploadSends: 0, unresolved: 0 };
  const [stored, sends] = await Promise.all([readPendingUploadStore(), readUnsentMessageStore()]);
  await waitForRestoreAdmission();
  const uploads = stored.uploads.filter((r) => !isHistoricalRestoreSource(r) && !restoreSourcePreserved('upload', r.localId));
  const queuedUploadSendRecords = stored.queuedUploadSendRecords.filter((r) => !isHistoricalRestoreSource(r) && !restoreSourcePreserved('queued', queuedRestoreSourceId(r)));
  const liveOwners = await liveOwnerIds();
  await waitForRestoreAdmission();
  const ownedElsewhere = (ownerId: string) => ownedByAnotherLiveTab(ownerId, liveOwners);
  notePendingUploadsOwnedElsewhere(uploads.filter((r) => ownedElsewhere(r.ownerId)));

  const alreadyOwned = new Set(ownedPendingUploadRecords().map((r) => r.localId));
  const restoredIds = new Set<string>();
  const unresolvedThreads = new Set<string>();
  const verdicts = new Map<string, Promise<DraftVerdict>>();

  for (const record of uploads) {
    if (isHistoricalRestoreSource(record)) continue;
    if (record.ownerId === pageOwnerId || ownedElsewhere(record.ownerId) || alreadyOwned.has(record.localId)) continue;
    if (record.landedHash !== undefined && sends.some((send) => send.body.image_hashes?.includes(record.landedHash!))) continue;
    let verdict: DraftVerdict;
    try {
      if (!verdicts.has(record.threadId)) verdicts.set(record.threadId, draftVerdict(record.threadId));
      verdict = await verdicts.get(record.threadId)!;
      await waitForRestoreAdmission();
      if (isHistoricalRestoreSource(record)) continue;
    } catch (err) {
      outcome.unresolved += 1;
      outcome.unresolvedReason ??= errorDetail(err);
      unresolvedThreads.add(record.threadId);
      continue;
    }
    if (verdict === 'gone' || !draftIsLive(record.threadId)) {
      forgetPendingUploadRecord(record.localId);
      outcome.gone += 1;
      continue;
    }
    if (record.landedHash !== undefined && getDraft(record.threadId).image_hashes.includes(record.landedHash)) {
      adoptPendingUploadRecord(record, true);
      restoredIds.add(record.localId);
      continue;
    }
    if (now - record.createdAt > PENDING_UPLOAD_MAX_AGE_MS) {
      forgetPendingUploadRecord(record.localId);
      outcome.expired += 1;
      continue;
    }
    resume(record);
    outcome.restored += 1;
    restoredIds.add(record.localId);
  }

  for (const queued of queuedUploadSendRecords) {
    if (isHistoricalRestoreSource(queued)) continue;
    if (queued.ownerId === pageOwnerId || ownedElsewhere(queued.ownerId)) continue;
    if (unresolvedThreads.has(queued.threadId)) continue;
    const imagesBack = queued.localIds.length > 0 && queued.localIds.every((id) => restoredIds.has(id));
    if (imagesBack && getDraft(queued.threadId).text === queued.text) {
      adoptQueuedUploadSendRecord(queued);
      queueUploadSend(queued.threadId, queued.intent);
      continue;
    }
    forgetQueuedUploadSend(queued.threadId, queued);
    if (queued.localIds.length > 0) outcome.unsentQueuedUploadSends += 1;
  }

  const message = droppedImagesMessage(outcome);
  if (message) showToast(message, 'warning', { key: RESTORE_TOAST_KEY });
  return outcome;
}

function resume(record: PendingUploadRecord): void {
  adoptPendingUploadRecord(record);
  const file = new File([record.bytes], record.name, { type: record.mime });
  void resumePendingUpload(record.threadId, record.localId, file);
}

function runRestore(): void {
  restorePendingUploads().then((outcome) => {
    pendingUnresolved = outcome.unresolved > 0;
  }).catch((err) => {
    if (rerunWhenPageShows(err, runRestore)) return;
    showToast(`Could not bring back images attached before the reload: ${errorDetail(err)}`, 'error');
  });
}

let pendingUnresolved = false;

export function installPendingUploadRestore(): () => void {
  holdPageOwnerLock();
  let ran = false;
  let wasConnected = isConnected.peek();
  const stops = [
    effect(() => {
      if (ran || !threadListFetched.value) return;
      ran = true;
      runRestore();
    }),
    effect(() => {
      const connected = isConnected.value;
      if (connected && !wasConnected && ran && pendingUnresolved) runRestore();
      wasConnected = connected;
    }),
    effect(() => {
      const queued = queuedUploadSends.value;
      void pendingUploads.value;
      void composeDrafts.value;
      const stored = ownedQueuedUploadSends();
      for (const [threadId, intent] of queued) {
        const now: QueuedUploadSendSnapshot = {
          localIds: getPendingUploads(threadId).map((u) => u.localId),
          text: getDraft(threadId).text,
        };
        if (!sameSnapshot(stored.get(threadId), now)) persistQueuedUploadSend(threadId, intent, now);
      }
      for (const threadId of [...stored.keys()]) {
        if (!queued.has(threadId)) forgetQueuedUploadSend(threadId);
      }
    }),
  ];
  return () => {
    for (const stop of stops) stop();
  };
}

function sameSnapshot(a: QueuedUploadSendSnapshot | undefined, b: QueuedUploadSendSnapshot): boolean {
  return a !== undefined && a.text === b.text
    && a.localIds.length === b.localIds.length && a.localIds.every((id, i) => id === b.localIds[i]);
}

export function _resetPendingUploadRestoreForTesting(): void {
  restoring = null;
  pendingUnresolved = false;
}
