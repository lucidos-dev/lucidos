/** Production runtime, real IndexedDB/Web Locks and actual page reloads.
 * The routed origin is disposable; no engine or user workspace is contacted. */
import { test, expect } from '@playwright/test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

declare const restore: typeof import('../src/store/restoreRuntime')
  & typeof import('../src/utils/restoreAdmission')
  & typeof import('../src/store/composeDrafts')
  & typeof import('../src/store/restoreGenerationRecords')
  & typeof import('../src/api/client/_core')
  & typeof import('../src/store/actions/thread-sync')
  & typeof import('../src/components/chat/prompt-input-helpers')
  & typeof import('../src/store/pendingUploads')
  & typeof import('../src/store/actions/app-bridge')
  & typeof import('../src/store/pendingUploadRecords')
  & typeof import('../src/store/unsentMessageRecords')
  & Pick<typeof import('../src/store/actions/pendingUploadRestore'), 'restorePendingUploads'>
  & Pick<typeof import('../src/store/actions/unsentMessageRestore'), 'restoreUnsentMessages'>
  & { sdkRequest: typeof import('../../../packages/lucidos-sdk/src/_fetch').request;
      sdkSse: typeof import('../../../packages/lucidos-sdk/src/sse').sse }
  & Pick<typeof import('../src/store/store'), 'backupProgress'>;
const origin = 'http://localhost:43119';
let bundle: string;
test.beforeAll(async () => {
  const result = await build({
    stdin: { contents: `export * from './src/store/restoreRuntime';
      export * from './src/utils/restoreAdmission';
      export * from './src/store/composeDrafts';
      export * from './src/store/restoreGenerationRecords';
      export * from './src/api/client/_core';
      export { connectThreadEvents, disconnectThreadEvents } from './src/store/actions/thread-sync';
      export { backupProgress } from './src/store/store';
      export * from './src/components/chat/prompt-input-helpers';
      export * from './src/store/pendingUploads';
      export * from './src/store/actions/app-bridge';
      export * from './src/store/pendingUploadRecords';
      export * from './src/store/unsentMessageRecords';
      export { restorePendingUploads } from './src/store/actions/pendingUploadRestore';
      export { restoreUnsentMessages } from './src/store/actions/unsentMessageRestore';
      export { request as sdkRequest } from '../../packages/lucidos-sdk/src/_fetch';
      export { sse as sdkSse } from '../../packages/lucidos-sdk/src/sse';`,
      resolveDir: fileURLToPath(new URL('..', import.meta.url)), loader: 'ts' },
    bundle: true, write: false, format: 'iife', globalName: 'restore',
    define: { 'import.meta.env.DEV': 'false' },
    // Match Vite's SDK source alias; the package main is an iframe IIFE.
    alias: { '@lucidos/sdk': fileURLToPath(new URL('../../../packages/lucidos-sdk/src/index.ts', import.meta.url)) },
    plugins: [{ name: 'build-identity', setup(b) {
      b.onResolve({ filter: /^virtual:build-id$/ }, () => ({ path: 'build-id', namespace: 'fixture' }));
      b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const CLIENT_BUILD_ID = "restore-browser-contract";' }));
    } }],
  });
  bundle = result.outputFiles[0].text;
});

test.beforeEach(async ({ page }) => {
  await page.route(`${origin}/**`, route => {
    if (route.request().resourceType() === 'document') return route.fulfill({
      contentType: 'text/html', body: '<!doctype html><title>restore runtime contract</title>',
    });
    return route.abort();
  });
  await page.goto(origin);
  await page.addScriptTag({ content: bundle });
});

