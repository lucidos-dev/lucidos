import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { appUrl, fetchEventTypes, pinAppApi, unpinAppApi } from './apps';
import { parseWidgetParams } from '../../../../../packages/lucidos-sdk/src/params';
import { engineRestarting } from '../../store/store';


const originalFetch = globalThis.fetch;

function okJson(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('appUrl', () => {
  it('builds the bare app URL', () => {
    expect(appUrl('pr-understanding')).toMatch(/\/app\/pr-understanding\/$/);
  });

  it('appends the WIP-preview thread id as a query', () => {
    const url = appUrl('pr-understanding', 'thread-7');
    expect(url).toMatch(/\/app\/pr-understanding\/\?thread_id=thread-7$/);
  });

  it('appends the fragment last, after the query', () => {
    const url = appUrl('pr-understanding', 'thread-7', 'pr-1645');
    expect(url).toMatch(/\/app\/pr-understanding\/\?thread_id=thread-7#pr-1645$/);
  });

  it('appends the fragment with no query when there is no thread', () => {
    const url = appUrl('pr-understanding', undefined, 'pr-1645');
    expect(url).toMatch(/\/app\/pr-understanding\/#pr-1645$/);
  });

  it('keeps a `?` inside the fragment out of the query', () => {
    const url = appUrl('viewer', undefined, 'report?tab=files');
    expect(url).toMatch(/\/app\/viewer\/#report\?tab=files$/);
    expect(url.indexOf('#')).toBeLessThan(url.indexOf('?'));
  });

  it('adds no empty fragment for an empty target', () => {
    expect(appUrl('pr-understanding', undefined, '')).not.toContain('#');
  });

  describe('with a device minted', () => {
    beforeEach(() => localStorage.setItem('lucidos-device-id', 'cfdb4cbfe6044f8b'));
    afterEach(() => localStorage.removeItem('lucidos-device-id'));

    it('carries the device', () => {
      expect(appUrl('pr-understanding')).toContain('?device=cfdb4cbfe6044f8b');
    });

    it('carries it beside the WIP-preview thread, not instead of it', () => {
      const url = appUrl('pr-understanding', 'thread-7');
      expect(url).toContain('thread_id=thread-7');
      expect(url).toContain('device=cfdb4cbfe6044f8b');
    });

    it('carries widget params beside the host keys, and the SDK reads both back', () => {
      const params = { clip: 'artifacts/voices/marin.mp3', thread_id: 'not-a-host-key' };
      const url = new URL(appUrl('lucidos-sound-player', 'thread-7', undefined, params), 'http://h');
      expect(url.searchParams.get('thread_id')).toBe('thread-7');
      expect(url.searchParams.get('device')).toBe('cfdb4cbfe6044f8b');
      expect(parseWidgetParams(url.search)).toEqual(params);
    });

    it('writes one URL for one instance, whatever the key order', () => {
      expect(appUrl('w', undefined, undefined, { b: 1, a: { d: 2, c: 3 } }))
        .toBe(appUrl('w', undefined, undefined, { a: { c: 3, d: 2 }, b: 1 }));
    });

    it('adds no params key for empty params', () => {
      expect(appUrl('w', undefined, undefined, {})).not.toContain('params=');
    });

    it('still puts the fragment last', () => {
      const url = appUrl('pr-understanding', undefined, 'pr-1645');
      expect(url.indexOf('device=')).toBeLessThan(url.indexOf('#'));
      expect(url.endsWith('#pr-1645')).toBe(true);
    });
  });

  it('carries no device before one is minted', () => {
    expect(appUrl('pr-understanding')).not.toContain('device=');
  });
});

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

describe('pinAppApi and unpinAppApi', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    engineRestarting.value = false;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('resolves when the engine reports success', async () => {
    mockFetch.mockImplementation(() => Promise.resolve(okJson({ success: true })));

    await expect(pinAppApi('habit-tracker', 'device-1')).resolves.toEqual({ success: true });
    await expect(unpinAppApi('habit-tracker', 'device-1')).resolves.toEqual({ success: true });
  });

  it('rejects a pin the engine refused, with its reason', async () => {
    mockFetch.mockResolvedValue(okJson({ success: false, error: 'Failed to pin app: pool timed out' }));

    await expect(pinAppApi('habit-tracker', 'device-1')).rejects.toThrow('Failed to pin app: pool timed out');
  });

  it('rejects an unpin the engine refused, naming the app when no reason came', async () => {
    mockFetch.mockResolvedValue(okJson({ success: false }));

    await expect(unpinAppApi('habit-tracker', 'device-1')).rejects.toThrow('habit-tracker');
  });
});
