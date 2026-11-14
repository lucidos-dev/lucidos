import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
  cards: new Map<string, import('./unsentMessages').UnsentMessage>(),
  queued: new Map<string, import('./uploadSendIntents').UploadSendIntent>(),
  uploads: new Map<string, import('./pendingUploads').PendingUpload[]>(),
  retained: [] as import('./pendingUploadRecords').PendingUploadRecord[],
  sends: [] as import('./unsentMessageRecords').UnsentMessageRecord[],
  generation: null as string | null | undefined,
  recoveries: [] as import('./restoreGenerationRecords').RestoreIntentRecord[],
  journal: vi.fn(async () => {}),
  observe: vi.fn(async () => {}), preserve: vi.fn<import('./restoreGenerationRecords').RestoreIntentStore['preserve']>(async () => {}),
  create: vi.fn(async () => {}), complete: vi.fn(async () => {}), reload: vi.fn(),
}));
vi.mock('../utils/basePath', () => ({ WORKSPACE_ID: 'test-workspace' }));
vi.mock('./restoreGenerationRecords', () => ({ createRestoreGenerationStore: () => ({
  journalHistorical: state.journal,
  readGeneration: async () => state.generation,
  observe: state.observe, preserve: state.preserve,
  readRecoveries: async () => state.recoveries,
  prepareRecovery: async () => ({ generation: state.generation, drafts: [{ sourceThreadId: 'old', threadId: 'new' }], completed: [] }),
  completeRecoveryDraft: state.complete,
}) }));
vi.mock('../api/client/threads', () => ({ createRecoveredDraft: state.create }));
vi.mock('./composeDrafts', () => ({ composeDrafts: { peek: () => new Map([['old', { text: 'local edit', image_hashes: [], mode: null }]]) } }));
vi.mock('./composeSelections', () => ({ composeSelections: { peek: () => new Map() }, pendingComposeSelection: { peek: () => ({}) } }));
vi.mock('./pageOwner', () => ({ pageOwnerId: 'this-page', holdPageOwnerLock: vi.fn(), liveOwnerIds: async () => new Set<string>() }));
vi.mock('./pendingUploadRecords', () => ({ livePendingUploadRecords: () => state.retained,
  readPendingUploadStore: async () => ({ uploads: [], queuedUploadSendRecords: [] }) }));
vi.mock('./unsentMessages', () => ({ unsentMessages: { peek: () => state.cards } }));
vi.mock('./uploadSendIntents', () => ({ queuedUploadSends: { peek: () => state.queued } }));
vi.mock('./pendingUploads', () => ({ pendingUploads: { peek: () => state.uploads } }));
vi.mock('./unsentMessageRecords', () => ({ liveUnsentMessageRecords: () => state.sends, readUnsentMessageStore: async () => [] }));

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks();
  state.generation = null; state.recoveries = []; state.uploads = new Map();
  state.retained = []; state.cards = new Map(); state.queued = new Map();
  state.sends = [];
  state.journal.mockResolvedValue();
  state.observe.mockResolvedValue(); state.create.mockResolvedValue();
  vi.stubGlobal('document', { body: { inert: false } });
  vi.stubGlobal('window', { location: { reload: state.reload } });
});
afterEach(() => vi.unstubAllGlobals());

async function setup() {
  const runtime = await import('./restoreRuntime');
  const gate = await import('../utils/restoreAdmission');
  runtime.installRestoreRuntime();
  return { ...runtime, ...gate };
}

it('keeps startup reads closed until the first unrestored hello is durably observed', async () => {
  const app = await setup();
  const epoch = app.beginRestoreConnection();
  expect(app.restoreAdmissionReady()).toBe(false);
  expect(await app.acceptRestoreHello(epoch, null)).toBe(true);
  expect(state.observe).toHaveBeenCalledWith(null, { generation: null });
  expect(app.restoreAdmissionReady()).toBe(true);
  expect(state.create).not.toHaveBeenCalled();
});

