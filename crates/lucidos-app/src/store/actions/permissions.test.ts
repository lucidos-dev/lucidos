import { describe, it, expect, beforeEach, vi } from 'vitest';

const postMcpConsent = vi.fn(async () => {});
const postCommandConsent = vi.fn(async () => {});
const scrollToBottom = vi.fn();

vi.mock('../../api/client', () => ({ postMcpConsent, postCommandConsent }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom }));

const { resolveCodingAgentPermission, resolveCommandPermission } = await import('./permissions');

describe('resolveCodingAgentPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('force-scrolls to the bottom BEFORE POSTing consent so the resumed stream tails', async () => {
    await resolveCodingAgentPermission('req-1', true, 'session');

    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    expect(postMcpConsent).toHaveBeenCalledWith('req-1', true, 'session');
    const scrollOrder = scrollToBottom.mock.invocationCallOrder[0];
    const consentOrder = postMcpConsent.mock.invocationCallOrder[0];
    // Scroll must fire before the await so scrolledUp=false + resize-mode=scroll
    // are set before the answered-state re-render's ResizeObserver fires.
    expect(scrollOrder).toBeLessThan(consentOrder);
  });

  it('forwards a bare allow (no persist scope)', async () => {
    await resolveCodingAgentPermission('req-2', true);
    expect(postMcpConsent).toHaveBeenCalledWith('req-2', true, undefined);
  });

  it('still pins before a rejected POST and propagates the error to the caller', async () => {
    postMcpConsent.mockRejectedValueOnce(new Error('boom'));
    await expect(resolveCodingAgentPermission('req-3', false)).rejects.toThrow('boom');
    // Pin happens before the throw — the card's decide() rolls back + toasts.
    expect(scrollToBottom).toHaveBeenCalledTimes(1);
  });
});

describe('resolveCommandPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('force-scrolls to the bottom BEFORE POSTing command consent', async () => {
    await resolveCommandPermission('req-4', true, 'narrow');

    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    expect(postCommandConsent).toHaveBeenCalledWith('req-4', true, 'narrow');
    expect(postMcpConsent).not.toHaveBeenCalled();
    const scrollOrder = scrollToBottom.mock.invocationCallOrder[0];
    const consentOrder = postCommandConsent.mock.invocationCallOrder[0];
    expect(scrollOrder).toBeLessThan(consentOrder);
  });
});