// Only the browser's socket is controlled. The production transport opener,
// connectThreadEvents serialization, restore runtime and IndexedDB stay real.
test('lower-sequence hello journals before subsequent production stream frames', async ({ page }) => {
  const generation = '10000000-0000-4000-8000-000000000001';
  await page.evaluate(() => {
    Object.defineProperty(window, 'SharedWorker', { value: undefined, configurable: true });
    class Socket {
      onmessage: ((event: MessageEvent) => void) | null = null;
      onopen: (() => void) | null = null;
      onerror: (() => void) | null = null;
      constructor(_url: string) { (window as unknown as { socket: Socket }).socket = this; }
      addEventListener() {}
      close() {}
    }
    Object.defineProperty(window, 'EventSource', { value: Socket, configurable: true });
    restore.installRestoreRuntime();
    restore.connectThreadEvents();
  });
  await page.evaluate(() => {
    const socket = (window as unknown as { socket: EventSource }).socket;
    socket.onopen!(new Event('open'));
    socket.onmessage!(new MessageEvent('message', { data: JSON.stringify({
      type: 'StreamHello', seq: 9000, data: { restore_generation: null },
    }) }));
  });
  await expect.poll(() => page.evaluate(() => restore.restoreAdmissionReady())).toBe(true);
  await page.evaluate(() => {
    const socket = (window as unknown as { socket: EventSource }).socket;
    socket.onmessage!(new MessageEvent('message', { data: JSON.stringify({
      type: 'BackupProgress', seq: 9001, data: { phase: 'admitted before restore', progress: 3, total: 4 },
    }) }));
  });
  await expect.poll(() => page.evaluate(() => restore.backupProgress.peek()))
    .toEqual({ phase: 'admitted before restore', progress: 3, total: 4 });
  await page.evaluate(() => restore.patchDraft('stream-composer', { text: 'retain through the stream hello' }));
  let reloads = 0;
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) reloads++; });
  await page.evaluate(generation => {
    const socket = (window as unknown as { socket: EventSource }).socket;
    socket.onopen!(new Event('open'));
    // A restored log can restart below the old stream's sequence. Queue a
    // data frame immediately behind its hello while IDB work is outstanding.
    for (const frame of [
      { type: 'StreamHello', seq: 1, data: { restore_generation: generation } },
      { type: 'BackupProgress', seq: 2, data: { phase: 'must not dispatch', progress: 1, total: 2 } },
    ]) socket.onmessage!(new MessageEvent('message', { data: JSON.stringify(frame) }));
    // Let the new document inspect the old document's state at navigation.
    window.addEventListener('pagehide', () => sessionStorage.setItem('stream-result', JSON.stringify({
      progress: restore.backupProgress.peek(), ready: restore.restoreAdmissionReady(),
    })));
  }, generation);
  await expect.poll(() => reloads).toBe(1);
  await page.waitForLoadState();
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(async () => ({
    prior: JSON.parse(sessionStorage.getItem('stream-result')!),
    generation: await restore.createRestoreGenerationStore(null, 'inspector').readGeneration(),
    texts: (await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries())
      .flatMap(record => record.intent.drafts.map(draft => draft.draft.text)),
  }))).toEqual({ prior: { progress: { phase: 'admitted before restore', progress: 3, total: 4 }, ready: false }, generation, texts: ['retain through the stream hello'] });
});


