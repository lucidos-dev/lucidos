import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it('counts a page waiting for its lifetime lock as live', async () => {
  vi.resetModules();
  const { liveOwnerIds, PAGE_OWNER_LOCK_PREFIX } = await import('./pageOwner');
  vi.stubGlobal('navigator', { locks: { request: (_name: string, callback: () => void) => {
    callback();
    return new Promise(() => {});
  }, query: async () => ({
    held: [{ name: `${PAGE_OWNER_LOCK_PREFIX}held` }],
    pending: [{ name: `${PAGE_OWNER_LOCK_PREFIX}starting` }, { name: 'unrelated-lock' }],
  }) } });
  expect(await liveOwnerIds()).toEqual(new Set(['held', 'starting']));
});

it('keeps unavailable or refused lock inspection unknown', async () => {
  vi.resetModules();
  const { liveOwnerIds } = await import('./pageOwner');
  vi.stubGlobal('navigator', {});
  expect(await liveOwnerIds()).toBeNull();
  vi.stubGlobal('navigator', { locks: { query: async () => { throw new Error('unavailable'); } } });
  expect(await liveOwnerIds()).toBeNull();
});

it('keeps owners unknown when requesting a lifetime lock fails but inspection succeeds', async () => {
  vi.resetModules();
  const { liveOwnerIds, holdPageOwnerLock } = await import('./pageOwner');
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.stubGlobal('navigator', { locks: {
    request: () => Promise.reject(new Error('refused')),
    query: async () => ({ held: [], pending: [] }),
  } });
  holdPageOwnerLock();
  await Promise.resolve();
  expect(await liveOwnerIds()).toBeNull();
});

it('keeps synchronous lock request failures unknown without aborting startup', async () => {
  vi.resetModules();
  const { liveOwnerIds, holdPageOwnerLock } = await import('./pageOwner');
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  const request = vi.fn(() => { throw new Error('denied'); });
  vi.stubGlobal('navigator', { locks: { request, query: async () => ({ held: [], pending: [] }) } });
  expect(holdPageOwnerLock).not.toThrow();
  expect(await liveOwnerIds()).toBeNull();
  expect(request).toHaveBeenCalledOnce();
});

it('keeps owners unknown if the lifetime lock unexpectedly releases', async () => {
  vi.resetModules();
  const { liveOwnerIds } = await import('./pageOwner');
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.stubGlobal('navigator', { locks: {
    request: (_name: string, callback: () => void) => { callback(); return Promise.resolve(); },
    query: async () => ({ held: [], pending: [] }),
  } });
  expect(await liveOwnerIds()).toBeNull();
});

it('keeps owners unknown until this page actually acquires its lifetime lock', async () => {
  vi.resetModules();
  const { liveOwnerIds, holdPageOwnerLock } = await import('./pageOwner');
  let acquire: (() => void) | undefined;
  const request = vi.fn((_name: string, callback: () => void) => {
    acquire = callback;
    return new Promise(() => {});
  });
  vi.stubGlobal('navigator', { locks: { request, query: async () => ({ held: [], pending: [] }) } });
  holdPageOwnerLock();
  expect(await liveOwnerIds()).toBeNull();
  holdPageOwnerLock();
  expect(request).toHaveBeenCalledOnce();
  acquire!();
  expect(await liveOwnerIds()).toEqual(new Set());
});
