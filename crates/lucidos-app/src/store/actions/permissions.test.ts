import { describe, it, expect, beforeEach, vi } from 'vitest';

const postMcpConsent = vi.fn(async () => {});
const postCommandConsent = vi.fn(async () => {});

vi.mock('../../api/client', () => ({ postMcpConsent, postCommandConsent }));

const { resolveCodingAgentPermission, resolveCommandPermission } = await import('./permissions');

/** Resolving a permission card posts the consent and nothing else.
 *
 *  These assertions are inverted from what they were. All three cards used to
 *  force the transcript to the bottom BEFORE the POST, so that the agent's
 *  resumed stream would tail; the tests pinned the ordering of the two. The
 *  scroll is gone (the app does not decide where the reader looks), so the
 *  ordering has nothing to order.
 *
 *  Its absence is pinned by a source scan rather than here. This file mocked
 *  `scrollState` and asserted the mock was never called, which could not fail
 *  once `permissions.ts` stopped importing the module at all: an unimported mock
 *  is never applied. The scan in
 *  `components/chat/__tests__/scroll-follow-the-live-edge.test.ts` reads every
 *  module under `store/actions` and fails on any unsanctioned reach for the live
 *  edge, this one included, which is a guard that can actually go red. */
describe('resolveCodingAgentPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('POSTs consent without moving the transcript', async () => {
    await resolveCodingAgentPermission('req-1', true, 'session');

    expect(postMcpConsent).toHaveBeenCalledWith('req-1', true, 'session');
  });

  it('forwards a bare allow (no persist scope)', async () => {
    await resolveCodingAgentPermission('req-2', true);
    expect(postMcpConsent).toHaveBeenCalledWith('req-2', true, undefined);
  });

  it('propagates a rejected POST to the caller, still without scrolling', async () => {
    // The card's decide() rolls back its optimistic state and toasts.
    postMcpConsent.mockRejectedValueOnce(new Error('boom'));
    await expect(resolveCodingAgentPermission('req-3', false)).rejects.toThrow('boom');
  });
});

describe('resolveCommandPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('POSTs command consent, and only that', async () => {
    await resolveCommandPermission('req-4', true, 'narrow');

    expect(postCommandConsent).toHaveBeenCalledWith('req-4', true, 'narrow');
    expect(postMcpConsent).not.toHaveBeenCalled();
  });
});
