
import { batch, effect } from '@preact/signals';

import {
  ApiError,
  UPLOAD_STALLED_MESSAGE,
  UPLOAD_UNANSWERED_MESSAGE,
  isTransientFetchError,
  isTransportError,
  uploadThreadBlob,
} from '../../api/client';
import { addAttachedImageHash, rememberSessionBlobUrl } from '../../components/chat/pastedImages';
import { errorDetail } from '../../utils/errorDetail';
import { sha256Hex } from '../../utils/sha256Hex';
import { generateUuid } from '../../utils/uuid';
import { composeDrafts } from '../composeDrafts';
import { landedImages } from '../landedImages';
import {
  addPendingUpload,
  detachPendingUpload,
  getPendingUpload,
  hasPendingUpload,
  pendingUploads,
  removePendingUpload,
  setPendingUploadContentHash,
  setPendingUploadState,
  type PendingUpload,
  type PendingUploadState,
} from '../pendingUploads';
import {
  forgetPendingUploadRecord,
  markPendingUploadLanded,
  ownedPendingUploadRecords,
  persistPendingUpload,
  sendingImageHashes,
  type OwnedRecord,
} from '../pendingUploadRecords';
import { isConnected, showToast, threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { awaitThreadStarted, isThreadStartPending } from './compose';
import { unsentMessages } from '../unsentMessages';

export const UPLOAD_RETRY_DELAYS_MS: readonly number[] = [1_000, 3_000, 8_000];

export const DUPLICATE_IMAGE_TOAST = 'That image is already attached to this message.';

const UPLOAD_FAILED_TOAST_KEY = 'image-upload-failed';

const runs = new Map<string, AbortController>();

export function attachPendingUpload(args: { threadId: string; file: File; bytes: ArrayBuffer }): Promise<void> {
  const { threadId, file, bytes } = args;
  const localId = generateUuid();
  addPendingUpload(newPendingUpload(threadId, localId, file));
  persistPendingUpload({ localId, threadId, name: file.name, mime: file.type, bytes });
  nameByContent(threadId, localId, Promise.resolve(bytes));
  return runPendingUpload(threadId, localId);
}

export function resumePendingUpload(threadId: string, localId: string, file: File): Promise<void> {
  addPendingUpload(newPendingUpload(threadId, localId, file));
  nameByContent(threadId, localId, file.arrayBuffer());
  return runPendingUpload(threadId, localId);
}

function nameByContent(threadId: string, localId: string, bytes: Promise<ArrayBuffer>): void {
  bytes
    .then(sha256Hex)
    .then((hash) => {
      if (hash) setPendingUploadContentHash(threadId, localId, hash);
    })
    .catch((err) => console.warn(`[uploads] could not hash a pending image: ${errorDetail(err)}`));
}

function newPendingUpload(threadId: string, localId: string, file: File): PendingUpload {
  return {
    localId,
    threadId,
    previewUrl: URL.createObjectURL(file),
    mime: file.type,
    state: { kind: 'uploading', sentBytes: 0, totalBytes: file.size },
    file,
  };
}

export function describeUploadFailure(err: unknown): string {
  if (err instanceof ApiError) return err.reason;
  if (isTransportError(err)) return 'Lucidos could not be reached';
  if (err instanceof DOMException && err.name === 'TimeoutError') {
    const ours = err.message === UPLOAD_STALLED_MESSAGE || err.message === UPLOAD_UNANSWERED_MESSAGE;
    return ours ? err.message : 'Lucidos took too long to answer';
  }
  if (err instanceof DOMException && err.name === 'AbortError') return 'The upload was interrupted';
  if (err instanceof Error) return err.message;
  return String(err);
}

function isRefusal(err: unknown): boolean {
  return err instanceof ApiError && err.httpCode >= 400 && err.httpCode < 500;
}

export function stateAfterFailure(
  err: unknown,
  failedAttempts: number,
  connected: boolean,
): PendingUploadState {
  const reason = describeUploadFailure(err);
  if (!isTransientFetchError(err)) return { kind: 'failed', reason, retryable: !isRefusal(err) };
  if (!connected) return { kind: 'offline', reason };
  if (failedAttempts > UPLOAD_RETRY_DELAYS_MS.length) return { kind: 'failed', reason, retryable: true };
  return { kind: 'retrying', attempt: failedAttempts + 1, reason };
}

export async function runPendingUpload(threadId: string, localId: string): Promise<void> {
  if (runs.has(localId)) return;
  const controller = new AbortController();
  runs.set(localId, controller);
  try {
    await attemptUntilSettled(threadId, localId, controller.signal);
  } finally {
    if (runs.get(localId) === controller) runs.delete(localId);
  }
}

async function attemptUntilSettled(threadId: string, localId: string, signal: AbortSignal): Promise<void> {
  for (let failed = 0; ; ) {
    const entry = getPendingUpload(threadId, localId);
    if (!entry || signal.aborted) return;
    let hash: string;
    try {
      hash = await uploadOnce(entry, signal);
    } catch (err) {
      if (signal.aborted || !hasPendingUpload(threadId, localId)) return;
      if (draftIsGone(threadId)) {
        removePendingUpload(threadId, localId);
        return;
      }
      failed += 1;
      const next = stateAfterFailure(err, failed, isConnected.value);
      setPendingUploadState(threadId, localId, next);
      if (next.kind === 'failed') {
        showToast(`Image upload failed: ${next.reason}`, 'error', { key: UPLOAD_FAILED_TOAST_KEY });
        return;
      }
      if (next.kind === 'offline') return;
      await abortableDelay(UPLOAD_RETRY_DELAYS_MS[failed - 1], signal);
      continue;
    }
    if (signal.aborted || !hasPendingUpload(threadId, localId)) return;
    promote(entry, hash);
    return;
  }
}

async function uploadOnce(entry: PendingUpload, signal: AbortSignal): Promise<string> {
  const { threadId, localId, file } = entry;
  if (isThreadStartPending(threadId)) {
    setPendingUploadState(threadId, localId, { kind: 'waiting-for-thread' });
  }
  await awaitThreadStarted(threadId);
  signal.throwIfAborted();
  setPendingUploadState(threadId, localId, { kind: 'uploading', sentBytes: 0, totalBytes: file.size });
  const { hash } = await uploadThreadBlob(threadId, file, {
    signal,
    onProgress: (p) => setPendingUploadState(threadId, localId, { kind: 'uploading', ...p }),
    onBodySent: () => setPendingUploadState(threadId, localId, { kind: 'finishing' }),
  });
  return hash;
}

function promote(entry: PendingUpload, hash: string): void {
  const { threadId, localId, previewUrl } = entry;
  let attached = true;
  batch(() => {
    rememberSessionBlobUrl(hash, previewUrl);
    attached = addAttachedImageHash(threadId, hash);
    markPendingUploadLanded(localId, hash);
    detachPendingUpload(threadId, localId);
  });
  if (!attached) showToast(DUPLICATE_IMAGE_TOAST, 'info');
}

function draftIsGone(threadId: string): boolean {
  const thread = threadMap.peek().get(threadId);
  return !thread || thread.meta.state === 'discarded';
}

function abortableDelay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(done, ms);
    signal.addEventListener('abort', done, { once: true });
    function done() {
      clearTimeout(timer);
      signal.removeEventListener('abort', done);
      resolve();
    }
  });
}

