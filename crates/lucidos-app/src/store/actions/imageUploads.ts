/** The upload pipeline for images attached to a draft: one run per pending
 *  entry, from waiting on the draft's thread to a confirmed hash in the draft.
 *  State lives in `store/pendingUploads.ts`; the composer only draws it. Each
 *  entry's bytes are also kept on the device (`store/pendingUploadRecords.ts`),
 *  so a page reload can resume it. An entry lands on its upload's answer or
 *  on the engine's `ImageUploaded` event, whichever comes first.
 *
 *  Plans: `docs/plans/2026-10-02-image-upload-progress-and-resilience.md`,
 *  `docs/plans/2026-10-02-pending-uploads-survive-a-reload.md`,
 *  `docs/plans/2026-10-04-image-upload-lands-on-its-event.md`. */

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
import { composeDrafts, getDraft } from '../composeDrafts';
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
  type OwnedRecord,
} from '../pendingUploadRecords';
import { isConnected, showToast, threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { awaitThreadStarted, isThreadStartPending, serverDraft, serverDraftVersion } from './compose';

/** Backoff before each automatic retry of a transient failure. Its length is
 *  the retry budget: once spent, the entry fails and offers a manual Retry. */
export const UPLOAD_RETRY_DELAYS_MS: readonly number[] = [1_000, 3_000, 8_000];

/** Said when the upload lands on bytes the draft already holds. See
 *  `addAttachedImageHash`. */
export const DUPLICATE_IMAGE_TOAST = 'That image is already attached to this message.';

/** Repeated failures collapse into one card rather than one per image. */
const UPLOAD_FAILED_TOAST_KEY = 'image-upload-failed';

/** The run in progress for each pending entry, keyed by `localId`. Removing
 *  the entry aborts it, and a second Retry cannot start a parallel run. */
const runs = new Map<string, AbortController>();

/** Show a just-attached image as a chip, keep its bytes on the device, and
 *  upload it. Resolves when the upload run ends. */
export function attachPendingUpload(args: { threadId: string; file: File; bytes: ArrayBuffer }): Promise<void> {
  const { threadId, file, bytes } = args;
  const localId = generateUuid();
  addPendingUpload(newPendingUpload(threadId, localId, file));
  persistPendingUpload({ localId, threadId, name: file.name, mime: file.type, bytes });
  nameByContent(threadId, localId, Promise.resolve(bytes));
  return runPendingUpload(threadId, localId);
}

/** Resume an image a previous page load left behind. Its record already
 *  exists and is this page's now. */
export function resumePendingUpload(threadId: string, localId: string, file: File): Promise<void> {
  addPendingUpload(newPendingUpload(threadId, localId, file));
  nameByContent(threadId, localId, file.arrayBuffer());
  return runPendingUpload(threadId, localId);
}

/** Give the entry the hash its `ImageUploaded` will carry, alongside the
 *  upload. An event that beats the hash still lands it (`landedImages`). */
function nameByContent(threadId: string, localId: string, bytes: Promise<ArrayBuffer>): void {
  bytes
    .then(sha256Hex)
    .then((hash) => {
      if (hash) setPendingUploadContentHash(threadId, localId, hash);
    })
    // Best-effort: without a hash the entry still lands on its upload's own
    // answer, as every entry did before, so the user loses nothing here.
    .catch((err) => console.warn(`[uploads] could not hash a pending image: ${errorDetail(err)}`));
}

/** The blob URL is handed off twice: first to the pending entry, for the chip,
 *  then to `sessionBlobUrls` (`promote`), so the confirmed image keeps
 *  rendering from the same in-memory File. Swapping to the server URL re-
 *  fetched over the network on iOS Safari PWA, a brief black flash. */
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

/** Why an upload failed, in words for the chip, its tooltip and the toast.
 *  An `ApiError` already carries the engine's own normalized sentence. */
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

/** A 4xx is the engine refusing THIS upload: a format it does not take, or a
 *  draft that is gone. Retrying the same bytes cannot change that answer. */
function isRefusal(err: unknown): boolean {
  return err instanceof ApiError && err.httpCode >= 400 && err.httpCode < 500;
}

/** The state after attempt number `failedAttempts` (1-based) failed. Only a
 *  transient failure is retried automatically, and only while connected and
 *  within the budget. A verdict fails at once. Pure, for testing. */
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

/** Upload one pending entry until it lands, fails, or the user removes it.
 *  Resolves when the run ends, whatever the outcome; it never rejects. A call
 *  while a run is already going for this entry joins nothing and returns. */
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
      // Removed or cancelled while we were away: the user is done with it.
      if (signal.aborted || !hasPendingUpload(threadId, localId)) return;
      // The draft itself is gone: refused and rolled back, or discarded. Its
      // composer is not on screen, so the chip has nowhere to say anything.
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
      // Resumed by `resumeOfflineUploads` when the connection comes back.
      if (next.kind === 'offline') return;
      await abortableDelay(UPLOAD_RETRY_DELAYS_MS[failed - 1], signal);
      continue;
    }
    // Mid-flight cancel: the X dropped the entry and revoked its URL, so the
    // hash must not reach the draft.
    if (signal.aborted || !hasPendingUpload(threadId, localId)) return;
    promote(entry, hash);
    return;
  }
}

