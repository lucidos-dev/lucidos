import { afterEach, expect, it, vi } from 'vitest';

vi.mock('../../utils/basePath', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../utils/basePath')>(),
  REMOTE_GATEWAY: 'peer',
  withBase: (path: string) => `/work@peer${path}`,
}));

afterEach(() => {
  vi.doUnmock('virtual:build-id');
  vi.unstubAllGlobals();
});

async function sync(runningBuild: string, servedBuild: string) {
  vi.resetModules();
  vi.doMock('virtual:build-id', () => ({ CLIENT_BUILD_ID: runningBuild }));
  vi.stubGlobal('navigator', {});
  vi.stubGlobal('fetch', vi.fn(async (url: string) => {
    if (url === '/~/sw.js') return new Response("const BUILD_ID = 'entry-build';");
    if (url === '/work@peer/') {
      return new Response(`<meta name="lucidos-build-id" content="${servedBuild}">`);
    }
    throw new Error(`Unexpected request: ${url}`);
  }));

  const { preferences, updateAvailable, toasts } = await import('../store');
  const { syncClientUpdateFromBuild } = await import('./client-update');
  preferences.value = { status: 'loaded', data: {} };
  await syncClientUpdateFromBuild();
  return {
    badge: updateAvailable.value,
    toast: toasts.value.some((toast) => toast.key === 'update-available'),
  };
}

it('keeps the update badge and toast absent when the peer shell matches the running build', async () => {
  expect(await sync('peer-build', 'peer-build')).toEqual({ badge: false, toast: false });
});

it('shows the update until reload runs the build served by the peer shell', async () => {
  expect(await sync('peer-build', 'newer-build')).toEqual({ badge: true, toast: true });
  expect(await sync('newer-build', 'newer-build')).toEqual({ badge: false, toast: false });
});