export function cancelPendingUpload(threadId: string, localId: string): void {
  removePendingUpload(threadId, localId);
}

export function retryPendingUpload(threadId: string, localId: string): Promise<void> {
  const state = getPendingUpload(threadId, localId)?.state;
  if (!state || (state.kind !== 'failed' && state.kind !== 'offline')) return Promise.resolve();
  if (state.kind === 'failed' && !state.retryable) return Promise.resolve();
  return runPendingUpload(threadId, localId);
}

function resumeOfflineUploads(): void {
  for (const list of pendingUploads.peek().values()) {
    for (const entry of list) {
      if (entry.state.kind === 'offline') void runPendingUpload(entry.threadId, entry.localId);
    }
  }
}

let wasConnected = isConnected.peek();
effect(() => {
  const connected = isConnected.value;
  if (connected && !wasConnected) resumeOfflineUploads();
  wasConnected = connected;
});

effect(() => {
  const landed = landedImages.value;
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) {
      const hash = entry.contentHash;
      if (hash && landed.get(entry.threadId)?.has(hash)) promote(entry, hash);
    }
  }
});

export function recordStillNeeded(
  record: OwnedRecord,
  entryPending: boolean,
  localHashes: readonly string[],
  unresolvedHashes: readonly string[] = [],
): boolean {
  if (entryPending) return true;
  const hash = record.landedHash;
  if (hash === undefined) return false;
  return localHashes.includes(hash) || unresolvedHashes.includes(hash);
}

effect(() => {
  const entries = pendingUploads.value;
  const attached = [...composeDrafts.value.values()].flatMap((draft) => draft.image_hashes);
  const unresolved = [...unsentMessages.value.values()];
  const sending = sendingImageHashes.value;
  const pending = new Set<string>();
  for (const list of entries.values()) for (const entry of list) pending.add(entry.localId);
  for (const [localId, controller] of runs) {
    if (!pending.has(localId)) {
      controller.abort();
      runs.delete(localId);
    }
  }
  for (const record of ownedPendingUploadRecords()) {
    const needed = recordStillNeeded(
      record,
      pending.has(record.localId),
      attached,
      [...sending, ...unresolved.flatMap((send) => send.body.image_hashes ?? [])],
    );
    if (!needed) forgetPendingUploadRecord(record.localId);
  }
});

let liveEntryThreads = new Set<string>();
effect(() => {
  const threads = threadMap.value;
  const seenLive = liveEntryThreads;
  liveEntryThreads = new Set();
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) {
      if (isLive(threads.get(entry.threadId))) liveEntryThreads.add(entry.threadId);
      else if (seenLive.has(entry.threadId)) cancelPendingUpload(entry.threadId, entry.localId);
    }
  }
});

function isLive(thread: ThreadState | undefined): boolean {
  return thread !== undefined && thread.meta.state !== 'discarded';
}

export function _resetImageUploadRunsForTesting(): void {
  for (const controller of runs.values()) controller.abort();
  runs.clear();
}
