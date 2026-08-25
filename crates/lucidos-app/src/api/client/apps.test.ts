import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fetchEventTypes } from './apps';
import { engineRestarting } from '../../store/store';

// The trigger event-type picker caches whatever verdict this read produces. So
// one dropped fetch used to paint "Failed to load event types" until the panel
// was reopened. `retryTransientRead` is what stops a blip becoming a verdict.

const originalFetch = globalThis.fetch;

function okJson(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('fetchEventTypes', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    engineRestarting.value = false;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    engineRestarting.value = false;
    vi.restoreAllMocks();
  });

  it('reads the event-type list from the engine', async () => {
    mockFetch.mockResolvedValue(okJson(['DeployFinished', 'OuraSleepImported']));

    await expect(fetchEventTypes()).resolves.toEqual(['DeployFinished', 'OuraSleepImported']);
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(String(mockFetch.mock.calls[0][0])).toContain('/api/v1/events/types');
  });

  it('retries a dropped connection and resolves with the second attempt', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(okJson(['DeployFinished']));

    await expect(fetchEventTypes()).resolves.toEqual(['DeployFinished']);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('retries a fired client deadline too', async () => {
    mockFetch
      .mockRejectedValueOnce(new DOMException('timed out', 'TimeoutError'))
      .mockResolvedValueOnce(okJson([]));

    await expect(fetchEventTypes()).resolves.toEqual([]);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('does not retry an engine failure, which is a real verdict', async () => {
    mockFetch.mockResolvedValue(
      new Response(JSON.stringify({ error: 'catalog query failed' }), { status: 500 }),
    );

    await expect(fetchEventTypes()).rejects.toThrow('catalog query failed');
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('surfaces the second failure when the retry fails too', async () => {
    mockFetch.mockRejectedValue(new TypeError('Load failed'));

    await expect(fetchEventTypes()).rejects.toThrow('Load failed');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});
