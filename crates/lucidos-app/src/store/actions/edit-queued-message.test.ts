// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const { refreshThreadEvents } = vi.hoisted(() => ({ refreshThreadEvents: vi.fn(async (_id: string) => {}) }));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  refreshThreadEvents,
}));

import { editQueuedMessage, removeQueuedMessage } from './chat';
import { removingQueuedMessageIds, toasts } from '../store';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';

const originalFetch = globalThis.fetch;

function answerRemoveWith(response: () => Response): string[] {
  const removed: string[] = [];
  globalThis.fetch = vi.fn(async (url: string, init?: { body?: string }) => {
    if (url.includes('/chat/queued-message/remove')) {
      removed.push(JSON.parse(init?.body ?? '{}').message_id);
      return response();
    }
    return new Response(null, { status: 200 });
  }) as unknown as typeof fetch;
  return removed;
}

const message = { id: 'm2', text: 'also check the totals', imageHashes: ['hash-b'] };

describe('editQueuedMessage', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    _resetComposeDraftsForTesting();
    removingQueuedMessageIds.value = new Set();
    toasts.value = [];
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    _resetComposeDraftsForTesting();
    toasts.value = [];
  });

  it('takes the message back, then puts it in compose after the draft', async () => {
    setDraft('t-1', { text: 'half a thought', image_hashes: ['hash-a'], mode: null });
    const removed = answerRemoveWith(() => new Response(null, { status: 200 }));

    await editQueuedMessage('t-1', message);

    expect(removed).toEqual(['m2']);
    expect(getDraft('t-1')).toMatchObject({
      image_hashes: ['hash-a', 'hash-b'],
      text: 'half a thought\n\nalso check the totals',
    });
    expect(toasts.value).toEqual([]);
  });

  it('leaves compose alone and says why when the agent already read it', async () => {
    setDraft('t-1', { text: 'half a thought', image_hashes: [], mode: null });
    answerRemoveWith(() => new Response(
      JSON.stringify({
        error: 'The coding agent has already read this message, so it can no longer be taken back.',
        reason: 'already_read',
      }),
      { status: 409, headers: { 'Content-Type': 'application/json' } },
    ));

    await editQueuedMessage('t-1', message);

    expect(getDraft('t-1')).toMatchObject({ text: 'half a thought', image_hashes: [] });
    expect(refreshThreadEvents).toHaveBeenCalledWith('t-1');
    const shown = JSON.stringify(toasts.value);
    expect(shown).toContain('Failed to edit queued message');
    expect(shown).toContain('already read this message');
  });
});

describe('removeQueuedMessage', () => {
  beforeEach(() => {
    removingQueuedMessageIds.value = new Set();
    toasts.value = [];
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    toasts.value = [];
  });

  it('takes back a message a dead agent process held, with nothing to say', async () => {
    const removed = answerRemoveWith(() => new Response(null, { status: 200 }));

    await removeQueuedMessage('t-1', 'm2');

    expect(removed).toEqual(['m2']);
    expect(toasts.value).toEqual([]);
    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('says why and re-syncs when the session is still winding down', async () => {
    answerRemoveWith(() => new Response(
      JSON.stringify({
        error: "The coding agent's session is still winding down. Remove the message again in a moment.",
        reason: 'withdraw_refused',
      }),
      { status: 409, headers: { 'Content-Type': 'application/json' } },
    ));

    await removeQueuedMessage('t-1', 'm2');

    expect(refreshThreadEvents).toHaveBeenCalledWith('t-1');
    const shown = JSON.stringify(toasts.value);
    expect(shown).toContain('Failed to remove queued message');
    expect(shown).toContain('still winding down');
  });
});
