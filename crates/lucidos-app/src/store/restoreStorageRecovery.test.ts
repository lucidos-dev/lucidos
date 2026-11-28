import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const idb = vi.hoisted(() => ({
  data: new Map<string, unknown>(),
  opened: 0,
  wedged: (_connection: number): boolean => false,
}));
vi.mock('../utils/basePath', () => ({ WORKSPACE_ID: 'test-workspace', BASE_PATH: '', API: '/api/v1' }));
vi.mock('../utils/lazyComponent', () => ({ lazyModule: <M>(load: () => Promise<M>) => {
  let loaded: M | undefined;
  return { preload: async () => { loaded = await load(); return true; }, isLoaded: () => loaded !== undefined, get: () => loaded! };
} }));
vi.mock('../api/client/threads', () => ({ createRecoveredDraft: vi.fn(async () => {}) }));
vi.mock('./store', async () => ({ engineRestarting: (await import('@preact/signals')).signal(false) }));
vi.mock('./composeDrafts', () => ({ composeDrafts: { peek: () => new Map([['draft', { text: 'unsent text', image_hashes: [], mode: null }]]) } }));
vi.mock('./composeSelections', () => ({ composeSelections: { peek: () => new Map() }, pendingComposeSelection: { peek: () => ({}) } }));
vi.mock('./pageOwner', () => ({ pageOwnerId: 'this-page', holdPageOwnerLock: vi.fn(), liveOwnerIds: async () => new Set<string>() }));
vi.mock('./pendingUploadRecords', () => ({ livePendingUploadRecords: () => [],
  readPendingUploadHistory: async () => ({ uploads: [], queuedUploadSendRecords: [] }),
  readArchivedUploadBytes: async () => undefined,
  readPendingUploadStore: async () => ({ uploads: [], queuedUploadSendRecords: [] }) }));
vi.mock('./unsentMessages', () => ({ unsentMessages: { peek: () => new Map() } }));
vi.mock('./uploadSendIntents', () => ({ queuedUploadSends: { peek: () => new Map() } }));
vi.mock('./pendingUploads', () => ({ pendingUploads: { peek: () => new Map() } }));
vi.mock('./unsentMessageRecords', () => ({ liveUnsentMessageRecords: () => [], readUnsentMessageStore: async () => [] }));

function fakeIndexedDB() {
  return {
    open: () => {
      const connection = ++idb.opened;
      const req: Record<string, unknown> & { onsuccess?: () => void } = { transaction: null };
      req.result = {
        onclose: null, onversionchange: null,
        close: vi.fn(),
        transaction: (stores: string | string[]) => {
          const wedged = idb.wedged(connection);
          const ops: Array<() => void> = [];
          const tx: Record<string, unknown> & { oncomplete?: () => void } = {
            error: null,
            abort: () => {},
            objectStore: (store: string) => {
              if (typeof stores === 'string' ? store !== stores : !stores.includes(store)) {
                throw new Error(`Store ${store} is outside the transaction`);
              }
              return {
                transaction: tx,
                get: (key: string) => {
                  const request: { result?: unknown; onsuccess?: () => void } = {};
                  ops.push(() => {
                    request.result = structuredClone(idb.data.get(key));
                    request.onsuccess?.();
                  });
                  return request;
                },
                put: (value: unknown, key: string) => { ops.push(() => idb.data.set(key, structuredClone(value))); },
              };
            },
          };
          if (!wedged) {
            setTimeout(() => {
              for (let i = 0; i < ops.length; i++) ops[i]();
              tx.oncomplete?.();
            });
          }
          return tx;
        },
      };
      setTimeout(() => req.onsuccess?.());
      return req;
    },
  };
}

const generationA = '11111111-1111-4111-8111-111111111111';
const generationB = '22222222-2222-4222-8222-222222222222';
const fetch = vi.fn(async (_url: unknown, _init?: RequestInit) => new Response(null, { status: 204 }));
const reload = vi.fn();

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  idb.data = new Map();
  idb.opened = 0;
  idb.wedged = () => false;
  fetch.mockClear();
  reload.mockClear();
  vi.stubGlobal('indexedDB', fakeIndexedDB());
  vi.stubGlobal('fetch', fetch);
  vi.stubGlobal('document', { body: { inert: false }, querySelector: () => null });
  vi.stubGlobal('window', { location: { reload } });
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function setup() {
  const runtime = await import('./restoreRuntime');
  const gate = await import('../utils/restoreAdmission');
  const { DEVICE_DATABASE_STALL_MS, DeviceDatabaseStalledError } = await import('./deviceDatabase');
  const { notifications } = await import('../../../../packages/lucidos-sdk/src/notifications');
  runtime.installRestoreRuntime();
  return { ...runtime, ...gate, notifications, DEVICE_DATABASE_STALL_MS, DeviceDatabaseStalledError };
}

const observed = () => (idb.data.get('workspace') as { generation: string | null } | undefined)?.generation;

it('recovers a page-load generation read that stalled, on a fresh connection', async () => {
  idb.data.set('workspace', { generation: generationA, recoveries: [] });
  idb.wedged = (connection) => connection === 1;
  const app = await setup();
  const held = app.notifications.markAllRead();
  const refused = expect(held).rejects.toThrow('Workspace restored');
  await vi.advanceTimersByTimeAsync(app.DEVICE_DATABASE_STALL_MS);

  const hello = app.acceptRestoreHello(app.beginRestoreConnection(), generationA);
  await vi.advanceTimersByTimeAsync(10);
  expect(await hello).toBe(true);
  expect(idb.opened).toBe(2);
  expect(app.restoreAdmissionReady()).toBe(true);
  await refused;
  expect(fetch).not.toHaveBeenCalled();

  await app.notifications.markAllRead();
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(new Headers(fetch.mock.calls[0][1]?.headers).get('x-lucidos-restore-generation')).toBe(generationA);
});

