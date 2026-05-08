import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { focusedThreadId } from '../store';

// jsdom isn't loaded; the test-setup stubs globalThis.{add,remove}EventListener
// but not document/window-scoped versions. Inject minimal copies so the
// presence module's wiring doesn't blow up.
function noopListener() { /* noop */ }
(document as unknown as { addEventListener: typeof noopListener }).addEventListener = noopListener;
(document as unknown as { removeEventListener: typeof noopListener }).removeEventListener = noopListener;
(document as unknown as { hasFocus: () => boolean }).hasFocus = () => true;
Object.defineProperty(document, 'visibilityState', {
  configurable: true,
  get: () => 'visible',
});
(window as unknown as { addEventListener: typeof noopListener }).addEventListener = noopListener;
(window as unknown as { removeEventListener: typeof noopListener }).removeEventListener = noopListener;

const fetchMock = vi.fn().mockResolvedValue({ ok: true });
(globalThis as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;

const { startPresenceTracking, stopPresenceTracking } = await import('./presence');

function lastFetchBody(): { device_id: string; thread_id: string; focused: boolean } | null {
  const calls = fetchMock.mock.calls;
  const call = calls.length > 0 ? calls[calls.length - 1] : null;
  if (!call) return null;
  const body = (call[1] as RequestInit | undefined)?.body;
  return typeof body === 'string' ? JSON.parse(body) : null;
}

function fetchBodies(): Array<{ device_id: string; thread_id: string; focused: boolean }> {
  return fetchMock.mock.calls.map((c) => {
    const body = (c[1] as RequestInit | undefined)?.body;
    return typeof body === 'string' ? JSON.parse(body) : null;
  }).filter(Boolean) as Array<{ device_id: string; thread_id: string; focused: boolean }>;
}

describe('thread presence tracking', () => {
  beforeEach(() => {
    fetchMock.mockClear();
    focusedThreadId.value = null;
  });

  afterEach(() => {
    stopPresenceTracking();
    focusedThreadId.value = null;
  });

  it('emits ThreadFocused when a thread becomes focused', () => {
    startPresenceTracking();
    fetchMock.mockClear();
    focusedThreadId.value = 'thread-a';
    const body = lastFetchBody();
    expect(body).toMatchObject({ thread_id: 'thread-a', focused: true });
    expect(typeof body?.device_id).toBe('string');
  });

  it('emits unfocus then focus when switching threads', () => {
    startPresenceTracking();
    focusedThreadId.value = 'thread-a';
    fetchMock.mockClear();
    focusedThreadId.value = 'thread-b';

    const calls = fetchBodies();
    expect(calls).toHaveLength(2);
    expect(calls[0]).toMatchObject({ thread_id: 'thread-a', focused: false });
    expect(calls[1]).toMatchObject({ thread_id: 'thread-b', focused: true });
  });

  it('emits ThreadUnfocused when focusedThreadId clears', () => {
    startPresenceTracking();
    focusedThreadId.value = 'thread-a';
    fetchMock.mockClear();
    focusedThreadId.value = null;
    expect(lastFetchBody()).toMatchObject({ thread_id: 'thread-a', focused: false });
  });

  it('does NOT re-emit when the same focused state is set repeatedly', () => {
    startPresenceTracking();
    focusedThreadId.value = 'thread-a';
    fetchMock.mockClear();
    // Re-assigning the same value still triggers the effect (signals deliver
    // the new value), but presence module should dedup.
    focusedThreadId.value = 'thread-a';
    focusedThreadId.value = 'thread-a';
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('skips emission entirely when no thread is ever focused', () => {
    startPresenceTracking();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
