/**
 * Uploading image attachments are not sendable until the blob endpoint
 * returns a hash, but the Send button should remain clickable. A click during
 * that window queues the current send intent and PromptInput retries once the
 * pending upload entry settles into draft image_hashes.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('PromptInput upload send queue', () => {
  it('queues submit while uploads are in flight before dispatchSend can run', () => {
    const fn = promptSource.match(/async function submit\(\)[\s\S]*?\n  \}/);
    expect(fn, 'submit() not found').not.toBeNull();
    const body = fn![0];
    expect(body).toMatch(/if\s*\(\s*threadId\s*&&\s*uploadInFlight\s*\)\s*\{[\s\S]*?queueUploadSend\(threadId/);

    const queueIdx = body.indexOf('queueUploadSend(threadId');
    const dispatchIdx = body.indexOf('beginSend');
    expect(queueIdx).toBeGreaterThan(-1);
    expect(dispatchIdx).toBeGreaterThan(queueIdx);
  });

  it('does not disable the Send button just because an upload is in flight', () => {
    expect(promptSource).not.toContain("morphMode === 'send' ? uploadsBlocking");
    expect(promptSource).toContain("morphMode === 'send' ? false");
    expect(promptSource).toContain('Send after image upload');
  });

  // `submit` empties the box at its own dispatch point and returns before that
  // one, the send being owed to an upload. Left alone, the box would still
  // claim to hold unsent typing, and `resolveEmptyDraftSync` would adopt the
  // message just sent straight back as a draft.
  it('empties the box where the queued send actually dispatches', () => {
    const fn = promptSource.match(/function sendQueuedAfterUpload\([\s\S]*?\n  \}/);
    expect(fn, 'sendQueuedAfterUpload() not found').not.toBeNull();
    const body = fn![0];
    const clearIdx = body.indexOf("writeComposerValue(el, '')");
    const dispatchIdx = body.indexOf('beginSend');
    expect(clearIdx).toBeGreaterThan(-1);
    expect(dispatchIdx).toBeGreaterThan(clearIdx);
    // And ONLY when the mounted box is this thread's. The retry is per thread,
    // so it can fire after the reader has moved on. Clearing then would empty
    // somebody else's draft off the screen.
    expect(body).toContain('el.dataset.threadId === threadId');
    // Past every branch that CANCELS the send: those must leave the box alone.
    for (const cancelled of ['clearSubmittingThread(threadId)']) {
      expect(body.lastIndexOf(cancelled)).toBeLessThan(clearIdx);
    }
  });

  it('moves a queued upload send into the normal Cancel morph', () => {
    expect(promptSource).toMatch(/const\s+uploadSendQueued\s*=\s*focusedTid\s*\?\s*queuedUploadSends\.value\.has\(focusedTid\)\s*:\s*false/);
    expect(promptSource).toContain('const morphHasContent = hasContent && !hasPendingMultiQ && !uploadSendQueued');
    expect(promptSource).toMatch(/if\s*\(\s*queuedUploadSends\.value\.has\(targetId\)\s*\)\s*\{[\s\S]*?clearQueuedUploadSend\(targetId\)/);
    expect(promptSource).not.toContain('Will send after image upload');
  });
});
