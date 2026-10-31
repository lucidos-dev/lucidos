/**
 * An attached image is never a chip that merely looks finished, and a send
 * that waits on one is visible for its whole life. It ends dispatched,
 * cancelled, or released with the reason, never silently. Plan:
 * `docs/plans/2026-10-02-image-upload-progress-and-resilience.md`.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

(globalThis as any).URL.revokeObjectURL = () => {};

import { pendingChipView } from '../PendingUploadChip';
import {
  clearQueuedUploadSend,
  queueUploadSend,
  queuedUploadSends,
  settleQueuedUploadSends,
  submittingThreadIds,
  uploadBlockedSends,
  uploadSendNotice,
  uploadSendNoticeText,
} from '../prompt-input-helpers';
import {
  addPendingUpload,
  removePendingUpload,
  setPendingUploadState,
  uploadsGate,
  _resetPendingUploadsForTesting,
  type PendingUpload,
  type PendingUploadState,
} from '../../../store/pendingUploads';

const T = 't-1';

function upload(localId: string, state: PendingUploadState): PendingUpload {
  return { localId, threadId: T, previewUrl: `blob:${localId}`, mime: 'image/png', state, file: {} as File };
}

const failed: PendingUploadState = { kind: 'failed', reason: 'Lucidos could not be reached', retryable: true };
const uploading: PendingUploadState = { kind: 'uploading', sentBytes: 1, totalBytes: 4 };

describe('pendingChipView draws every state differently from a finished chip', () => {
  it('shows determinate progress while bytes move', () => {
    expect(pendingChipView(uploading)).toEqual({
      ring: 0.25, caption: '25%', label: 'Uploading image, 25%', action: 'none',
    });
  });

  it('turns for every wait of unknown length, each with its own caption', () => {
    const waits: PendingUploadState[] = [
      { kind: 'waiting-for-thread' },
      { kind: 'finishing' },
      { kind: 'retrying', attempt: 2, reason: 'Lucidos could not be reached' },
      { kind: 'offline', reason: 'Lucidos could not be reached' },
    ];
    const views = waits.map(pendingChipView);
    expect(views.every((v) => v.ring === 'spin' && v.action === 'none')).toBe(true);
    expect(views.map((v) => v.caption)).toEqual(['Waiting', 'Uploading', 'Retrying', 'Offline']);
    // The reason reaches the tooltip and the accessible name.
    expect(views[2].label).toContain('Lucidos could not be reached');
  });

  it('calls the wait after the browser took the body uploading, never finished', () => {
    // WebKit reports a small image sent before its bytes travel, so on a phone
    // this state covers nearly the whole upload.
    const view = pendingChipView({ kind: 'finishing' });
    expect(view.caption).toBe('Uploading');
    expect(view.label).not.toMatch(/uploaded|finish/i);
  });

  it('offers Retry for a failure a retry can fix, and only names one it cannot', () => {
    expect(pendingChipView(failed)).toMatchObject({ ring: null, caption: 'Retry', action: 'retry' });
    const refused = pendingChipView({ kind: 'failed', reason: 'That file is a PDF', retryable: false });
    expect(refused).toMatchObject({ ring: null, caption: 'Failed', action: 'refused' });
    expect(refused.label).toContain('That file is a PDF');
  });
});

describe('uploadSendNotice', () => {
  it('says a queued send is waiting, counting what it waits on', () => {
    const one = uploadSendNotice({ useCodingAgent: false, context: null }, false, [upload('a', uploading)]);
    expect(uploadSendNoticeText(one!)).toBe('Sends when the image finishes uploading');
    const two = uploadSendNotice({ useCodingAgent: false, context: null }, false, [upload('a', uploading), upload('b', uploading)]);
    expect(uploadSendNoticeText(two!)).toBe('Sends when the 2 images finish uploading');
    const aside = uploadSendNotice({ useCodingAgent: false, context: null, asSideQuestion: true }, false, [upload('a', uploading)]);
    expect(uploadSendNoticeText(aside!)).toBe('Asks when the image finishes uploading');
  });

  it('says why a send did not go while an image is failed', () => {
    const notice = uploadSendNotice(undefined, true, [upload('a', failed)]);
    expect(uploadSendNoticeText(notice!)).toBe('Not sent: an image failed to upload. Retry it or remove it, then send.');
  });

  it('says nothing with no send waiting and none refused', () => {
    expect(uploadSendNotice(undefined, false, [upload('a', uploading)])).toBeNull();
    // A refusal mark outlives nothing: with no failed image there is no reason left.
    expect(uploadSendNotice(undefined, true, [upload('a', uploading)])).toBeNull();
  });
});

describe('settleQueuedUploadSends', () => {
  beforeEach(() => {
    _resetPendingUploadsForTesting();
    queuedUploadSends.value = new Map();
    uploadBlockedSends.value = new Set();
    submittingThreadIds.value = new Set();
  });

  it('keeps a send queued while its image is in flight, retries included', () => {
    addPendingUpload(upload('a', uploading));
    queueUploadSend(T, { useCodingAgent: false, context: null });
    const dispatch = vi.fn();
    settleQueuedUploadSends(dispatch);
    setPendingUploadState(T, 'a', { kind: 'retrying', attempt: 2, reason: 'x' });
    settleQueuedUploadSends(dispatch);
    setPendingUploadState(T, 'a', { kind: 'offline', reason: 'x' });
    settleQueuedUploadSends(dispatch);
    expect(dispatch).not.toHaveBeenCalled();
    expect(queuedUploadSends.value.has(T)).toBe(true);
  });

  it('dispatches once the image lands', () => {
    addPendingUpload(upload('a', uploading));
    queueUploadSend(T, { useCodingAgent: true, context: null });
    removePendingUpload(T, 'a');
    const dispatch = vi.fn();
    settleQueuedUploadSends(dispatch);
    expect(dispatch).toHaveBeenCalledWith(T, { useCodingAgent: true, context: null });
    expect(queuedUploadSends.value.has(T)).toBe(false);
  });

  it('releases the send on a terminal failure and says why, keeping the image', () => {
    addPendingUpload(upload('a', uploading));
    queueUploadSend(T, { useCodingAgent: false, context: null });
    setPendingUploadState(T, 'a', failed);
    const dispatch = vi.fn();
    settleQueuedUploadSends(dispatch);

    expect(dispatch).not.toHaveBeenCalled();
    expect(queuedUploadSends.value.has(T)).toBe(false);
    // Send comes back: the optimistic Cancel morph is released.
    expect(submittingThreadIds.value.has(T)).toBe(false);
    expect(uploadBlockedSends.value.has(T)).toBe(true);
    expect(uploadsGate(T)).toBe('failed');
  });

  it('drops the reason once the failed image is retried or removed', () => {
    addPendingUpload(upload('a', failed));
    uploadBlockedSends.value = new Set([T]);
    setPendingUploadState(T, 'a', uploading);
    settleQueuedUploadSends(vi.fn());
    expect(uploadBlockedSends.value.has(T)).toBe(false);
  });

  it('a user cancel ends the send with nothing left behind', () => {
    addPendingUpload(upload('a', uploading));
    queueUploadSend(T, { useCodingAgent: false, context: null });
    clearQueuedUploadSend(T);
    removePendingUpload(T, 'a');
    const dispatch = vi.fn();
    settleQueuedUploadSends(dispatch);
    expect(dispatch).not.toHaveBeenCalled();
    expect(submittingThreadIds.value.has(T)).toBe(false);
  });
});
