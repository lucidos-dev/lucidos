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
  primeBridgedStorage,
  wsLocalGet,
  wsSessionGet,
  wsSessionSet,
  _resetBridgedStorageForTesting,
} from './_storage';
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

describe('the mirror keeps the two stores apart', () => {
  it('a session value does not answer a local read of the same name', async () => {
    // `localStorage` and `sessionStorage` are separate stores, and one flat map
    // would quietly merge them. Nothing overlaps today, which is exactly why
    // this has to be pinned rather than noticed.
    vi.spyOn(bridge, 'callHost').mockResolvedValue({
      'local:ws:myws:shared': 'from-local',
      'session:ws:myws:shared': 'from-session',
    });
    await primeBridgedStorage();

    expect(wsLocalGet('shared')).toBe('from-local');
    expect(wsSessionGet('shared')).toBe('from-session');
  });

  it('a write is readable at once, without waiting on the host', async () => {
    // The host is told after, so a reader never awaits a round trip.
    const tell = vi.spyOn(bridge, 'tellHost').mockImplementation(() => {});
    wsSessionSet('scroll', '420');

    expect(wsSessionGet('scroll')).toBe('420');
    expect(wsLocalGet('scroll')).toBeNull();
    expect(tell).toHaveBeenCalledWith('storage.set', {
      key: 'ws:myws:scroll', value: '420', session: true,
    });
  });

  it('a failed prime reads as nothing stored, which is a first visit', async () => {
    vi.spyOn(bridge, 'callHost').mockRejectedValue(new Error('no host'));
    await expect(primeBridgedStorage()).rejects.toThrow();

    expect(wsSessionGet('scroll')).toBeNull();
  });
});
