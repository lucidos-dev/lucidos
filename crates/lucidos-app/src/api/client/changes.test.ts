import { beforeEach, expect, it, vi } from 'vitest';

vi.mock('./_core', () => ({
  API: '/api/v1',
  json: vi.fn(),
  text: vi.fn(),
  mutatingFetch: vi.fn(),
  throwIfNotOk: vi.fn(),
}));

import { mutatingFetch, throwIfNotOk } from './_core';
import { rebuildEngine } from './changes';

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(mutatingFetch).mockResolvedValue(new Response(null, { status: 202 }));
});

it('joins an existing build for the pending-version action', async () => {
  await rebuildEngine('join');
  expect(mutatingFetch).toHaveBeenCalledWith('/api/v1/engine/rebuild?mode=join', { method: 'POST' });
  expect(throwIfNotOk).toHaveBeenCalled();
});

it('preserves force rebuild for existing callers and explicit restart', async () => {
  await rebuildEngine();
  await rebuildEngine('restart');
  expect(mutatingFetch).toHaveBeenCalledTimes(2);
  expect(mutatingFetch).toHaveBeenLastCalledWith('/api/v1/engine/rebuild', { method: 'POST' });
});

it('propagates a failed build request', async () => {
  vi.mocked(throwIfNotOk).mockRejectedValueOnce(new Error('request failed'));
  await expect(rebuildEngine('join')).rejects.toThrow('request failed');
});
