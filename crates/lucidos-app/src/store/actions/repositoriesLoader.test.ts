
import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { repositories } from '../store';

vi.mock('../../api/client', async (importActual) => {
  const actual = await importActual<typeof import('../../api/client')>();
  return { ...actual, json: vi.fn() };
});

import { json, ApiError } from '../../api/client';
import { loadRepositories, refreshRepositories } from './repositoriesLoader';
import { joinSliceLoadsDuring } from '../bootstrapSliceLoads';

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

describe('refreshRepositories answers with a request sent after the call', () => {
  beforeEach(() => {
    read.mockReset();
    repositories.value = { status: 'not-loaded' };
  });

  it('waits out a load already in flight, then sends its own', async () => {
    let answerFirst: (repos: unknown) => void = () => {};
    read
      .mockImplementationOnce(() => new Promise(resolve => { answerFirst = resolve; }))
      .mockResolvedValueOnce([{ id: 'r2', name: 'added-later', path: '/repos/added-later' }]);
    const earlier = loadRepositories();

    const refreshed = refreshRepositories();
    answerFirst([]);
    await earlier;
    await refreshed;

    expect(read).toHaveBeenCalledTimes(2);
    expect(repositories.value).toMatchObject({ status: 'loaded', data: [{ id: 'r2' }] });
  });

  it('sends one request when nothing is in flight', async () => {
    read.mockResolvedValueOnce([]);

    await refreshRepositories();

    expect(read).toHaveBeenCalledTimes(1);
  });
});

describe('loadRepositories inside a bootstrap gap repair', () => {
  beforeEach(() => {
    (json as Mock).mockReset();
    repositories.value = { status: 'not-loaded' };
  });

  it('joins the bootstrap without sharing that join with a later call', async () => {
    let finishBootstrap = () => {};
    const bootstrap = new Promise<void>((resolve) => { finishBootstrap = resolve; });
    let joined: Promise<void> | undefined;
    joinSliceLoadsDuring(bootstrap, () => { joined = loadRepositories(); });
    expect(joined).toBe(bootstrap);
    expect(json).not.toHaveBeenCalled();

    (json as Mock).mockResolvedValueOnce([]);
    await loadRepositories();
    expect(repositories.value).toEqual({ status: 'loaded', data: [] });
    finishBootstrap();
  });
});
