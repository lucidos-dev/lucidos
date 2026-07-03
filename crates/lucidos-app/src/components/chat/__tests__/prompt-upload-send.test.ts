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

  it('moves a queued upload send into the normal Cancel morph', () => {
    expect(promptSource).toMatch(/const\s+uploadSendQueued\s*=\s*focusedTid\s*\?\s*queuedUploadSends\.value\.has\(focusedTid\)\s*:\s*false/);
    expect(promptSource).toContain('const morphHasContent = hasContent && !hasPendingMultiQ && !uploadSendQueued');
    expect(promptSource).toMatch(/if\s*\(\s*queuedUploadSends\.value\.has\(targetId\)\s*\)\s*\{[\s\S]*?clearQueuedUploadSend\(targetId\)/);
    expect(promptSource).not.toContain('Will send after image upload');
  });
});