it('captures live composers and reloads once on a changed generation without releasing startup', async () => {
  const app = await setup();
  const epoch = app.beginRestoreConnection();
  expect(await app.acceptRestoreHello(epoch, 'restored')).toBe(false);
  expect(document.body.inert).toBe(true);
  expect(state.preserve).toHaveBeenCalledWith('restored', expect.objectContaining({
    drafts: [expect.objectContaining({ draft: expect.objectContaining({ text: 'local edit' }) })],
  }), { generation: null });
  await app.acceptRestoreHello(epoch, 'restored');
  expect(state.reload).toHaveBeenCalledTimes(1);
  expect(app.restoreAdmissionReady()).toBe(false);
});

// A failed capture must never leave the page frozen: the user still needs to
// read and copy what they typed while the next connection retries admission.
it('unfreezes the page when preserving intent fails on a changed generation', async () => {
  const app = await setup();
  state.preserve.mockRejectedValueOnce(new Error('QuotaExceededError'));
  const epoch = app.beginRestoreConnection();
  await expect(app.acceptRestoreHello(epoch, 'restored')).rejects.toThrow('QuotaExceededError');
  expect(document.body.inert).toBe(false);
  expect(state.reload).not.toHaveBeenCalled();
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), 'restored')).toBe(false);
  expect(document.body.inert).toBe(true);
  expect(state.reload).toHaveBeenCalledTimes(1);
});

it('reconstructs journaled drafts before releasing startup and suppresses their original Retry source', async () => {
  state.generation = 'restored';
  state.recoveries = [{ pageId: 'old-page', generation: 'restored', intent: {
    drafts: [{ threadId: 'old', draft: { text: 'unsent', image_hashes: [], mode: null }, selection: {} }],
    pendingSelection: {}, unsentMessages: [{ eventId: 'old-send' } as import('./unsentMessageRecords').UnsentMessageRecord],
  } }];
  const app = await setup();
  const epoch = app.beginRestoreConnection();
  state.create.mockImplementationOnce(async () => { expect(app.restoreAdmissionReady()).toBe(false); });
  expect(await app.acceptRestoreHello(epoch, 'restored')).toBe(true);
  expect(state.create).toHaveBeenCalledWith('new', 'restored', state.recoveries[0].intent.drafts[0]);
  expect(state.complete).toHaveBeenCalledTimes(1);
  expect(app.restoreSourcePreserved('send', 'old-send')).toBe(true);
  expect(app.restoreAdmissionReady()).toBe(true);
});

it('does not admit failed persistence or a stale hello', async () => {
  const app = await setup();
  const old = app.beginRestoreConnection();
  const current = app.beginRestoreConnection();
  expect(await app.acceptRestoreHello(old, null)).toBe(false);
  state.observe.mockRejectedValueOnce(new Error('IndexedDB unavailable'));
  await expect(app.acceptRestoreHello(current, null)).rejects.toThrow('IndexedDB unavailable');
  expect(app.restoreAdmissionReady()).toBe(false);
});


it('captures production in-memory files omitted from durable upload records', async () => {
  const app = await setup();
  const bytes = new Uint8Array([4, 5]).buffer;
  state.uploads.set('old', [{ localId: 'live-only', threadId: 'old', mime: 'image/png', previewUrl: '',
    file: { name: 'photo.png', size: 2, arrayBuffer: async () => bytes } as File,
    state: { kind: 'offline', reason: 'offline' } }]);
  const epoch = app.beginRestoreConnection();
  expect(await app.acceptRestoreHello(epoch, 'restored')).toBe(false);
  const intent = state.preserve.mock.calls[0][1];
  expect(intent.drafts[0].uploads?.[0].bytes).toEqual(bytes);
  expect(state.reload).toHaveBeenCalledTimes(1);
});

it('captures landed live bytes after their pending File detached and durable storage omitted them', async () => {
  state.retained = [{ localId: 'landed', threadId: 'old', name: 'photo.png', mime: 'image/png',
    bytes: new Uint8Array([4, 5]).buffer, size: 2, createdAt: 1, ownerId: 'this-page', landedHash: 'confirmed' }];
  const app = await setup();
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), 'restored')).toBe(false);
  expect(state.preserve.mock.calls[0][1].drafts[0].uploads).toEqual(state.retained);
});

