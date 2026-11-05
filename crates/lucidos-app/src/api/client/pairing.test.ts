import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';

import { pairDesktopWindow, redeemPairingCode } from './pairing';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';

vi.mock('../../utils/platform', () => ({ isTauri: vi.fn() }));
vi.mock('../../utils/tauri', () => ({ invoke: vi.fn() }));

const mockIsTauri = vi.mocked(isTauri);
const mockInvoke = vi.mocked(invoke);

function okFetch() {
  return vi.fn().mockResolvedValue(new Response(JSON.stringify({ paired: true }), { status: 200 }));
}

describe('redeemPairingCode', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('posts the code same-origin, so the HttpOnly cookie lands in this jar', async () => {
    const fetchMock = okFetch();
    vi.stubGlobal('fetch', fetchMock);
    await redeemPairingCode('12345678', 'My iPhone');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/~/api/v1/auth/pair');
    expect(init.credentials).toBe('same-origin');
    expect(JSON.parse(init.body)).toEqual({ code: '12345678', label: 'My iPhone' });
  });

  it('rejects with the gateway\'s own sentence, not its status text', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ error: 'that pairing code is not valid or has expired' }), {
          status: 400,
        }),
      ),
    );
    await expect(redeemPairingCode('00000000')).rejects.toThrow(/not valid or has expired/);
  });
});

describe('pairDesktopWindow', () => {
  beforeEach(() => {
    mockIsTauri.mockReset();
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('mints through the Rust side and redeems in the page', async () => {
    // The split is the point. Redeeming in Rust would put the credential in an
    // HTTP client that authorizes nothing the user can see.
    mockIsTauri.mockReturnValue(true);
    mockInvoke.mockResolvedValue({ code: '12345678', expires_in_secs: 300 });
    const fetchMock = okFetch();
    vi.stubGlobal('fetch', fetchMock);

    await pairDesktopWindow();

    expect(mockInvoke).toHaveBeenCalledWith('mint_pairing_code');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/~/api/v1/auth/pair');
    expect(JSON.parse(init.body)).toEqual({ code: '12345678', label: 'Lucidos desktop' });
  });

  it('rejects off Tauri without reaching for a bridge that is not there', async () => {
    mockIsTauri.mockReturnValue(false);
    const fetchMock = okFetch();
    vi.stubGlobal('fetch', fetchMock);
    await expect(pairDesktopWindow()).rejects.toThrow(/desktop app/);
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('never posts an empty code when the mint came back without one', async () => {
    mockIsTauri.mockReturnValue(true);
    const fetchMock = okFetch();
    vi.stubGlobal('fetch', fetchMock);
    for (const minted of [{}, { code: '' }, { code: '   ' }, null]) {
      mockInvoke.mockResolvedValue(minted);
      await expect(pairDesktopWindow()).rejects.toThrow(/no pairing code/);
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('surfaces a refused mint rather than falling silent', async () => {
    // The caller shows the typed form on a rejection, so swallowing it here
    // would leave the window with no pairing surface at all.
    mockIsTauri.mockReturnValue(true);
    mockInvoke.mockRejectedValue(new Error('the gateway on port 5252 did not mint a pairing code'));
    vi.stubGlobal('fetch', okFetch());
    await expect(pairDesktopWindow()).rejects.toThrow(/did not mint/);
  });
});
