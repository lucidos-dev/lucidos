import { afterEach, expect, it, vi } from 'vitest';
import { createRecoveredDraft } from './threads';
import { sha256Hex } from '../../utils/sha256Hex';
vi.mock('../../utils/sha256Hex', () => ({ sha256Hex: vi.fn(async () => 'b'.repeat(64)) }));

const originalFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = originalFetch; vi.restoreAllMocks(); });

it('creates recovered content and selections together, fenced by generation', async () => {
  const fetch = vi.fn().mockResolvedValue(new Response('{}'));
  globalThis.fetch = fetch;
  await createRecoveredDraft('fresh', 'generation', {
    threadId: 'archived',
    draft: { text: 'keep', image_hashes: ['a'.repeat(64)], mode: 'lucidos' },
    selection: { model: 'example-model' },
  });
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({
    id: 'fresh', mode: 'lucidos',
    recovered_compose: { generation: 'generation', text: 'keep',
      image_hashes: ['a'.repeat(64)], selection: { model: 'example-model' } },
  });
});

it('leaves a lost response to durable recovery, without a transport retry', async () => {
  const fetch = vi.fn().mockRejectedValue(new TypeError('Load failed'));
  globalThis.fetch = fetch;
  await expect(createRecoveredDraft('fresh', 'generation', {
    threadId: 'archived', draft: { text: 'keep', image_hashes: [], mode: 'lucidos' }, selection: {},
  })).rejects.toThrow('Load failed');
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('transports pending bytes and deduplicates landed hashes in the atomic create', async () => {
  const fetch = vi.fn().mockResolvedValue(new Response('{}'));
  globalThis.fetch = fetch;
  await createRecoveredDraft('fresh', 'generation', {
    threadId: 'archived', draft: { text: 'with image', image_hashes: ['b'.repeat(64)], mode: null }, selection: {},
    uploads: [{ localId: 'image', threadId: 'archived', name: 'image.png', mime: 'image/png',
      bytes: new Uint8Array([1]).buffer, size: 1, createdAt: 0, ownerId: 'old-page', landedHash: 'b'.repeat(64) }],
  });
  expect(fetch).toHaveBeenCalledTimes(1);
  const body = JSON.parse(fetch.mock.calls[0][1].body);
  expect(body.recovered_images).toEqual([{ hash: 'b'.repeat(64), base64: 'AQ==' }]);
  expect(body.recovered_compose.image_hashes).toEqual(['b'.repeat(64)]);
});

it('refuses corrupt retained bytes before posting or acknowledging a draft', async () => {
  const fetch = vi.fn();
  globalThis.fetch = fetch;
  await expect(createRecoveredDraft('fresh', 'generation', {
    threadId: 'archived', draft: { text: '', image_hashes: [], mode: null }, selection: {},
    uploads: [{ localId: 'image', threadId: 'archived', name: 'image.png', mime: 'image/png',
      bytes: new Uint8Array([1]).buffer, size: 1, createdAt: 0, ownerId: 'old-page', landedHash: 'a'.repeat(64) }],
  })).rejects.toThrow('hash mismatch');
  expect(fetch).not.toHaveBeenCalled();
  expect(sha256Hex).toHaveBeenCalled();
});

it('materializes retained inline unsent images without submitting the message', async () => {
  const fetch = vi.fn().mockResolvedValue(new Response('{}'));
  globalThis.fetch = fetch;
  await createRecoveredDraft('fresh', 'generation', {
    threadId: 'unsent:old', draft: { text: 'review first', image_hashes: [], mode: 'lucidos' }, selection: {},
    unsentMessage: { eventId: 'old', threadId: 'archived',
      body: { mode: 'human', message: 'review first', images: [{ base64: 'AQ==', mime_type: 'image/png' }] },
      settlement: { kind: 'raw-new' }, sentAt: '', phase: 'unsent', failedRetries: 0, ownerId: 'old-page' },
  });
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch.mock.calls[0][0]).toMatch(/\/threads$/);
  const body = JSON.parse(fetch.mock.calls[0][1].body);
  expect(body.recovered_images).toEqual([{ hash: 'b'.repeat(64), base64: 'AQ==' }]);
  expect(body.recovered_compose.image_hashes).toEqual(['b'.repeat(64)]);
  expect(body.recovered_compose.text).toBe('review first');
});
