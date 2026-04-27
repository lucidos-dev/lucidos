import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cancelChat, interruptClaudeCode, submitChat } from './client';

// iOS Safari rejects with TypeError("Load failed") when the PWA's HTTP/2
// connection is half-closed (typical after backgrounding). The service worker
// has fetchWithRetry for GETs, but POSTs bypass the SW (body-stream cloning is
// broken on iOS WebKit), so the client must retry transport-layer failures
// itself for idempotent operations.

const originalFetch = globalThis.fetch;

describe('mutating fetch retry on TypeError', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('cancelChat retries once on TypeError("Load failed") and succeeds', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response(null, { status: 200 }));

    await cancelChat('thread-123');

    expect(mockFetch).toHaveBeenCalledTimes(2);
    const [url1, init1] = mockFetch.mock.calls[0];
    const [url2, init2] = mockFetch.mock.calls[1];
    expect(url1).toContain('/api/chat/cancel');
    expect(url1).toBe(url2);
    expect(init1.method).toBe('POST');
    expect(init2.method).toBe('POST');
  });

  it('interruptClaudeCode retries once on TypeError("Load failed") and succeeds', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response(null, { status: 200 }));

    await interruptClaudeCode('thread-123');

    expect(mockFetch).toHaveBeenCalledTimes(2);
    expect(mockFetch.mock.calls[0][0]).toContain('/api/claude-code/interrupt');
  });

  it('cancelChat propagates error if both attempts fail', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));

    await expect(cancelChat('thread-123')).rejects.toThrow('Load failed');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('cancelChat does NOT retry on a non-network error (HTTP 500)', async () => {
    mockFetch.mockResolvedValueOnce(new Response('boom', { status: 500 }));

    await expect(cancelChat('thread-123')).rejects.toThrow();
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('cancelChat does NOT retry on a non-transport TypeError (real programming bug)', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError("Cannot read property 'foo' of undefined"));

    await expect(cancelChat('thread-123')).rejects.toThrow();
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('submitChat does NOT retry on TypeError (not idempotent — would duplicate the message)', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('Load failed'));

    await expect(
      submitChat({ message: 'hi', mode: 'human', event_id: 'evt-1', thread_id: 't-1' }),
    ).rejects.toThrow('Load failed');
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});