it('captures an in-flight send omitted from durable storage and from the unsent card map', async () => {
  state.sends = [{ eventId: 'posting', threadId: 'old', body: { message: 'in flight', mode: 'human', image_hashes: ['confirmed'] },
    ownerId: 'this-page', phase: 'sending', failedRetries: 0, sentAt: new Date().toISOString(), settlement: { kind: 'follow-up' } }];
  state.retained = [{ localId: 'landed', threadId: 'old', name: 'photo.png', mime: 'image/png',
    bytes: new Uint8Array([4, 5]).buffer, size: 2, createdAt: 1, ownerId: 'this-page', landedHash: 'confirmed' }];
  const app = await setup();
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), 'restored')).toBe(false);
  const intent = state.preserve.mock.calls[0][1];
  expect(intent.unsentMessages).toEqual(state.sends);
  expect(intent.drafts.find((draft) => draft.draft.text === 'in flight')?.uploads).toEqual(state.retained);
});

it('keeps admission closed when live image bytes cannot be read', async () => {
  const app = await setup();
  state.uploads.set('old', [{ localId: 'unreadable', threadId: 'old', mime: 'image/png', previewUrl: '',
    file: { name: 'photo.png', size: 2, arrayBuffer: async (): Promise<ArrayBuffer> => { throw new Error('file unavailable'); } } as File,
    state: { kind: 'offline', reason: 'offline' } }]);
  const epoch = app.beginRestoreConnection();
  await expect(app.acceptRestoreHello(epoch, 'restored')).rejects.toThrow('file unavailable');
  expect(state.reload).not.toHaveBeenCalled();
  expect(state.preserve).not.toHaveBeenCalled();
  expect(app.restoreAdmissionReady()).toBe(false);
});

it('publishes the source origin only after successful reconstruction admission', async () => {
  state.generation = 'restored';
  const app = await setup();
  const origin = await import('../utils/restoreSourceGeneration');
  expect(origin.sourceGeneration()).toBe(null);
  state.observe.mockRejectedValueOnce(new Error('cannot observe'));
  const epoch = app.beginRestoreConnection();
  await expect(app.acceptRestoreHello(epoch, 'restored')).rejects.toThrow('cannot observe');
  expect(origin.sourceGeneration()).toBe(null);
  expect(await app.acceptRestoreHello(epoch, 'restored')).toBe(true);
  expect(origin.sourceGeneration()).toBe('restored');
});


it('names the step a stalled admission is in, and never admits on the stall', async () => {
  const app = await setup();
  const { DeviceDatabaseStalledError } = await import('./deviceDatabase');
  expect(app.restoreAdmissionPhase()).toBe('awaiting-hello');
  const epoch = app.beginRestoreConnection();
  let release!: () => void;
  state.journal.mockReturnValueOnce(new Promise((_, reject) => {
    release = () => reject(new DeviceDatabaseStalledError('test', 'transaction'));
  }));
  const hello = app.acceptRestoreHello(epoch, null);
  await vi.waitFor(() => expect(app.restoreAdmissionPhase()).toBe('journal'));
  release();
  await expect(hello).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
  expect(app.restoreAdmissionReady()).toBe(false);
  expect(app.restoreAdmissionPhase()).toBe('journal');

  const retry = app.beginRestoreConnection();
  expect(app.restoreAdmissionPhase()).toBe('awaiting-hello');
  expect(await app.acceptRestoreHello(retry, null)).toBe(true);
  expect(app.restoreAdmissionPhase()).toBe('admitted');
});

it('keeps a stale hello from moving the current connection phase', async () => {
  const app = await setup();
  const old = app.beginRestoreConnection();
  app.beginRestoreConnection();
  expect(await app.acceptRestoreHello(old, null)).toBe(false);
  expect(app.restoreAdmissionPhase()).toBe('awaiting-hello');
});

it('keeps admission closed after late-source journal failure, and retries on reconnect', async () => {
  const app = await setup();
  const epoch = app.beginRestoreConnection();
  state.journal.mockRejectedValueOnce(new Error('journal unavailable'));
  await expect(app.acceptRestoreHello(epoch, null)).rejects.toThrow('journal unavailable');
  expect(app.restoreAdmissionReady()).toBe(false);
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), null)).toBe(true);
});


