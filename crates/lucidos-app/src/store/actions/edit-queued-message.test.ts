// @vitest-environment jsdom
/**
 * Edit on a queued message takes it back, then puts its text and images in the
 * compose box after any draft. A refused take-back leaves compose alone, so a
 * message the agent already read never also sits in compose. See
 * docs/plans/2026-09-29-withdraw-a-queued-claude-code-message.md.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const { updateComposeSpy } = vi.hoisted(() => ({ updateComposeSpy: vi.fn() }));
vi.mock('./compose', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./compose')>()),
  updateCompose: updateComposeSpy,
}));
const { refreshThreadEvents } = vi.hoisted(() => ({ refreshThreadEvents: vi.fn(async (_id: string) => {}) }));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  refreshThreadEvents,
}));

import { editQueuedMessage } from './chat';
import { removingQueuedMessageIds, toasts } from '../store';
import { setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';

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
    _resetComposeDraftsForTesting();
    removingQueuedMessageIds.value = new Set();
    toasts.value = [];
    updateComposeSpy.mockClear();
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    _resetComposeDraftsForTesting();
    toasts.value = [];
  });

  it('takes the message back, then puts it in compose after the draft', async () => {
    setDraft('t-1', { text: 'half a thought', image_hashes: ['hash-a'], mode: null });
    const removed = answerRemoveWith(() => new Response(null, { status: 200 }));

    await editQueuedMessage('t-1', message);

    expect(removed).toEqual(['m2']);
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { image_hashes: ['hash-a', 'hash-b'] });
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'half a thought\n\nalso check the totals' });
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

    expect(updateComposeSpy).not.toHaveBeenCalled();
    expect(refreshThreadEvents).toHaveBeenCalledWith('t-1');
    const shown = JSON.stringify(toasts.value);
    expect(shown).toContain('Failed to edit queued message');
    expect(shown).toContain('already read this message');
  });
});