for (const initial of [null, '10000000-0000-4000-8000-000000000001']) {
  test(`SDK socket fences late responses and subsequent writes from ${initial}`, async ({ page }) => {
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const requests: string[] = [];
    await page.route(`${origin}/api/v1/sdk-probe`, async route => {
      requests.push(route.request().headers()['x-lucidos-restore-generation']);
      await held;
      await route.fulfill({ contentType: 'application/json', body: '{"stale":true}' });
    });
    await page.evaluate(initial => {
      Object.defineProperty(window, 'SharedWorker', { value: undefined, configurable: true });
      class Socket {
        onmessage: ((event: MessageEvent) => void) | null = null;
        onopen: (() => void) | null = null;
        onerror: (() => void) | null = null;
        constructor(_url: string) { (window as unknown as { socket: Socket }).socket = this; }
        addEventListener() {}
        close() {}
      }
      Object.defineProperty(window, 'EventSource', { value: Socket, configurable: true });
      restore.sdkSse.connect();
      const socket = (window as unknown as { socket: EventSource }).socket;
      socket.onmessage!(new MessageEvent('message', { data: JSON.stringify({
        type: 'StreamHello', data: { restore_generation: initial },
      }) }));
      (window as unknown as { result: Promise<string> }).result = restore.sdkRequest('/sdk-probe')
        .then(() => 'accepted stale response', err => err.message);
    }, initial);
    await expect.poll(() => requests.length).toBe(1);
    await page.evaluate(() => {
      const socket = (window as unknown as { socket: EventSource }).socket;
      socket.onmessage!(new MessageEvent('message', { data: JSON.stringify({
        type: 'StreamHello', data: { restore_generation: '20000000-0000-4000-8000-000000000002' },
      }) }));
    });
    release();
    expect(await page.evaluate(() => (window as unknown as { result: Promise<string> }).result))
      .toContain('Workspace restored');
    expect(await page.evaluate(() => restore.sdkRequest('/sdk-probe', { method: 'POST' })
      .then(() => 'sent stale write', err => err.message))).toContain('Workspace restored');
    expect(requests).toEqual([initial ?? 'none']);
  });
}

test('production upload queue is retained through reload without dispatch', async ({ page }) => {
  const recovered: Array<Record<string, unknown>> = [];
  await page.route(`${origin}/api/v1/threads`, route => {
    recovered.push(route.request().postDataJSON());
    return route.fulfill({ contentType: 'application/json', body: '{}' });
  });
  let reloads = 0;
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) reloads++; });
  await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.patchDraft('queued-composer', { text: 'review this queued send' });
    const file = new File(['retained upload bytes'], 'queued.png', { type: 'image/png' });
    restore.addPendingUpload({ threadId: 'queued-composer', localId: 'queued-image', file,
      mime: file.type, previewUrl: URL.createObjectURL(file), state: { kind: 'finishing' } });
    restore.queueUploadSend('queued-composer', { useCodingAgent: false, context: { note: 'retained context' } });
    let dispatched = 0;
    restore.settleQueuedUploadSends(() => { dispatched++; });
    if (dispatched !== 0) throw new Error('Queue ignored in-flight upload');
    // begin closes admission synchronously; the queued intent and its bytes
    // must survive capture without dispatch, including after the real reload.
    const connection = restore.beginRestoreConnection();
    window.addEventListener('pagehide', () => sessionStorage.setItem('queue-dispatched', String(dispatched)));
    void restore.acceptRestoreHello(connection, 'restored-generation');
    restore.settleQueuedUploadSends(() => { dispatched++; });
  });
  await expect.poll(() => reloads).toBe(1);
  await page.waitForLoadState();
  expect(await page.evaluate(() => sessionStorage.getItem('queue-dispatched'))).toBe('0');
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(async () => {
    const records = await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries();
    return {
      queue: records.flatMap(r => r.intent.queuedUploadSends ?? []).map(q => ({ text: q.text, intent: q.intent })),
      bytes: records.flatMap(r => r.intent.drafts.flatMap(d => d.uploads ?? []))
        .map(u => new TextDecoder().decode(u.bytes)),
    };
  })).toEqual({ queue: [{ text: 'review this queued send', intent: { useCodingAgent: false, context: { note: 'retained context' } } }],
    bytes: ['retained upload bytes'] });
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    const admitted = await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
    let dispatched = 0;
    restore.settleQueuedUploadSends(() => { dispatched++; });
    return { admitted, dispatched, queueSize: restore.queuedUploadSends.peek().size };
  })).toEqual({ admitted: true, dispatched: 0, queueSize: 0 });
  expect(recovered.length).toBeGreaterThan(0);
  expect(recovered.every(r => r.recovered_compose && !r.message)).toBe(true);
});