async function uploadOnce(entry: PendingUpload, signal: AbortSignal): Promise<string> {
  const { threadId, localId, file } = entry;
  // The blob endpoint guards on the thread row existing. A fresh draft's
  // `POST /threads` may still be in flight, or owed after a transient failure,
  // in which case this wait re-attempts it.
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

/** Hand the blob URL to the session map FIRST, so `getAttachedImages` returns
 *  it the moment the hash lands. Then commit the hash and detach the entry
 *  without revoking, in one batch, so the strip never renders with neither.
 *
 *  The draft refuses a hash it already holds (a second paste of one
 *  screenshot). `rememberSessionBlobUrl` keeps the first URL either way. */
function promote(entry: PendingUpload, hash: string): void {
  const { threadId, localId, previewUrl } = entry;
  let attached = true;
  batch(() => {
    rememberSessionBlobUrl(hash, previewUrl);
    attached = addAttachedImageHash(threadId, hash);
    // Before the detach, so the record outlives the entry until the engine's
    // draft holds the hash (`recordStillNeeded`).
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

/** The X on a pending chip. Dropping the entry aborts its run. */
export function cancelPendingUpload(threadId: string, localId: string): void {
  removePendingUpload(threadId, localId);
}

/** The Retry on a failed chip: a fresh run with a fresh budget. */
export function retryPendingUpload(threadId: string, localId: string): Promise<void> {
  const state = getPendingUpload(threadId, localId)?.state;
  if (!state || (state.kind !== 'failed' && state.kind !== 'offline')) return Promise.resolve();
  if (state.kind === 'failed' && !state.retryable) return Promise.resolve();
  return runPendingUpload(threadId, localId);
}

/** Restart every upload that was waiting for the connection. */
function resumeOfflineUploads(): void {
  for (const list of pendingUploads.peek().values()) {
    for (const entry of list) {
      if (entry.state.kind === 'offline') void runPendingUpload(entry.threadId, entry.localId);
    }
  }
}

// An `offline` entry exists only while the connection is down, so the moment
// it comes back is the one moment to resume. Covers a reconnect noticed by the
// health poll and an iOS wake alike, since both flip `connectionStatus`.
let wasConnected = isConnected.peek();
effect(() => {
  const connected = isConnected.value;
  if (connected && !wasConnected) resumeOfflineUploads();
  wasConnected = connected;
});

// The engine reported these bytes stored on this draft, so the entry has
// landed. Its upload's own answer can be lost on the way back, which stalled
// the chip until the watchdog re-uploaded. Whichever signal comes first lands
// it. Detaching aborts the run, so the other finds no entry and does nothing.
effect(() => {
  const landed = landedImages.value;
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) {
      const hash = entry.contentHash;
      if (hash && landed.get(entry.threadId)?.has(hash)) promote(entry, hash);
    }
  }
});

/** Whether this page still needs a record's bytes to survive a reload:
 *  - while its entry is pending, yes;
 *  - once it landed, until the engine's draft holds the hash, or the local
 *    draft no longer does (sent, removed, discarded);
 *  - an entry that ended without landing, no. Pure, for testing. */
export function recordStillNeeded(
  record: OwnedRecord,
  entryPending: boolean,
  localHashes: readonly string[],
  serverHashes: readonly string[] | undefined,
): boolean {
  if (entryPending) return true;
  const hash = record.landedHash;
  if (hash === undefined) return false;
  return localHashes.includes(hash) && !(serverHashes ?? []).includes(hash);
}

// Every way an entry ends shows up here as a change to one of three signals:
// the entry map, a local draft, or the engine's draft. So no removal path can
// forget its record, and no run outlives its entry.
effect(() => {
  const entries = pendingUploads.value;
  void composeDrafts.value;
  void serverDraftVersion.value;
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
      getDraft(record.threadId).image_hashes,
      serverDraft.get(record.threadId)?.imageHashes,
    );
    if (!needed) forgetPendingUploadRecord(record.localId);
  }
});

// A draft that is discarded, or whose row is removed, takes its pending images
// with it. Only a thread this rule has seen live counts, so an entry whose
// thread this page never saw is left alone.
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
