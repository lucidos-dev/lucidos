// @vitest-environment jsdom
/**
 * App storage, on the host side of the bridge.
 *
 * The host is the boundary: it decides which app a frame is from its own
 * element, and an app frame cannot name another app. The workspace namespacing
 * is installed for real here, because the gateway topology is where the prime
 * used to come back empty.
 */
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { APP_STORAGE_QUOTA, APP_STORAGE_VALUE_MAX, SDK_STORAGE_QUOTA } from '@lucidos/sdk';
import { installWorkspaceStorage } from '../../utils/workspaceStorage';

vi.mock('../../utils/basePath', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/basePath')>()),
  WORKSPACE_ID: 'myws',
}));
const showToast = vi.fn();
vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showToast: (...args: unknown[]) => showToast(...args),
}));

const { clearAppStorage, installAppBridge, _resetAppBridgeForTesting } = await import('./app-bridge');

type Reply = { ok: boolean; value?: unknown; error?: string };

/** A store whose methods live on its prototype, as the browser's do. The suite's
 *  shared stub keeps them on the instance, where the workspace override cannot
 *  reach, so the namespacing this file is about would never run. */
class PrototypeStorage {
  readonly entries = new Map<string, string>();
  getItem(key: string): string | null { return this.entries.get(key) ?? null; }
  setItem(key: string, value: string): void { this.entries.set(key, value); }
  removeItem(key: string): void { this.entries.delete(key); }
  clear(): void { this.entries.clear(); }
  key(i: number): string | null { return [...this.entries.keys()][i] ?? null; }
  get length(): number { return this.entries.size; }
}

const local = new PrototypeStorage();
const session = new PrototypeStorage();

beforeAll(() => {
  for (const [name, store] of [['localStorage', local], ['sessionStorage', session]] as const) {
    Object.defineProperty(globalThis, name, { value: store, writable: true, configurable: true });
  }
  installWorkspaceStorage(local as unknown as Storage, 'myws');
});

let stop: () => void;
let habit: HTMLIFrameElement;
let demo: HTMLIFrameElement;
let counter = 0;

function mountFrame(src: string): HTMLIFrameElement {
  const frame = document.createElement('iframe');
  frame.setAttribute('data-role', 'app-ui-frame');
  frame.setAttribute('src', src);
  document.body.appendChild(frame);
  return frame;
}

/** Send one bridge op as `frame` and wait for the host's reply. jsdom cannot
 *  set `event.source` through `postMessage`, so the event is built by hand. */
function ask(frame: HTMLIFrameElement, op: string, args: unknown): Promise<Reply> {
  const win = frame.contentWindow as Window;
  const id = `t-${++counter}`;
  return new Promise((resolve) => {
    vi.spyOn(win, 'postMessage').mockImplementation((m: unknown) => {
      const reply = m as Reply & { id?: string };
      if (reply.id === id) resolve(reply);
    });
    const event = new MessageEvent('message', { data: { type: 'lucidos:bridge', id, op, args } });
    Object.defineProperty(event, 'source', { value: win });
    window.dispatchEvent(event);
  });
}

const set = (frame: HTMLIFrameElement, key: string, value: string, extra: object = {}) =>
  ask(frame, 'storage.set', { space: 'app', area: 'local', key, value, ...extra });

beforeEach(() => {
  _resetAppBridgeForTesting();
  localStorage.clear();
  sessionStorage.clear();
  showToast.mockClear();
  habit = mountFrame('/app/habit-tracker/');
  demo = mountFrame('/app/demo-director/');
  stop = installAppBridge();
});

afterEach(() => {
  stop();
  habit.remove();
  demo.remove();
  vi.restoreAllMocks();
});

