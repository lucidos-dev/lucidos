import { afterEach, expect, it, vi } from 'vitest';

vi.mock('../utils/basePath', () => ({ REMOTE_GATEWAY: 'peer', withBase: (path: string) => `/work@peer${path}` }));
vi.mock('../store/store', async () => {
  const { signal } = await import('@preact/signals');
  return { preferences: signal({ status: 'not-loaded' }), showToast: vi.fn() };
});
vi.mock('../api/client', () => ({ setPreference: vi.fn() }));
import { getServedBuildId, refreshClient } from './sw-update';

afterEach(() => vi.unstubAllGlobals());

it('checks A installed generation even when remote workers are unavailable', async () => {
  vi.stubGlobal('navigator', {});
  const fetch = vi.fn(async () => ({ ok: true, text: async () => "const BUILD_ID = 'trusted-a-build';" }));
  vi.stubGlobal('fetch', fetch);
  expect(await getServedBuildId()).toBe('trusted-a-build');
  expect(fetch).toHaveBeenCalledWith('/~/sw.js', { cache: 'no-store' });
});

it('refreshes a remote client without touching a foreign registration or shared caches', () => {
  const reload = vi.fn();
  const getRegistration = vi.fn();
  vi.stubGlobal('navigator', { serviceWorker: { getRegistration } });
  vi.stubGlobal('window', { location: { reload } });
  refreshClient();
  expect(reload).toHaveBeenCalledOnce();
  expect(getRegistration).not.toHaveBeenCalled();
});