it('fails within its bounds while storage stays stalled, holds requests, and admits once it answers', async () => {
  idb.wedged = () => true;
  const app = await setup();
  const held = app.notifications.markAllRead();
  await vi.advanceTimersByTimeAsync(app.DEVICE_DATABASE_STALL_MS);

  const first = app.acceptRestoreHello(app.beginRestoreConnection(), null);
  const failed = expect(first).rejects.toBeInstanceOf(app.DeviceDatabaseStalledError);
  await vi.advanceTimersByTimeAsync(2 * app.DEVICE_DATABASE_STALL_MS);
  await failed;
  expect(app.restoreAdmissionReady()).toBe(false);
  expect(fetch).not.toHaveBeenCalled();
  expect(observed()).toBeUndefined();

  idb.wedged = () => false;
  const retry = app.acceptRestoreHello(app.beginRestoreConnection(), null);
  await vi.advanceTimersByTimeAsync(10);
  expect(await retry).toBe(true);
  expect(observed()).toBe(null);
  await held;
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('keeps the page baseline when an observe write stalls and another tab advances the marker', async () => {
  idb.data.set('workspace', { generation: generationA, recoveries: [] });
  const app = await setup();
  await vi.advanceTimersByTimeAsync(10);
  const held = app.notifications.markAllRead();

  idb.wedged = (connection) => connection === 1;
  const stalledObserve = app.acceptRestoreHello(app.beginRestoreConnection(), generationA);
  const observeFailed = expect(stalledObserve).rejects.toBeInstanceOf(app.DeviceDatabaseStalledError);
  await vi.advanceTimersByTimeAsync(app.DEVICE_DATABASE_STALL_MS);
  await observeFailed;
  expect(app.restoreAdmissionReady()).toBe(false);
  idb.data.set('workspace', { generation: generationB, recoveries: [] });
  idb.wedged = () => false;

  const stale = app.acceptRestoreHello(app.beginRestoreConnection(), generationA);
  const conflicted = expect(stale).rejects.toThrow('changed in another tab');
  await vi.advanceTimersByTimeAsync(10);
  await conflicted;
  expect(app.restoreAdmissionReady()).toBe(false);
  expect(fetch).not.toHaveBeenCalled();

  const restored = app.acceptRestoreHello(app.beginRestoreConnection(), generationB);
  await vi.advanceTimersByTimeAsync(10);
  expect(await restored).toBe(false);
  expect(reload).toHaveBeenCalledTimes(1);
  const record = idb.data.get('workspace') as { recoveries: Array<{ intent: { drafts: Array<{ draft: { text: string } }> } }> };
  expect(record.recoveries[0].intent.drafts[0].draft.text).toBe('unsent text');
  await vi.advanceTimersByTimeAsync(0);
  expect(fetch).not.toHaveBeenCalled();
  void held;
});

it('ignores an old connection that is admitted after a newer one began', async () => {
  idb.wedged = (connection) => connection === 1;
  const app = await setup();
  await vi.advanceTimersByTimeAsync(app.DEVICE_DATABASE_STALL_MS);
  const old = app.beginRestoreConnection();
  const current = app.beginRestoreConnection();
  const oldHello = app.acceptRestoreHello(old, null);
  await vi.advanceTimersByTimeAsync(10);
  expect(await oldHello).toBe(false);
  expect(app.restoreAdmissionReady()).toBe(false);
  const currentHello = app.acceptRestoreHello(current, null);
  await vi.advanceTimersByTimeAsync(10);
  expect(await currentHello).toBe(true);
});

it('reads the target thread while admission waits on hung storage, and only before the first admission', async () => {
  idb.wedged = () => true;
  const app = await setup();
  const { json } = await import('../api/client/_core');
  const fetchThreadById = (id: string) => json(`/api/v1/threads/${id}`);
  const fetchThreadEvents = (id: string) => json(`/api/v1/threads/${id}/events`);
  const thread = '33333333-3333-4333-8333-333333333333';
  fetch.mockImplementation(async (url) => String(url).endsWith('/events')
    ? Response.json({ events: [], currentAggregate: null })
    : Response.json({ thread_id: thread }));
  const held = app.notifications.markAllRead();
  const stalled = app.acceptRestoreHello(app.beginRestoreConnection(), null);
  const failed = expect(stalled).rejects.toBeInstanceOf(app.DeviceDatabaseStalledError);
  await vi.advanceTimersByTimeAsync(10);
  expect(app.restoreAdmissionPhase()).toBe('generation');

  await expect(fetchThreadById(thread)).resolves.toMatchObject({ thread_id: thread });
  await expect(fetchThreadEvents(thread)).resolves.toMatchObject({ events: [] });
  expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([`/api/v1/threads/${thread}`, `/api/v1/threads/${thread}/events`]);
  expect(app.restoreAdmissionReady()).toBe(false);
  await vi.advanceTimersByTimeAsync(4 * app.DEVICE_DATABASE_STALL_MS);
  await failed;

  idb.wedged = () => false;
  const admitted = app.acceptRestoreHello(app.beginRestoreConnection(), null);
  await vi.advanceTimersByTimeAsync(10);
  expect(await admitted).toBe(true);
  await held;
  fetch.mockClear();

  app.beginRestoreConnection();
  const later = fetchThreadById(thread);
  await vi.advanceTimersByTimeAsync(10);
  expect(fetch).not.toHaveBeenCalled();
  const readmitted = app.acceptRestoreHello(app.beginRestoreConnection(), null);
  await vi.advanceTimersByTimeAsync(10);
  expect(await readmitted).toBe(true);
  await expect(later).resolves.toMatchObject({ thread_id: thread });
});
