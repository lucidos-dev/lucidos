/** A repositories read that fails TRANSIENTLY must not park the Loadable on
 *  `failed`. Nothing retries it: the compose destination picker's render-path
 *  kick-off only fires on `not-loaded`, and the SSE refresh only re-fires a list
 *  that is already `loaded` — so the packaged app painted a permanent red
 *  "Failed to load repositories / Fetch is aborted" row, and the user's only
 *  escape was mutating the repository list by hand (which refetches).
 *
 *  Two guarantees pinned here: one retry on a transient rejection, and a
 *  human-readable reason (not the raw WebKit string) when it genuinely fails. */

import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { repositories } from '../store';

vi.mock('../../api/client', async (importActual) => {
  const actual = await importActual<typeof import('../../api/client')>();
  return { ...actual, json: vi.fn() };
});

import { json, ApiError } from '../../api/client';
import { loadRepositories } from './repositoriesLoader';

const read = json as unknown as Mock;

describe('loadRepositories — transient failures retry, real ones surface', () => {
  beforeEach(() => {
    read.mockReset();
    repositories.value = { status: 'not-loaded' };
  });

  it('retries once when the browser cancels the fetch mid-flight', async () => {
    read
      .mockRejectedValueOnce(new DOMException('Fetch is aborted', 'AbortError'))
      .mockResolvedValueOnce([{ id: 'r1', name: 'my-project', path: '/repos/my-project' }]);

    await loadRepositories();

    expect(read).toHaveBeenCalledTimes(2);
    expect(repositories.value.status).toBe('loaded');
  });

  it('retries once when our own deadline fires (engine still booting)', async () => {
    read
      .mockRejectedValueOnce(new DOMException('Request timed out', 'TimeoutError'))
      .mockResolvedValueOnce([]);

    await loadRepositories();

    expect(read).toHaveBeenCalledTimes(2);
    expect(repositories.value).toEqual({ status: 'loaded', data: [] });
  });

  it('does NOT retry a real backend failure, and keeps the engine reason', async () => {
    read.mockRejectedValue(new ApiError(500, 'Failed to list repositories: DB error'));

    await loadRepositories();

    expect(read).toHaveBeenCalledTimes(1);
    expect(repositories.value).toMatchObject({
      status: 'failed',
      error: 'Failed to list repositories: DB error',
      httpCode: 500,
    });
  });

  it('parks on failed with a readable reason — never the raw WebKit abort string', async () => {
    read.mockRejectedValue(new DOMException('Fetch is aborted', 'AbortError'));

    await loadRepositories();

    expect(read).toHaveBeenCalledTimes(2);
    expect(repositories.value).toEqual({ status: 'failed', error: 'request cancelled' });
  });
});
