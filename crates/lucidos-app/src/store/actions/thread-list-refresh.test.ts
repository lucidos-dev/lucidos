import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, showToast, THREAD_LIST_REFRESH_TOAST_KEY } from '../store';
import { ApiError } from '../../api/client';

const mockLoadAllThreads = vi.fn();
vi.mock('./thread-loading', () => ({
  loadAllThreads: () => mockLoadAllThreads(),
}));

const { refreshThreadList } = await import('./thread-list-refresh');

function card() {
  return toasts.value.find(t => t.key === THREAD_LIST_REFRESH_TOAST_KEY);
}

beforeEach(() => {
  vi.clearAllMocks();
  toasts.value = [];
  connectionStatus.value = 'connected';
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

describe('refreshThreadList: a delivery failure is not a verdict', () => {
  it('stays silent on a TimeoutError', async () => {
    mockLoadAllThreads.mockRejectedValue(new DOMException('Request timed out after 10000ms', 'TimeoutError'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });

  it('stays silent on an AbortError (the browser cancelling an in-flight fetch)', async () => {
    mockLoadAllThreads.mockRejectedValue(new DOMException('Fetch is aborted', 'AbortError'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });

  it('stays silent on a transport TypeError (Safari on a stale connection)', async () => {
    mockLoadAllThreads.mockRejectedValue(new TypeError('Load failed'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });
});

describe('refreshThreadList: a verdict still reaches the user', () => {
  it('raises exactly one keyed card for an ApiError while connected', async () => {
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'Failed to get saved threads'));
    await refreshThreadList();
    expect(toasts.value).toHaveLength(1);
    expect(card()!.type).toBe('error');
    expect(card()!.message).toContain('Failed to get saved threads');
  });

  it('raises a card for a parse error (the engine answered, the answer was unusable)', async () => {
    mockLoadAllThreads.mockRejectedValue(new SyntaxError('Unexpected token < in JSON at position 0'));
    await refreshThreadList();
    expect(card()).toBeDefined();
  });

  it('replaces its own card rather than stacking, across repeated failures', async () => {
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'boom'));
    await refreshThreadList();
    await refreshThreadList();
    await refreshThreadList();
    expect(toasts.value).toHaveLength(1);
  });
});

describe('refreshThreadList: the connection dot owns a sustained outage', () => {
  it('raises nothing while disconnected, even for a verdict', async () => {
    connectionStatus.value = 'disconnected';
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });

  it('raises nothing while connecting', async () => {
    connectionStatus.value = 'connecting';
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });
});

describe('refreshThreadList: a landed refresh retracts the card', () => {
  it('removes the card once a refresh succeeds', async () => {
    mockLoadAllThreads.mockRejectedValueOnce(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeDefined();

    mockLoadAllThreads.mockResolvedValueOnce(true);
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });

  it('leaves unrelated toasts alone when it retracts', async () => {
    showToast('Engine restarted', 'success', { key: 'unrelated' });
    mockLoadAllThreads.mockResolvedValue(true);
    await refreshThreadList();
    expect(toasts.value.map(t => t.key)).toEqual(['unrelated']);
  });
});

describe('refreshThreadList: never rejects', () => {
  it('resolves on a verdict', async () => {
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'boom'));
    await expect(refreshThreadList()).resolves.toBeUndefined();
  });

  it('resolves on a transient rejection', async () => {
    mockLoadAllThreads.mockRejectedValue(new DOMException('x', 'TimeoutError'));
    await expect(refreshThreadList()).resolves.toBeUndefined();
  });
});

describe('refreshThreadList: a declined load is not a landed one', () => {
  it('leaves the card standing when the load declined', async () => {
    mockLoadAllThreads.mockRejectedValueOnce(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeDefined();

    mockLoadAllThreads.mockResolvedValueOnce(false);
    await refreshThreadList();
    expect(card()).toBeDefined();
  });
});
