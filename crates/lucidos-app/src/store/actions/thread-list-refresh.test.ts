/**
 * Regression: an installed iOS PWA on cellular showed a sticky
 * `Failed to refresh the thread list: request timed out` card. The gateway log
 * for that window has the engine answering every `GET /api/v1/threads` between
 * 12ms and 2s against a 10s client deadline, so the ten seconds went into the
 * link (5G, a Tailscale tunnel, one HTTP/2 connection shared with the event
 * stream) and the card was reporting the link while blaming the engine.
 *
 * Both surfacing sites (the resume sync in connection.ts, the SSE resync in
 * thread-sync.ts) suppressed `AbortError` and transport errors but deliberately
 * let `TimeoutError` through. These tests pin the reversal, and the three other
 * properties the shared wrapper now owns: the connection gate, the retraction on
 * success, and never rejecting.
 */
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
  // The reported shape. `AbortSignal.timeout` fires this, and `_core.ts`
  // re-stamps WebKit's own AbortError as it so the deadline means the same thing
  // on every engine.
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

  // `'connecting'` is the value before the first health probe lands, so
  // reachability is unconfirmed rather than confirmed-good.
  it('raises nothing while connecting', async () => {
    connectionStatus.value = 'connecting';
    mockLoadAllThreads.mockRejectedValue(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeUndefined();
  });
});

describe('refreshThreadList: a landed refresh retracts the card', () => {
  // The card is keyed with no `autoDismissMs`, which `scheduleAutoDismiss`
  // treats as "never expire", so without this it stood above an up-to-date
  // drawer until the user tapped the X.
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
  // `resyncLoadedThreads` awaits it and then refreshes the focused thread, which
  // is what clears a stuck "Thinking" spinner after an SSE gap. A rejection
  // propagating out of here would skip that repair, which is the very failure
  // that function exists to perform.
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
  // `loadAllThreads` resolves FALSE when it declined: the engine is mid-restart,
  // or another load is already in flight. Nothing was read either way, so the
  // card's claim is untouched. Flagged independently by two reviewers on
  // 2026-08-07, when the wrapper retracted on any resolve.
  it('leaves the card standing when the load declined', async () => {
    mockLoadAllThreads.mockRejectedValueOnce(new ApiError(500, 'boom'));
    await refreshThreadList();
    expect(card()).toBeDefined();

    mockLoadAllThreads.mockResolvedValueOnce(false);
    await refreshThreadList();
    expect(card()).toBeDefined();
  });
});