describe('app storage is scoped per app by the host', () => {
  it('primes what an app wrote, behind the workspace namespace', async () => {
    expect((await set(habit, 'room', 'kitchen')).ok).toBe(true);
    // The override namespaces the write, and the prime has to find it there.
    // The literal pins the stored key layout: changing it orphans saved values.
    expect([...local.entries.keys()]).toEqual(['ws:myws:appbridge:habit-tracker:app:room']);

    const prime = await ask(habit, 'storage.prime', {});
    expect(prime.ok).toBe(true);
    expect((prime.value as any).app.local).toEqual({ room: 'kitchen' });
  });

  it('never hands one app the keys of another', async () => {
    await set(habit, 'room', 'kitchen');
    await set(demo, 'room', 'studio');

    const habitPrime = (await ask(habit, 'storage.prime', {})).value as any;
    const demoPrime = (await ask(demo, 'storage.prime', {})).value as any;
    expect(habitPrime.app.local).toEqual({ room: 'kitchen' });
    expect(demoPrime.app.local).toEqual({ room: 'studio' });
  });

  it('ignores an app id the frame names for itself', async () => {
    await set(demo, 'room', 'forged', { appId: 'habit-tracker', app: 'habit-tracker' });
    const habitPrime = (await ask(habit, 'storage.prime', {})).value as any;
    expect(habitPrime.app.local).toEqual({});
  });

  it('cannot reach another app through the key itself', async () => {
    // A key is only ever a suffix under the caller's own prefix.
    await set(demo, '../habit-tracker:app:room', 'forged');
    await set(demo, 'appbridge:habit-tracker:app:room', 'forged');
    const habitPrime = (await ask(habit, 'storage.prime', {})).value as any;
    expect(habitPrime.app.local).toEqual({});
  });

  it('refuses a frame whose src names no app', async () => {
    const stray = mountFrame('/static/something.html');
    const reply = await set(stray, 'room', 'kitchen');
    expect(reply.ok).toBe(false);
    expect((await ask(stray, 'storage.prime', {})).ok).toBe(false);
    stray.remove();
  });

  it('keeps local and session apart, and the sdk space apart from the app', async () => {
    await set(habit, 'k', 'local-app');
    await ask(habit, 'storage.set', { space: 'app', area: 'session', key: 'k', value: 'session-app' });
    await ask(habit, 'storage.set', { space: 'sdk', area: 'session', key: 'k', value: 'session-sdk' });

    const prime = (await ask(habit, 'storage.prime', {})).value as any;
    expect(prime.app.local).toEqual({ k: 'local-app' });
    expect(prime.app.session).toEqual({ k: 'session-app' });
    expect(prime.sdk.session).toEqual({ k: 'session-sdk' });
  });

  it('clears one area of one app, and nothing else', async () => {
    await set(habit, 'a', '1');
    await ask(habit, 'storage.set', { space: 'app', area: 'session', key: 'b', value: '2' });
    await ask(habit, 'storage.set', { space: 'sdk', area: 'local', key: 'c', value: '3' });
    await set(demo, 'a', '4');

    expect((await ask(habit, 'storage.clear', { space: 'app', area: 'local' })).ok).toBe(true);

    const prime = (await ask(habit, 'storage.prime', {})).value as any;
    expect(prime.app.local).toEqual({});
    expect(prime.app.session).toEqual({ b: '2' });
    expect(prime.sdk.local).toEqual({ c: '3' });
    expect(((await ask(demo, 'storage.prime', {})).value as any).app.local).toEqual({ a: '4' });
  });

  it('primes the area that works when the other is disabled', async () => {
    await set(habit, 'room', 'kitchen');
    const real = Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage')!;
    Object.defineProperty(globalThis, 'sessionStorage', {
      configurable: true,
      get() { throw new DOMException('disabled', 'SecurityError'); },
    });
    try {
      const prime = await ask(habit, 'storage.prime', {});
      expect(prime.ok).toBe(true);
      expect((prime.value as any).app.local).toEqual({ room: 'kitchen' });
      expect((prime.value as any).app.session).toEqual({});
    } finally {
      Object.defineProperty(globalThis, 'sessionStorage', real);
    }
  });

  it('forgets a deleted app on this device, and only that app', async () => {
    await set(habit, 'room', 'kitchen');
    await ask(habit, 'storage.set', { space: 'sdk', area: 'session', key: 'scroll', value: '1' });
    await set(demo, 'room', 'studio');

    clearAppStorage('habit-tracker');

    const habitPrime = (await ask(habit, 'storage.prime', {})).value as any;
    expect(habitPrime.app.local).toEqual({});
    expect(habitPrime.sdk.session).toEqual({});
    expect(((await ask(demo, 'storage.prime', {})).value as any).app.local).toEqual({ room: 'studio' });
  });

  it('removes one key', async () => {
    await set(habit, 'a', '1');
    await ask(habit, 'storage.remove', { space: 'app', area: 'local', key: 'a' });
    expect(((await ask(habit, 'storage.prime', {})).value as any).app.local).toEqual({});
  });
});

describe('app storage limits and failures', () => {
  it('refuses a value over the cap, stores nothing, and says so', async () => {
    const reply = await set(habit, 'big', 'x'.repeat(APP_STORAGE_VALUE_MAX + 1));
    expect(reply.ok).toBe(false);
    expect(reply.error).toContain('limit');
    expect(((await ask(habit, 'storage.prime', {})).value as any).app.local).toEqual({});
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('limit'),
      'error',
      expect.objectContaining({ title: 'App "habit-tracker" could not save' }),
    );
  });

  it('refuses a write that would take an app over its quota', async () => {
    const half = 'x'.repeat(APP_STORAGE_VALUE_MAX - 10);
    expect((await set(habit, 'a', half)).ok).toBe(true);
    expect((await set(habit, 'b', half)).ok).toBe(true);
    // Two halves fit; a third key of any size takes it past the quota.
    expect(APP_STORAGE_QUOTA).toBeLessThan(3 * half.length);
    expect((await set(habit, 'c', half)).ok).toBe(false);
    // Another app has its own quota.
    expect((await set(demo, 'c', half)).ok).toBe(true);
  });

  it('reports a full browser store to the app and to the user', async () => {
    const full = vi.spyOn(PrototypeStorage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });
    const reply = await set(habit, 'a', '1');
    full.mockRestore();
    expect(reply.ok).toBe(false);
    expect(reply.error).toContain('full');
    expect(showToast).toHaveBeenCalledTimes(1);
  });

  it('logs an sdk-space failure rather than toasting it', async () => {
    const full = vi.spyOn(PrototypeStorage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await ask(habit, 'storage.set', { space: 'sdk', area: 'session', key: 'scroll', value: '1' });
    full.mockRestore();
    expect(showToast).not.toHaveBeenCalled();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('"habit-tracker"'), 'Browser storage is full');
  });

  it('caps the sdk space too, since the frame names the space', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    const big = 'x'.repeat(SDK_STORAGE_QUOTA);
    const reply = await ask(habit, 'storage.set', { space: 'sdk', area: 'local', key: 'k', value: big });
    expect(reply.ok).toBe(false);
    expect(((await ask(habit, 'storage.prime', {})).value as any).sdk.local).toEqual({});
  });

  it('refuses a malformed request rather than storing a guess', async () => {
    expect((await ask(habit, 'storage.set', { space: 'app', area: 'local', key: 1, value: 'v' })).ok).toBe(false);
    expect((await ask(habit, 'storage.set', { space: 'host', area: 'local', key: 'k', value: 'v' })).ok).toBe(false);
    expect((await ask(habit, 'storage.set', { space: 'app', area: 'cookies', key: 'k', value: 'v' })).ok).toBe(false);
  });
});
