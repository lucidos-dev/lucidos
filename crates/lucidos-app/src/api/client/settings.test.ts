import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  confirmBackupKey,
  copyLadderFromChat,
  sortLadder,
  getBackupKey,
  startBackupKeySetup,
  verifyBackupKey,
  getCredentialValue,
  handOverDevice,
  listResolvedFormRequests,
} from './settings';
import { DEVICE_ID_KEY } from '../../utils/deviceIdHeader';

describe('listResolvedFormRequests', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('bounds lookups and combines only proven resolutions', async () => {
    const fetch = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(['resolved-first'])))
      .mockResolvedValueOnce(new Response(JSON.stringify(['resolved-last'])));
    vi.stubGlobal('fetch', fetch);
    expect(await listResolvedFormRequests([])).toEqual([]);
    expect(fetch).not.toHaveBeenCalled();
    const ids = Array.from({ length: 101 }, (_, index) => `request-${index}`);
    expect(await listResolvedFormRequests(ids)).toEqual(['resolved-first', 'resolved-last']);
    expect(fetch.mock.calls.map(([url]) => new URL(url, 'https://example.com').searchParams.get('request_ids')?.split(',')))
      .toEqual([ids.slice(0, 100), ids.slice(100)]);
  });
});

describe('handOverDevice', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ success: true, outcome: 'moved' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    localStorage.setItem(DEVICE_ID_KEY, 'minted-locally');
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    localStorage.removeItem(DEVICE_ID_KEY);
    vi.restoreAllMocks();
  });

  it('asserts the id being adopted, not the one still in storage', async () => {
    await handOverDevice('minted-locally', 'paired-device');

    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toContain('/devices/hand-over');
    expect(init.headers['x-lucidos-device-id']).toBe('paired-device');
    expect(JSON.parse(init.body)).toEqual({
      old_device_id: 'minted-locally',
      device_id: 'paired-device',
    });
  });

  it('sends a header the engine will accept, matching the body it carries', async () => {
    await handOverDevice('before-reinstall', 'paired-again');

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['x-lucidos-device-id']).toBe(JSON.parse(init.body).device_id);
  });
});

describe('getCredentialValue', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  const okJson = (body: unknown) =>
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  const forbidden = () =>
    new Response(JSON.stringify({ error: 'a one-shot reveal token is required' }), {
      status: 403,
      headers: { 'Content-Type': 'application/json' },
    });

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('mints a token, then spends it on the value', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ token: 'tok-1', expires_in_secs: 30 }))
      .mockResolvedValueOnce(okJson({ auth_type: 'api_key', auth_value: 's3cret' }));

    const value = await getCredentialValue('cred-1');

    expect(value).toEqual({ auth_type: 'api_key', auth_value: 's3cret' });
    const [mintUrl, mintInit] = mockFetch.mock.calls[0];
    expect(mintUrl).toContain('/credential-reveal-token?id=cred-1');
    expect(mintInit.method).toBe('POST');
    const [readUrl] = mockFetch.mock.calls[1];
    expect(readUrl).toContain('/credential-value?id=cred-1&token=tok-1');
  });

  it('re-mints once when the token was already spent', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ token: 'spent', expires_in_secs: 30 }))
      .mockResolvedValueOnce(forbidden())
      .mockResolvedValueOnce(okJson({ token: 'fresh', expires_in_secs: 30 }))
      .mockResolvedValueOnce(okJson({ auth_type: 'api_key', auth_value: 's3cret' }));

    const value = await getCredentialValue('cred-1');

    expect(value).toEqual({ auth_type: 'api_key', auth_value: 's3cret' });
    expect(mockFetch).toHaveBeenCalledTimes(4);
    expect(mockFetch.mock.calls[3][0]).toContain('token=fresh');
  });

  it('gives up after one retry, so a real refusal is not a loop', async () => {
    mockFetch.mockResolvedValue(forbidden());

    await expect(getCredentialValue('cred-1')).rejects.toThrow();
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});