test('real iframe bridge does not release queued calls into a restored generation', async ({ page }) => {
  const requests: string[] = [];
  const replies: Array<() => Promise<void>> = [];
  await page.route(`${origin}/api/v1/threads/list?*`, route => {
    requests.push(route.request().url());
    expect(route.request().headers()['x-lucidos-restore-generation']).toBe('none');
    replies.push(() => route.fulfill({ contentType: 'application/json', body: '{}' }));
  });
  await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.installAppBridge();
    const frame = document.createElement('iframe');
    frame.dataset.role = 'app-ui-frame';
    frame.src = '/app/restore-probe/';
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.srcdoc = `<script>
      for (let i = 0; i < 4; i++) parent.postMessage({ type: 'lucidos:bridge', id: 'call-' + i,
        op: 'fetch', args: { path: '/threads/list?n=' + i, method: 'GET', headers: {},
          body: { kind: 'none' }, timeoutMs: 30000 } }, '*');
    </script>`;
    document.body.appendChild(frame);
  });
  await expect.poll(() => requests.length).toBe(3);
  let reloads = 0;
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) reloads++; });
  await page.evaluate(() => {
    // These original-generation calls were admitted by the real postMessage
    // sender check. Closing runtime admission must fence their late replies
    // and the fourth invocation waiting in the production fetch lane.
    const connection = restore.beginRestoreConnection();
    void restore.acceptRestoreHello(connection, 'restored-generation');
  });
  await Promise.all(replies.map(reply => reply().catch(() => {})));
  await expect.poll(() => reloads).toBe(1);
  await page.waitForLoadState();
  expect(requests).toHaveLength(3);
  expect(requests.some(url => url.endsWith('n=3'))).toBe(false);
});


for (const initial of [null, '10000000-0000-4000-8000-000000000001']) {
  test(`idempotent HTTP retry and admission reopen preserve ${initial}`, async ({ page }) => {
    const requests: string[] = [];
    await page.route(`${origin}/api/v1/retry-probe`, async route => {
      requests.push(route.request().headers()['x-lucidos-restore-generation']);
      if (requests.length === 1) return route.abort('failed');
      return route.fulfill({ body: 'accepted' });
    });
    await page.evaluate(async initial => {
      await restore.createRestoreGenerationStore(null, 'prior-page').observe(initial);
      restore.installRestoreRuntime();
      await restore.acceptRestoreHello(restore.beginRestoreConnection(), initial);
    }, initial);
    expect(await page.evaluate(async () => {
      const response = await restore.mutatingFetchIdempotent('/api/v1/retry-probe', { method: 'PATCH', body: 'same intent' });
      return response.text();
    })).toBe('accepted');
    expect(requests).toEqual([initial ?? 'none', initial ?? 'none']);

    // Close the real runtime gate before creating the invocation. No HTTP
    // dispatch is allowed until a same-generation hello admits this page.
    await page.evaluate(() => {
      const connection = restore.beginRestoreConnection();
      const probe = restore.mutatingFetchIdempotent('/api/v1/retry-probe', { method: 'PATCH', body: 'waiting intent' })
        .then(response => response.text());
      Object.assign(window, { admissionProbe: { connection, probe } });
    });
    // A browser task boundary lets the request reach its admission wait.
    expect(await page.evaluate(() => restore.restoreAdmissionReady())).toBe(false);
    expect(requests).toHaveLength(2);
    expect(await page.evaluate(async initial => {
      const { connection, probe } = (window as unknown as { admissionProbe: { connection: number; probe: Promise<string> } }).admissionProbe;
      if (!await restore.acceptRestoreHello(connection, initial)) throw new Error('Same generation refused');
      return probe;
    }, initial)).toBe('accepted');
    expect(requests).toEqual([initial ?? 'none', initial ?? 'none', initial ?? 'none']);
  });
}

