/**
 * The bridged storage mirror.
 *
 * An isolated frame's own storage throws, so the host holds it. The bridge is
 * async and every reader here is synchronous, which is why the whole store is
 * primed once and served from memory.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import * as bridge from './_bridge';
import {
  bridgedAppBackend,
  primeBridgedStorage,
  wsLocalGet,
  wsSessionGet,
  wsSessionSet,
  _resetBridgedStorageForTesting,
} from './_storage';
import { emptyAppStorageSnapshot } from './appStorage';
import { configure } from './_fetch';

const saved: Record<string, unknown> = {};

beforeEach(() => {
  configure({ baseUrl: '/myws' });
  _resetBridgedStorageForTesting();
  saved.window = (globalThis as any).window;
  (globalThis as any).window = { parent: { postMessage: vi.fn() }, addEventListener() {} };
  bridge._setBridgedForTesting(true);
});

afterEach(() => {
  bridge._setBridgedForTesting(null);
  _resetBridgedStorageForTesting();
  (globalThis as any).window = saved.window;
  vi.restoreAllMocks();
});

function snapshot(fill: (s: ReturnType<typeof emptyAppStorageSnapshot>) => void) {
  const s = emptyAppStorageSnapshot();
  fill(s);
  return s;
}

describe('the mirror keeps the stores apart', () => {
  it('a session value does not answer a local read of the same name', async () => {
    // `localStorage` and `sessionStorage` are separate stores, and one flat map
    // would quietly merge them.
    vi.spyOn(bridge, 'callHost').mockResolvedValue(snapshot((s) => {
      s.sdk.local.shared = 'from-local';
      s.sdk.session.shared = 'from-session';
    }));
    await primeBridgedStorage();

    expect(wsLocalGet('shared')).toBe('from-local');
    expect(wsSessionGet('shared')).toBe('from-session');
  });

  it('an app key never answers an SDK read, nor the reverse', async () => {
    vi.spyOn(bridge, 'callHost').mockResolvedValue(snapshot((s) => {
      s.app.local.theme = 'app-value';
    }));
    await primeBridgedStorage();

    expect(wsLocalGet('theme')).toBeNull();
    expect(bridgedAppBackend('local').get('theme')).toBe('app-value');
  });

  it('an SDK write is readable at once, without waiting on the host', () => {
    // The host is told after, so a reader never awaits a round trip. The host
    // scopes by workspace and app, so the key travels bare.
    const tell = vi.spyOn(bridge, 'tellHost').mockImplementation(() => {});
    wsSessionSet('scroll', '420');

    expect(wsSessionGet('scroll')).toBe('420');
    expect(wsLocalGet('scroll')).toBeNull();
    expect(tell).toHaveBeenCalledWith('storage.set', {
      space: 'sdk', area: 'session', key: 'scroll', value: '420',
    });
  });

  it('a failed prime reads as nothing stored, which is a first visit', async () => {
    vi.spyOn(bridge, 'callHost').mockRejectedValue(new Error('no host'));
    await expect(primeBridgedStorage()).rejects.toThrow();

    expect(wsSessionGet('scroll')).toBeNull();
  });

  it('asks the host once, however many callers wait', async () => {
    const call = vi.spyOn(bridge, 'callHost').mockResolvedValue(emptyAppStorageSnapshot());
    await Promise.all([primeBridgedStorage(), primeBridgedStorage()]);
    await primeBridgedStorage();
    expect(call).toHaveBeenCalledTimes(1);
  });
});

describe('a write before the prime answers', () => {
  it('is not overwritten by the older snapshot', async () => {
    let answer!: (v: unknown) => void;
    vi.spyOn(bridge, 'callHost').mockImplementation((op) => (op === 'storage.prime'
      ? new Promise((r) => { answer = r; })
      : Promise.resolve(null)));
    vi.spyOn(bridge, 'tellHost').mockImplementation(() => {});

    const primed = primeBridgedStorage();
    void bridgedAppBackend('local').set('room', 'new');
    wsSessionSet('scroll', '9');
    answer(snapshot((s) => {
      s.app.local.room = 'old';
      s.app.local.other = 'kept';
      s.sdk.session.scroll = '1';
    }));
    await primed;

    const app = bridgedAppBackend('local');
    expect(app.get('room')).toBe('new');
    expect(app.get('other')).toBe('kept');
    expect(wsSessionGet('scroll')).toBe('9');
  });

  it('a clear before the prime keeps the snapshot out of that area', async () => {
    let answer!: (v: unknown) => void;
    vi.spyOn(bridge, 'callHost').mockImplementation((op) => (op === 'storage.prime'
      ? new Promise((r) => { answer = r; })
      : Promise.resolve(null)));

    const primed = primeBridgedStorage();
    void bridgedAppBackend('local').clear();
    answer(snapshot((s) => {
      s.app.local.room = 'old';
      s.app.session.tab = 'kept';
    }));
    await primed;

    expect(bridgedAppBackend('local').entries()).toEqual([]);
    expect(bridgedAppBackend('session').get('tab')).toBe('kept');
  });
});

describe('a write the host refuses', () => {
  it('is rolled back in the mirror', async () => {
    vi.spyOn(bridge, 'callHost').mockImplementation((op) => (op === 'storage.prime'
      ? Promise.resolve(snapshot((s) => { s.app.local.room = 'kitchen'; }))
      : Promise.reject(new Error('Browser storage is full'))));
    await primeBridgedStorage();
    const app = bridgedAppBackend('local');

    await expect(app.set('room', 'studio')).rejects.toThrow('full');
    expect(app.get('room')).toBe('kitchen');

    await expect(app.set('fresh', 'x')).rejects.toThrow();
    expect(app.get('fresh')).toBeNull();

    await expect(app.clear()).rejects.toThrow();
    expect(app.get('room')).toBe('kitchen');
  });

  it('leaves a newer write to the same key alone', async () => {
    const answers: Array<(ok: boolean) => void> = [];
    vi.spyOn(bridge, 'callHost').mockImplementation(() => new Promise((resolve, reject) => {
      answers.push((ok) => (ok ? resolve(null) : reject(new Error('refused'))));
    }));
    const app = bridgedAppBackend('local');

    const first = app.set('room', 'a') as Promise<void>;
    const second = app.set('room', 'b') as Promise<void>;
    answers[1](true);
    answers[0](false);
    await second;
    await expect(first).rejects.toThrow();
    expect(app.get('room')).toBe('b');
  });
});
