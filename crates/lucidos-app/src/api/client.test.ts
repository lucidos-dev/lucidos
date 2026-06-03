import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cancelChat, checkHealth, fetchCCCommands, submitChat } from './client';

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
    expect(url1).toContain('/api/v1/chat/cancel');
    expect(url1).toBe(url2);
    expect(init1.method).toBe('POST');
    expect(init2.method).toBe('POST');
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

describe('fetchCCCommands url construction', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    }), { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('passes repo_id="" explicitly when repoId is empty string (default Lucidos)', async () => {
    await fetchCCCommands(undefined, '');
    const [url] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/claude-code/commands?repo_id=');
  });

  it('passes repo_id when a specific repoId is given', async () => {
    await fetchCCCommands(undefined, 'repo-uuid-123');
    const [url] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/claude-code/commands?repo_id=repo-uuid-123');
  });

  it('omits repo_id when repoId is undefined (no compose context)', async () => {
    await fetchCCCommands('thread-abc');
    const [url] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/claude-code/commands?thread_id=thread-abc');
  });

  it('passes both thread_id and repo_id when both are given', async () => {
    await fetchCCCommands('thread-abc', 'repo-uuid-123');
    const [url] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/claude-code/commands?thread_id=thread-abc&repo_id=repo-uuid-123');
  });

  it('omits the query string entirely when neither argument is given', async () => {
    await fetchCCCommands();
    const [url] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/claude-code/commands');
  });
});

// The watchdog needs to tell a transport failure (engine unreachable, no
// httpCode) from an HTTP failure (engine answered non-2xx, httpCode present)
// to pick the right recovery action — both used to collapse to `null`.
describe('checkHealth returns a discriminated Loadable shape', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('returns failed (no httpCode) when fetch throws a transport error', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    const result = await checkHealth();

    expect(result.status).toBe('failed');
    if (result.status === 'failed') {
      expect(result.httpCode).toBeUndefined();
      expect(result.error).toMatch(/Failed to fetch/i);
    }
  });

  it('returns failed with httpCode when the engine returns a 5xx', async () => {
    mockFetch.mockResolvedValueOnce(new Response('boom', { status: 503 }));

    const result = await checkHealth();

    expect(result.status).toBe('failed');
    if (result.status === 'failed') {
      expect(result.httpCode).toBe(503);
    }
  });

  it('returns loaded with the parsed body when the engine answers 200', async () => {
    const body = {
      status: 'ok',
      workspace: 'dev',
      workspace_path: '/tmp/dev',
      started_at: '2026-05-16T00:00:00Z',
    };
    mockFetch.mockResolvedValueOnce(new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }));

    const result = await checkHealth();

    expect(result.status).toBe('loaded');
    if (result.status === 'loaded') {
      expect(result.data.workspace).toBe('dev');
      expect(result.data.workspace_path).toBe('/tmp/dev');
      expect(result.data.started_at).toBe('2026-05-16T00:00:00Z');
    }
  });

  it('transport failure and HTTP failure are distinguishable to the caller', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    const transport = await checkHealth();

    mockFetch.mockResolvedValueOnce(new Response('', { status: 500 }));
    const http = await checkHealth();

    expect(transport.status).toBe('failed');
    expect(http.status).toBe('failed');
    if (transport.status === 'failed' && http.status === 'failed') {
      expect(transport.httpCode).toBeUndefined();
      expect(http.httpCode).toBe(500);
    }
  });
});
