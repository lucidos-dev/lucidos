import { afterEach, expect, it, vi } from 'vitest';

vi.mock('../utils/basePath', () => ({ REMOTE_GATEWAY: 'peer', withBase: (path: string) => `/work@peer${path}` }));
vi.mock('../store/store', async () => {
  const { signal } = await import('@preact/signals');
  return { preferences: signal({ status: 'not-loaded' }), showToast: vi.fn() };
});
vi.mock('../api/client', () => ({ setPreference: vi.fn() }));
import { getServedBuildId, refreshClient } from './sw-update';

afterEach(() => vi.unstubAllGlobals());

it('reads only the remote route shell when service workers are unavailable', async () => {
  vi.stubGlobal('navigator', {});
  const fetch = vi.fn(async () => new Response('<meta name="lucidos-build-id" content="peer-build">'));
  vi.stubGlobal('fetch', fetch);
  expect(await getServedBuildId()).toBe('peer-build');
  expect(fetch).toHaveBeenCalledExactlyOnceWith('/work@peer/', { cache: 'no-store', headers: { Accept: 'text/html' } });
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
