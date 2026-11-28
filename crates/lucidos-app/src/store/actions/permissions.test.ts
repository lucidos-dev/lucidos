import { describe, it, expect, beforeEach, vi } from 'vitest';

const postMcpConsent = vi.fn(async () => {});
const postCommandConsent = vi.fn(async () => {});

vi.mock('../../api/client', () => ({ postMcpConsent, postCommandConsent }));

const { resolveCodingAgentPermission, resolveCommandPermission } = await import('./permissions');

describe('resolveCodingAgentPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('POSTs consent without moving the transcript', async () => {
    await resolveCodingAgentPermission('req-1', true, 'session');

    expect(postMcpConsent).toHaveBeenCalledWith('req-1', true, 'session', undefined);
  });

  it('forwards a bare allow (no persist scope)', async () => {
    await resolveCodingAgentPermission('req-2', true);
    expect(postMcpConsent).toHaveBeenCalledWith('req-2', true, undefined, undefined);
  });

  it('sends Always deny on the coding-agent consent lane', async () => {
    await resolveCodingAgentPermission('req-5', false, undefined, true);
    expect(postMcpConsent).toHaveBeenCalledWith('req-5', false, undefined, true);
  });

  it('propagates a rejected POST to the caller, still without scrolling', async () => {
    postMcpConsent.mockRejectedValueOnce(new Error('boom'));
    await expect(resolveCodingAgentPermission('req-3', false)).rejects.toThrow('boom');
  });
});

describe('resolveCommandPermission', () => {
  beforeEach(() => vi.clearAllMocks());

  it('POSTs command consent, and only that', async () => {
    await resolveCommandPermission('req-4', true, 'narrow');

    expect(postCommandConsent).toHaveBeenCalledWith('req-4', true, 'narrow', undefined);
    expect(postMcpConsent).not.toHaveBeenCalled();
  });
});