describe('the legacy backup key reveal', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  const okJson = (body: unknown) =>
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  const forbidden = () =>
    new Response(JSON.stringify({ error: 'a one-shot reveal token is required' }), {
      status: 403,
      headers: { 'Content-Type': 'application/json' },
    });

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('mints a token, then spends it on the reveal', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ token: 'tok-1', expires_in_secs: 30 }))
      .mockResolvedValueOnce(okJson({ key: 'a2V5' }));

    expect(await getBackupKey()).toEqual({ key: 'a2V5' });

    const [mintUrl, mintInit] = mockFetch.mock.calls[0];
    expect(mintUrl).toContain('/backup/key/reveal-token');
    expect(mintInit.method).toBe('POST');
    const [readUrl, readInit] = mockFetch.mock.calls[1];
    expect(readUrl).toContain('/backup/key?token=tok-1');
    expect(readInit?.method).toBeUndefined();
  });

  it('re-mints once when the token was already spent', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ token: 'spent', expires_in_secs: 30 }))
      .mockResolvedValueOnce(forbidden())
      .mockResolvedValueOnce(okJson({ token: 'fresh', expires_in_secs: 30 }))
      .mockResolvedValueOnce(okJson({ key: 'a2V5' }));

    expect(await getBackupKey()).toEqual({ key: 'a2V5' });
    expect(mockFetch).toHaveBeenCalledTimes(4);
    expect(mockFetch.mock.calls[3][0]).toContain('token=fresh');
  });

  it('gives up after one retry, so a real refusal is not a loop', async () => {
    mockFetch.mockResolvedValue(forbidden());

    await expect(getBackupKey()).rejects.toThrow();
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('lets a 404 through untouched', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ token: 'tok-3', expires_in_secs: 30 }))
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ error: 'This workspace holds no legacy backup key.' }), {
          status: 404,
          headers: { 'Content-Type': 'application/json' },
        }),
      );

    await expect(getBackupKey()).rejects.toMatchObject({ httpCode: 404 });
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});

describe('the backup recipient routes', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  const okJson = (body: unknown) =>
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('starts a setup with one POST and returns the backup key it shows', async () => {
    mockFetch.mockResolvedValueOnce(okJson({ backup_key: 'AGE-SECRET-KEY-1TEST' }));

    expect(await startBackupKeySetup()).toBe('AGE-SECRET-KEY-1TEST');
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toContain('/backup/recipient/setup');
    expect(init.method).toBe('POST');
  });

  it('sends a pasted key in the body only', async () => {
    mockFetch
      .mockResolvedValueOnce(okJson({ recipient: 'age1x', pending: false, legacy_key: false, secs_since_verified: 0 }))
      .mockResolvedValueOnce(okJson({ matches: true }));

    await confirmBackupKey('AGE-SECRET-KEY-1TEST');
    expect(await verifyBackupKey('AGE-SECRET-KEY-1TEST')).toBe(true);

    for (const [url, init] of mockFetch.mock.calls) {
      expect(url).not.toContain('AGE-SECRET-KEY');
      expect(JSON.parse(init.body)).toEqual({ backup_key: 'AGE-SECRET-KEY-1TEST' });
    }
    expect(mockFetch.mock.calls[0][0]).toContain('/backup/recipient/confirm');
    expect(mockFetch.mock.calls[1][0]).toContain('/backup/recipient/verify');
  });
});

describe('copyLadderFromChat', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('posts the target surface and answers the rungs now on', async () => {
    const rungs = [{ model: 'claude-haiku-5-5', effort: 'high' }];
    const mockFetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ rungs }), { status: 200, headers: { 'Content-Type': 'application/json' } }),
    );
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    await expect(copyLadderFromChat('claude-code')).resolves.toEqual({ rungs });
    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toMatch(/\/api\/v1\/intelligence-ladder\/copy-from-chat$/);
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ surface: 'claude-code' });
  });
});

describe('sortLadder', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('posts the surface to sort and answers the rungs now on', async () => {
    const rungs = [{ model: 'claude-haiku-5-5', effort: 'high' }];
    const mockFetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ rungs }), { status: 200, headers: { 'Content-Type': 'application/json' } }),
    );
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    await expect(sortLadder('claude-code')).resolves.toEqual({ rungs });
    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toMatch(/\/api\/v1\/intelligence-ladder\/sort$/);
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ surface: 'claude-code' });
  });
});
