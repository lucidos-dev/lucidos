/**
 * `lucidos.storage`, the public face of app storage.
 *
 * Two shapes. An isolated frame reads from the primed mirror. A standalone app
 * tab writes browser storage itself, under the keys the host writes for a frame.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import * as bridge from './_bridge';
import { _resetBridgedStorageForTesting, wsLocalGet } from './_storage';
import { emptyAppStorageSnapshot, APP_STORAGE_QUOTA, APP_STORAGE_VALUE_MAX } from './appStorage';
import { storage, _resetStorageForTesting } from './storage';
import { configure } from './_fetch';

const saved: Record<string, unknown> = {};

function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => { m.set(k, v); },
    removeItem: (k: string) => { m.delete(k); },
    clear: () => m.clear(),
    key: (i: number) => [...m.keys()][i] ?? null,
    get length() { return m.size; },
  } as Storage;
}

beforeEach(() => {
  configure({ baseUrl: '/myws' });
  _resetBridgedStorageForTesting();
  _resetStorageForTesting();
  for (const name of ['window', 'localStorage', 'sessionStorage']) saved[name] = (globalThis as any)[name];
});

afterEach(() => {
  bridge._setBridgedForTesting(null);
  _resetBridgedStorageForTesting();
  for (const name of ['window', 'localStorage', 'sessionStorage']) (globalThis as any)[name] = saved[name];
  vi.restoreAllMocks();
});

describe('in an isolated frame', () => {
  beforeEach(() => {
    (globalThis as any).window = { parent: { postMessage: vi.fn() }, addEventListener() {} };
    bridge._setBridgedForTesting(true);
  });

  function hostWith(fill: (s: ReturnType<typeof emptyAppStorageSnapshot>) => void, writes: 'ok' | 'refuse' = 'ok') {
    const snapshot = emptyAppStorageSnapshot();
    fill(snapshot);
    return vi.spyOn(bridge, 'callHost').mockImplementation((op) => {
      if (op === 'storage.prime') return Promise.resolve(snapshot);
      return writes === 'ok' ? Promise.resolve(null) : Promise.reject(new Error('Browser storage is full'));
    });
  }

  it('reads a stored value synchronously once ready resolves', async () => {
    hostWith((s) => { s.app.local.room = 'kitchen'; s.app.session.tab = 'search'; });
    await storage.ready;
    expect(storage.local.getItem('room')).toBe('kitchen');
    expect(storage.session.getItem('tab')).toBe('search');
    expect(storage.local.getItem('tab')).toBeNull();
    expect(storage.local.length).toBe(1);
    expect(storage.local.key(0)).toBe('room');
    expect(storage.local.key(1)).toBeNull();
  });

  it('coerces keys and values to strings, as localStorage does', async () => {
    const call = hostWith(() => {});
    await storage.ready;
    storage.local.setItem(42 as unknown as string, { a: 1 } as unknown as string);
    expect(storage.local.getItem('42')).toBe('[object Object]');
    expect(call).toHaveBeenCalledWith('storage.set', {
      space: 'app', area: 'local', key: '42', value: '[object Object]',
    });
  });

  it('clear() takes the app keys of that area and leaves the SDK its own', async () => {
    const call = hostWith((s) => {
      s.app.local.a = '1';
      s.app.session.b = '2';
      s.sdk.local.c = '3';
    });
    await storage.ready;
    storage.local.clear();
    expect(storage.local.length).toBe(0);
    expect(storage.session.getItem('b')).toBe('2');
    expect(wsLocalGet('c')).toBe('3');
    expect(call).toHaveBeenCalledWith('storage.clear', { space: 'app', area: 'local' });
  });

  it('warns once when read before ready, naming the key', () => {
    hostWith(() => {});
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    void storage.ready;
    expect(storage.local.getItem('room')).toBeNull();
    storage.local.getItem('other');
    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0][0])).toContain('"room"');
  });

  it('throws QuotaExceededError for a value over the cap, and stores nothing', async () => {
    const call = hostWith(() => {});
    await storage.ready;
    expect(() => storage.local.setItem('big', 'x'.repeat(APP_STORAGE_VALUE_MAX + 1)))
      .toThrow(expect.objectContaining({ name: 'QuotaExceededError' }));
    expect(storage.local.getItem('big')).toBeNull();
    expect(call).toHaveBeenCalledTimes(1); // the prime only
  });

  it('throws QuotaExceededError past the per-app quota', async () => {
    hostWith(() => {});
    await storage.ready;
    const chunk = 'x'.repeat(APP_STORAGE_VALUE_MAX - 8);
    storage.local.setItem('a', chunk);
    storage.local.setItem('b', chunk);
    expect(2 * chunk.length + 2).toBeLessThanOrEqual(APP_STORAGE_QUOTA);
    expect(() => storage.local.setItem('c', chunk)).toThrow(expect.objectContaining({ name: 'QuotaExceededError' }));
    // Replacing a key counts it once, at its new size.
    expect(() => storage.local.setItem('a', chunk)).not.toThrow();
    // The session area has its own quota.
    expect(() => storage.session.setItem('c', chunk)).not.toThrow();
  });

  it('reports a write the host refused, and rolls it back', async () => {
    hostWith((s) => { s.app.local.room = 'kitchen'; }, 'refuse');
    await storage.ready;
    const failures: unknown[] = [];
    const stop = storage.onError((f) => failures.push(f));

    storage.local.setItem('room', 'studio');
    expect(storage.local.getItem('room')).toBe('studio');
    await vi.waitFor(() => expect(failures).toHaveLength(1));
    expect(failures[0]).toEqual({ op: 'set', area: 'local', key: 'room', message: 'Browser storage is full' });
    expect(storage.local.getItem('room')).toBe('kitchen');
    stop();
  });

  it('logs a refused write when the app listens for none', async () => {
    hostWith(() => {}, 'refuse');
    await storage.ready;
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    storage.local.removeItem('room');
    await vi.waitFor(() => expect(error).toHaveBeenCalledTimes(1));
  });

  it('ready rejects when the host cannot hand the store over', async () => {
    vi.spyOn(bridge, 'callHost').mockRejectedValue(new Error('no host'));
    await expect(storage.ready).rejects.toThrow('no host');
  });
});

describe('in a standalone app tab', () => {
  let local: Storage;

  beforeEach(() => {
    bridge._setBridgedForTesting(false);
    local = memoryStorage();
    (globalThis as any).localStorage = local;
    (globalThis as any).sessionStorage = memoryStorage();
    (globalThis as any).window = { location: { pathname: '/myws/app/habit-tracker/' } };
  });

  it('is ready at once', async () => {
    await expect(storage.ready).resolves.toBeUndefined();
  });

  it('writes the same key the host writes for a frame', () => {
    // The literals here pin the stored key layout, shared with the host.
    // Changing it orphans every value already saved.
    storage.local.setItem('room', 'kitchen');
    expect(local.getItem('ws:myws:appbridge:habit-tracker:app:room')).toBe('kitchen');
    expect(storage.local.getItem('room')).toBe('kitchen');
    expect(storage.local.length).toBe(1);
  });

  it('sees only this app, and clears only this app', () => {
    local.setItem('ws:myws:appbridge:demo-director:app:room', 'studio');
    local.setItem('ws:myws:lucidos-theme', 'dark');
    storage.local.setItem('room', 'kitchen');
    expect(storage.local.length).toBe(1);
    storage.local.clear();
    expect(local.getItem('ws:myws:appbridge:demo-director:app:room')).toBe('studio');
    expect(local.getItem('ws:myws:lucidos-theme')).toBe('dark');
  });

  it('refuses to write outside an app', () => {
    (globalThis as any).window = { location: { pathname: '/myws/static/x.html' } };
    expect(storage.local.getItem('room')).toBeNull();
    expect(() => storage.local.setItem('room', 'x')).toThrow('needs an app');
  });

  it('throws QuotaExceededError over the cap here too', () => {
    expect(() => storage.local.setItem('big', 'x'.repeat(APP_STORAGE_VALUE_MAX + 1)))
      .toThrow(expect.objectContaining({ name: 'QuotaExceededError' }));
  });
});
