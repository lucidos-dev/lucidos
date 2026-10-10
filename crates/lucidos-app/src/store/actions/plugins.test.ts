import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { InstalledPlugin } from '../types';

const fetchInstalledPlugins = vi.fn();
vi.mock('../../api/client', () => ({ fetchInstalledPlugins }));

const { installedPlugins } = await import('../store');
const { loadInstalledPlugins } = await import('./plugins');

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function plugin(id: string): InstalledPlugin {
  return { id, name: id, version: '1.0.0', content: [], files: [] } as unknown as InstalledPlugin;
}

describe('loadInstalledPlugins', () => {
  beforeEach(() => {
    fetchInstalledPlugins.mockReset();
    installedPlugins.value = { status: 'not-loaded' };
  });

  // Startup, the nav switch and the panel mount can each start a load. The
  // newest request describes the newest state, so an older response landing
  // last must not overwrite it.
  it('keeps the newest request when an older response lands last', async () => {
    const older = deferred<{ plugins: InstalledPlugin[] }>();
    const newer = deferred<{ plugins: InstalledPlugin[] }>();
    fetchInstalledPlugins.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);

    const first = loadInstalledPlugins();
    const second = loadInstalledPlugins();
    newer.resolve({ plugins: [plugin('after-install')] });
    await second;
    older.resolve({ plugins: [] });
    await first;

    expect(installedPlugins.value).toEqual({ status: 'loaded', data: [plugin('after-install')] });
  });

  it('ignores a stale failure behind a newer success', async () => {
    const older = deferred<{ plugins: InstalledPlugin[] }>();
    fetchInstalledPlugins
      .mockReturnValueOnce(older.promise)
      .mockResolvedValueOnce({ plugins: [plugin('kept')] });

    const first = loadInstalledPlugins();
    await loadInstalledPlugins();
    older.reject(new Error('boom'));
    await first;

    expect(installedPlugins.value).toEqual({ status: 'loaded', data: [plugin('kept')] });
  });
});