it('captures live source origins from the admitted page generation on a second restore', async () => {
  state.generation = 'first-restore';
  const app = await setup();
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), 'first-restore')).toBe(true);
  state.cards.set('uncertain', { threadId: 'old', body: { message: 'earlier', mode: 'human', image_hashes: ['confirmed'] },
    failedRetries: 0, settlement: { kind: 'follow-up' } });
  state.queued.set('old', { useCodingAgent: false, context: null });
  state.uploads.set('old', [{ localId: 'live-only', threadId: 'old', mime: 'image/png', previewUrl: '', contentHash: 'confirmed',
    file: { name: 'photo.png', size: 2, arrayBuffer: async () => new Uint8Array([4, 5]).buffer } as File,
    state: { kind: 'offline', reason: 'offline' } }]);
  expect(await app.acceptRestoreHello(app.beginRestoreConnection(), 'second-restore')).toBe(false);
  const intent = state.preserve.mock.calls[0][1];
  expect(intent.unsentMessages?.[0].sourceGeneration).toBe('first-restore');
  expect(intent.queuedUploadSends?.[0].sourceGeneration).toBe('first-restore');
  expect(intent.drafts.find((d) => d.uploads?.length)?.uploads?.[0].sourceGeneration).toBe('first-restore');
  expect(intent.drafts.find((d) => d.unsentMessage)?.uploads).toHaveLength(1);
});

it('requires an admitted restored connection for explicit source review actions', async () => {
  state.generation = 'restored';
  state.recoveries = [{ pageId: 'old-page', generation: 'restored', intent: {
    drafts: [{ threadId: 'old', draft: { text: 'retained', image_hashes: [], mode: null }, selection: {} }],
    pendingSelection: {},
  } }];
  const app = await setup();
  const reference = { pageId: 'old-page', generation: 'restored', sourceThreadId: 'old' };
  await expect(app.copyRetainedRestoreSource(reference)).rejects.toThrow('not ready');
  const epoch = app.beginRestoreConnection();
  expect(await app.acceptRestoreHello(epoch, 'restored')).toBe(true);
  state.create.mockClear(); state.complete.mockClear();
  expect((await app.readRetainedRestoreSources())[0].source.draft.text).toBe('retained');
  expect(state.create).not.toHaveBeenCalled();
  expect(await app.copyRetainedRestoreSource(reference)).toBe('new');
  expect(state.create).toHaveBeenCalledExactlyOnceWith('new', 'restored', state.recoveries[0].intent.drafts[0]);
  expect(state.complete).toHaveBeenCalledWith(state.recoveries[0], 'restored', 'new', true);
  app.beginRestoreConnection();
  await expect(app.copyRetainedRestoreSource(reference)).rejects.toThrow('not ready');
  await expect(app.readRetainedRestoreSources()).rejects.toThrow('connection changed');
});

// A page loading after a restore starts SDK calls before its first hello. The
// hello matches the page baseline, so those calls belong to that generation.
// A changed generation reloads the page instead, and the parked calls never send.
it('admits SDK mutations begun before the first hello at the page baseline', async () => {
  const generation = '11111111-1111-4111-8111-111111111111';
  state.generation = generation;
  const app = await setup();
  Object.assign(document, { querySelector: () => null });
  const { notifications } = await import('../../../../packages/lucidos-sdk/src/notifications');
  const fetch = vi.fn(async (_url: unknown, _init?: RequestInit) => new Response(null, { status: 204 }));
  vi.stubGlobal('fetch', fetch);
  const epoch = app.beginRestoreConnection();
  const pending = notifications.markAllRead();
  await Promise.resolve();
  expect(fetch).not.toHaveBeenCalled();
  expect(await app.acceptRestoreHello(epoch, generation)).toBe(true);
  await pending;
  await notifications.markAllRead();
  expect(fetch).toHaveBeenCalledTimes(2);
  for (const [, init] of fetch.mock.calls) {
    expect(new Headers(init?.headers).get('x-lucidos-restore-generation')).toBe(generation);
  }
});
