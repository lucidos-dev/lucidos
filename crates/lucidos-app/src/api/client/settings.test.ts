import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { getCredentialValue, handOverDevice } from './settings';
import { DEVICE_ID_KEY } from '../../utils/deviceIdHeader';

/**
 * The hand-over's subject is a change of identity. So it must claim the id it
 * is adopting, rather than the one it still stores.
 *
 * `fetchWithDefaults` stamps `x-lucidos-device-id` from `localStorage`, which
 * still holds the OLD id here by design: nothing is written until the row has
 * moved. Left to the default the request contradicts its own body, and the
 * engine refuses it. An up-to-date gateway replaces the header with the
 * authenticated id, so the override changes nothing there. It is what carries
 * the migration everywhere else. See
 * `docs/plans/2026-08-22-device-hand-over-must-not-need-a-fresh-gateway.md`.
 */
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
    // The engine's guard is `asserted == device_id` (`foreign_hand_over`), so
    // the two values this request sends must never drift apart.
    await handOverDevice('before-reinstall', 'paired-again');

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['x-lucidos-device-id']).toBe(JSON.parse(init.body).device_id);
  });
});

/**
 * Regression: a spent reveal token must not surface as a failed Copy.
 *
 * The reveal is two steps and the token is one-shot (ADR 0117). The service
 * worker re-issues a `GET` whose response was lost (`fetchWithRetry` in
 * `public/sw.js`), and the server already redeemed the token on the attempt
 * that vanished. Without the retry here, the mechanism that exists to rescue a
 * flaky connection would turn one into a hard failure instead.
 */
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
    // Mint, then mint again. The first 403 is the mint's own refusal, so the
    // read never runs on either attempt.
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});