test('upload settlement while admission is closed retains queued intent', async ({ page }) => {
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.patchDraft('gap-composer', { text: 'wait for admission' });
    const file = new File(['image bytes'], 'gap.png', { type: 'image/png' });
    restore.addPendingUpload({ threadId: 'gap-composer', localId: 'gap-image', file,
      mime: file.type, previewUrl: URL.createObjectURL(file), state: { kind: 'finishing' } });
    restore.queueUploadSend('gap-composer', { useCodingAgent: false });
    const connection = restore.beginRestoreConnection();
    restore.removePendingUpload('gap-composer', 'gap-image');
    let dispatched = 0;
    const dispatch = () => { dispatched++; };
    restore.settleQueuedUploadSends(dispatch);
    const closed = { dispatched, queued: restore.queuedUploadSends.peek().size };
    await restore.acceptRestoreHello(connection, null);
    restore.settleQueuedUploadSends(dispatch);
    restore.settleQueuedUploadSends(dispatch);
    return { closed, reopened: { dispatched, queued: restore.queuedUploadSends.peek().size } };
  })).toEqual({ closed: { dispatched: 0, queued: 1 }, reopened: { dispatched: 1, queued: 0 } });
});


test('startup retains historical upload and unsent bytes without requeue or dispatch', async ({ page }) => {
  await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.persistPendingUpload({ threadId: 'old-thread', localId: 'old-upload',
      name: 'old.png', mime: 'image/png', bytes: new TextEncoder().encode('original bytes').buffer });
    restore.persistQueuedUploadSend('old-thread', { useCodingAgent: false },
      { text: 'original queued text', localIds: ['old-upload'] });
    restore.persistSendingMessage({ eventId: 'old-send', threadId: 'old-thread',
      body: { message: 'original unsent text' }, settlement: { kind: 'follow-up' }, sentAt: new Date().toISOString() });
    await Promise.all([restore.readPendingUploadStore(), restore.readUnsentMessageStore()]);
  });
  await page.reload();
  await page.addScriptTag({ content: bundle });
  let reloads = 0;
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) reloads++; });
  await page.evaluate(() => {
    restore.installRestoreRuntime();
    void restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  });
  await expect.poll(() => reloads).toBe(1);
  await page.waitForLoadState();
  await page.addScriptTag({ content: bundle });
  const requests: Array<Record<string, unknown>> = [];
  await page.route(`${origin}/api/v1/**`, route => {
    requests.push({ method: route.request().method(), url: route.request().url(), body: route.request().postDataJSON() });
    return route.fulfill({ contentType: 'application/json', body: '{}' });
  });
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    const admitted = await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
    // Invoke the actual startup passes after runtime admission, exactly where
    // old source records would otherwise be adopted, uploaded or requeued.
    const uploads = await restore.restorePendingUploads();
    const sends = await restore.restoreUnsentMessages();
    let dispatched = 0;
    restore.settleQueuedUploadSends(() => { dispatched++; });
    const originals = await restore.readPendingUploadStore(true);
    const unsent = await restore.readUnsentMessageStore();
    const journal = await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries();
    return { admitted, uploads, sends, dispatched, queued: restore.queuedUploadSends.peek().size,
      bytes: originals.uploads.map(u => new TextDecoder().decode(u.bytes)),
      unsent: unsent.map(s => s.body.message),
      retainedQueue: journal.flatMap(r => r.intent.queuedUploadSends ?? []).map(q => q.text),
      retainedSends: journal.flatMap(r => r.intent.unsentMessages ?? []).map(s => s.body.message) };
  })).toEqual({ admitted: true,
    uploads: { restored: 0, gone: 0, expired: 0, unsentQueuedUploadSends: 0, unresolved: 0 },
    sends: { shown: 0, delivered: 0, movedToDraft: 0, unresolved: 0 }, dispatched: 0, queued: 0,
    bytes: ['original bytes'], unsent: ['original unsent text'],
    retainedQueue: ['original queued text'], retainedSends: ['original unsent text'] });
  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every(r => r.method === 'POST' && String(r.url).endsWith('/threads')
    && (r.body as Record<string, unknown>).recovered_compose && !(r.body as Record<string, unknown>).message)).toBe(true);
});
