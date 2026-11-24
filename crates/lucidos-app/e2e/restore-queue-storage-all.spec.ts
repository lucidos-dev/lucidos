import { test, expect } from '@playwright/test';
import { buildSync } from 'esbuild';
import { fileURLToPath } from 'node:url';
declare const storage: typeof import('../src/store/pendingUploadBackend');
const bundle = buildSync({
    entryPoints: [fileURLToPath(new URL('../src/store/pendingUploadBackend.ts', import.meta.url))],
    bundle: true, write: false, format: 'iife', globalName: 'storage',
}).outputFiles[0].text;
test('queue migration keeps the live source across writes, settlements, abort and reload', async ({ page }) => {
    await page.route('http://restore.test/**', route => route.fulfill({ body: '<!doctype html><title>storage test</title>', contentType: 'text/html' }));
    await page.goto('http://restore.test');
    await page.addScriptTag({ content: bundle });
    const result = await page.evaluate(async () => {
        const name = 'disposable-restore-queue';
        const old = { threadId: 'thread', ownerId: 'legacy', localIds: ['image'], text: 'old', intent: { useCodingAgent: false, context: null } };
        const legacy = await new Promise<IDBDatabase>((resolve, reject) => { const req = indexedDB.open(name, 1); req.onupgradeneeded = () => { req.result.createObjectStore('uploads', { keyPath: 'localId' }); req.result.createObjectStore('queued-upload-sends', { keyPath: 'threadId' }); }; req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error); });
        await new Promise((resolve, reject) => { const tx = legacy.transaction('queued-upload-sends', 'readwrite'); tx.objectStore('queued-upload-sends').put(old); tx.oncomplete = () => resolve(undefined); tx.onabort = reject; });
        let closed = false;
        legacy.onversionchange = () => { closed = true; legacy.close(); };
        const a = storage.createIndexedDbBackend(name), b = storage.createIndexedDbBackend(name);
        const migrated = await a.listQueuedUploadSends();
        const current = { ...old, ownerId: 'current', sourceGeneration: 'restored', text: 'new' };
        await Promise.all([a.putQueuedUploadSend(old), b.putQueuedUploadSend(current)]);
        await a.deleteQueuedUploadSend(old);
        const changed = { ...current, text: 'edited' };
        await b.putQueuedUploadSend(changed);
        await a.deleteQueuedUploadSend(current);
        const active = await a.listQueuedUploadSends();
        const refused = await new Promise(resolve => { const req = indexedDB.open(name, 1); req.onerror = () => resolve(req.error.name); req.onsuccess = () => { req.result.close(); resolve('unexpected success'); }; });
        const db = await new Promise<IDBDatabase>(resolve => { const r = indexedDB.open(name); r.onsuccess = () => resolve(r.result); });
        const original = IDBObjectStore.prototype.put;
        IDBObjectStore.prototype.put = function (...args) { const req = original.apply(this, args); if (this.name === 'queued-source-history')
            this.transaction.abort(); return req; };
        let aborted = false;
        try {
            await a.putQueuedUploadSend({ ...changed, text: 'must not land' });
        }
        catch {
            aborted = true;
        }
        finally {
            IDBObjectStore.prototype.put = original;
            db.close();
        }
        await a.putQueuedUploadSend(changed);
        await a.putQueuedUploadSend(changed);
        return { closed, migrated, active, refused, aborted, history: await b.listQueuedUploadHistory() };
    });
    expect(result.closed).toBe(true);
    expect(result.migrated.length).toBe(1);
    expect(result.active.length).toBe(1);
    expect(result.active[0].text).toBe('edited');
    expect(result.refused).toBe('VersionError');
    expect(result.aborted).toBe(true);
    expect(result.history.map(r => r.text)).toEqual(['edited']);
    await page.reload();
    await page.addScriptTag({ content: bundle });
    expect(await page.evaluate(async () => (await storage.createIndexedDbBackend('disposable-restore-queue').listQueuedUploadHistory()).length)).toBe(1);
});


test('v2 byte migration keeps sources through concurrent writes and abort, and deletion drops them', async ({ page }) => {
  await page.route('http://restore.test/**', route => route.fulfill({ body: '<!doctype html><title>byte test</title>', contentType: 'text/html' }));
  await page.goto('http://restore.test');
  await page.addScriptTag({ content: bundle });
  const result = await page.evaluate(async () => {
    const name = 'disposable-restore-bytes';
    const old = { localId: 'image', threadId: 'thread', ownerId: 'old', sourceGeneration: 'before',
      name: 'image.png', mime: 'image/png', size: 3, bytes: new Uint8Array([1, 2, 3]).buffer, createdAt: 1 };
    const legacy = await new Promise<IDBDatabase>((resolve, reject) => {
      const r = indexedDB.open(name, 2);
      r.onupgradeneeded = () => {
        r.result.createObjectStore('uploads', { keyPath: 'localId' });
        r.result.createObjectStore('queued-upload-sends', { keyPath: 'threadId' });
        r.result.createObjectStore('queued-source-history');
      };
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = legacy.transaction('uploads', 'readwrite');
      tx.objectStore('uploads').put(old);
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
    });
    let closed = false;
    legacy.onversionchange = () => { closed = true; legacy.close(); };
    const a = storage.createIndexedDbBackend(name), b = storage.createIndexedDbBackend(name);
    const migrated = await Promise.all((await a.listUploadHistory()).map(async (row) => ({
      owner: row.ownerId, bytes: [...new Uint8Array((await a.readUploadBytes(row))!)],
    })));
    const refused = await new Promise(resolve => {
      const r = indexedDB.open(name, 2);
      r.onerror = () => resolve(r.error.name);
      r.onsuccess = () => { r.result.close(); resolve('unexpected success'); };
    });
    await a.patchUpload('image', { ownerId: 'adopter', landedHash: 'landed' });
    const current = { ...old, ownerId: 'current', sourceGeneration: 'after', createdAt: 2, bytes: new Uint8Array([4, 5, 6]).buffer };
    await Promise.all([a.putUpload(old), b.putUpload(current)]);
    await a.deleteUpload('image');
    await b.putUpload(current);
    const original = IDBObjectStore.prototype.add;
    IDBObjectStore.prototype.add = function (...args) {
      const req = original.apply(this, args);
      if (this.name === 'upload-source-history') this.transaction.abort();
      return req;
    };
    let aborted = false;
    try { await a.putUpload({ ...old, ownerId: 'must-not-land' }); }
    catch { aborted = true; }
    finally { IDBObjectStore.prototype.add = original; }
    const activeAfterAbort = await b.listUploads();
    let collisionRefused = false;
    try { await a.putUpload({ ...current, bytes: new Uint8Array([7, 8, 9]).buffer }); }
    catch { collisionRefused = true; }
    await b.deleteUpload('image');
    await a.putUpload(old); await a.putUpload(old); await a.deleteUpload('image');
    const summarize = (rows: typeof old[]) => rows.map(r => ({ owner: r.ownerId, bytes: [...new Uint8Array(r.bytes)] }));
    return { closed, refused, migrated, aborted, collisionRefused,
      activeAfterAbort: summarize(activeAfterAbort), active: await a.listUploads(), history: await a.listUploadHistory() };
  });
  expect(result.closed).toBe(true);
  expect(result.refused).toBe('VersionError');
  expect(result.migrated).toEqual([{ owner: 'old', bytes: [1, 2, 3] }]);
  expect(result.aborted).toBe(true);
  expect(result.collisionRefused).toBe(true);
  expect(result.activeAfterAbort).toEqual([{ owner: 'current', bytes: [4, 5, 6] }]);
  expect(result.active).toEqual([]);
  expect(result.history).toEqual([]);
  await page.reload();
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(async () => {
    const backend = storage.createIndexedDbBackend('disposable-restore-bytes');
    return { active: (await backend.listUploads()).length, history: (await backend.listUploadHistory()).length };
  })).toEqual({ active: 0, history: 0 });
});

test('v3 archive migration and repeated adoption share bytes while preserving legacy variants', async ({ page }) => {
  await page.route('http://restore.test/**', route => route.fulfill({ body: '<!doctype html>', contentType: 'text/html' }));
  await page.goto('http://restore.test');
  await page.addScriptTag({ content: bundle });
  const result = await page.evaluate(async () => {
    const name = 'disposable-v3-archive';
    const upload = { localId: 'image', threadId: 'thread', ownerId: 'old', landedHash: 'landed',
      name: 'image.png', mime: 'image/png', size: 1, bytes: new Uint8Array([1]).buffer, createdAt: 1 };
    const open = (version?: number) => new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(name, version);
      request.onupgradeneeded = () => {
        const db = request.result;
        db.createObjectStore('uploads', { keyPath: 'localId' }).put(upload);
        db.createObjectStore('queued-upload-sends', { keyPath: 'threadId' });
        db.createObjectStore('queued-source-history');
        const history = db.createObjectStore('upload-source-history');
        const revisions = [upload, { ...upload, ownerId: 'previous' },
          { ...upload, ownerId: 'variant', bytes: new Uint8Array([2]).buffer }];
        for (const record of revisions) {
          history.add(record, JSON.stringify([record.localId, record.threadId, record.ownerId, null,
            record.name, record.mime, record.size, record.createdAt, record.landedHash]));
        }
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const old = await open(3);
    let closed = false;
    old.onversionchange = () => { closed = true; old.close(); };
    const backend = storage.createIndexedDbBackend(name);
    const [active] = await backend.listUploads();
    for (let i = 0; i < 20; i += 1) await backend.patchUpload('image', { ownerId: `owner-${i}` });
    const db = await open();
    const keys = () => new Promise<IDBValidKey[]>((resolve, reject) => {
      const request = db.transaction('upload-bytes').objectStore('upload-bytes').getAllKeys();
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const history = await backend.listUploadHistory();
    const count = (await keys()).length;
    const payloads = await Promise.all(history.filter(row => ['old', 'variant'].includes(row.ownerId)).map(async row =>
      [...new Uint8Array((await backend.readUploadBytes(row))!)]));
    const refused = await new Promise(resolve => {
      const request = indexedDB.open(name, 3);
      request.onerror = () => resolve(request.error.name);
      request.onsuccess = () => { request.result.close(); resolve('unexpected success'); };
    });
    await backend.deleteUpload('image');
    const remaining = (await keys()).length;
    const stale = history.find(row => row.ownerId === 'old')!;
    await backend.putUpload({ ...upload, ownerId: 'replacement', bytes: new Uint8Array([3]).buffer });
    const staleRead = await backend.readUploadBytes(stale);
    db.close();
    return { closed, refused, count, payloads, versions: history.length, embeddedBytes: history.some(row => 'bytes' in row),
      remaining, staleRead, activeProvenance: active.migratedBytes };
  });
  expect(result).toEqual({ closed: true, refused: 'VersionError', count: 2, payloads: [[1], [2]],
    versions: 23, embeddedBytes: false, remaining: 0, staleRead: undefined, activeProvenance: true });
});

declare const journal: typeof import('../src/store/restoreGenerationRecords');
const journalBundle = buildSync({
  entryPoints: [fileURLToPath(new URL('../src/store/restoreGenerationRecords.ts', import.meta.url))],
  bundle: true, write: false, format: 'iife', globalName: 'journal',
}).outputFiles[0].text;

test('v1 journal migration shares equal bytes and preserves distinct variants and destinations', async ({ page }) => {
  await page.route('http://restore.test/**', route => route.fulfill({ body: '<!doctype html>', contentType: 'text/html' }));
  await page.goto('http://restore.test');
  await page.addScriptTag({ content: journalBundle });
  const result = await page.evaluate(async () => {
    const name = journal.restoreGenerationDbName('migration');
    const upload = { localId: 'file', ownerId: 'original', threadId: 'thread', name: 'image.png',
      mime: 'image/png', size: 1, createdAt: 1, bytes: new Uint8Array([1]).buffer };
    const allocation = { generation: 'restored', drafts: [{ sourceThreadId: 'thread', threadId: 'destination' }],
      completed: ['destination'] };
    const recoveries = [
      upload, { ...upload, ownerId: 'adopted', landedHash: 'landed', sourceGeneration: 'before' },
      { ...upload, ownerId: 'variant', bytes: new Uint8Array([2]).buffer },
    ].map((record, i) => ({
      pageId: record.ownerId, generation: i ? 'restored' : null,
      materializations: [allocation], reviewedMaterializations: [allocation],
      intent: { captureOwnerId: record.ownerId, pendingSelection: { model: 'selected' },
        drafts: [{ threadId: 'thread', draft: { text: 'original draft', image_hashes: [], mode: null },
          selection: {}, uploads: [record] }] },
    }));
    const open = (version?: number) => new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(name, version);
      request.onupgradeneeded = () => request.result.createObjectStore('observations')
        .add({ generation: 'restored', recoveries }, 'workspace');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const old = await open(1);
    let closed = false;
    old.onversionchange = () => { closed = true; old.close(); };
    const store = journal.createRestoreGenerationStore('migration', 'observer');
    const records = await store.readRecoveries();
    const attachments = records.map((record) => record.intent.drafts[0].uploads![0]);
    const db = await open();
    const count = await new Promise<number>((resolve, reject) => {
      const request = db.transaction('upload-bytes').objectStore('upload-bytes').count();
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    db.close();
    const materialized = [];
    for (const { intent } of records) materialized.push(await store.materializeDraft(intent.drafts[0]));
    const refused = await open(1).then((db) => { db.close(); return 'unexpected success'; }, (error) => error.name);
    return { closed, refused, count, generation: await store.readGeneration(),
      embedded: attachments.some(attachment => 'bytes' in attachment),
      variants: attachments.map(attachment => attachment.byteKey[1]),
      payloads: materialized.map(draft => [...new Uint8Array(draft.uploads![0].bytes)]),
      landedHash: attachments[1].landedHash, sourceGeneration: attachments[1].sourceGeneration,
      contextPreserved: records.every((record, i) => record.pageId === recoveries[i].pageId &&
        record.generation === recoveries[i].generation && record.intent.captureOwnerId === recoveries[i].intent.captureOwnerId &&
        record.intent.pendingSelection.model === 'selected' && record.intent.drafts[0].draft.text === 'original draft' &&
        record.materializations?.[0].completed[0] === 'destination' &&
        record.reviewedMaterializations?.[0].drafts[0].threadId === 'destination'),
    };
  });
  expect(result).toEqual({ closed: true, refused: 'VersionError', count: 2, generation: 'restored',
    embedded: false, variants: [0, 0, 1], payloads: [[1], [1], [2]], landedHash: 'landed',
    sourceGeneration: 'before', contextPreserved: true });
});

test('historical capture survives deletion after listing and reports missing retained bytes', async ({ page }) => {
  await page.route('http://restore.test/**', route => route.fulfill({ body: '<!doctype html>', contentType: 'text/html' }));
  await page.goto('http://restore.test');
  await page.addScriptTag({ content: bundle });
  await page.addScriptTag({ content: journalBundle });
  const result = await page.evaluate(async () => {
    const name = 'disposable-archive-delete';
    const backend = storage.createIndexedDbBackend(name);
    const upload = { localId: 'deleted-file', threadId: 'thread', ownerId: 'old', name: 'image.png',
      mime: 'image/png', size: 1, bytes: new Uint8Array([1]).buffer, createdAt: 1 };
    await backend.putUpload(upload);
    await backend.putUpload({ ...upload, localId: 'retained-file' });
    const listed = await backend.listUploadHistory();
    const queue = { ownerId: 'old', threadId: 'thread', text: 'queued', intent: { useCodingAgent: false, context: null },
      localIds: ['deleted-file', 'retained-file'] };
    const draft = { threadId: 'standalone', draft: { text: '', image_hashes: [], mode: null }, selection: {} };
    const observation = journal.createRestoreGenerationStore('archive-delete', 'observer');
    await observation.observe('restored');

    await backend.deleteUpload('deleted-file');
    await observation.journalHistorical('restored', { drafts: [
      { ...draft, uploads: listed.filter(source => source.localId === 'deleted-file') },
      { ...draft, threadId: 'queued', uploads: listed, queuedUploadSend: queue },
    ], pendingSelection: {} }, 'capture', backend.readUploadBytes);
    const { intent } = (await observation.readRecoveries())[0];

    const [retained] = await backend.listUploadHistory();
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(name);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('upload-bytes', 'readwrite');
      tx.objectStore('upload-bytes').delete(retained.byteKey);
      tx.oncomplete = () => resolve();
      tx.onabort = () => reject(tx.error);
    });
    db.close();
    const missing = await backend.readUploadBytes(retained).then(() => null, error => error.message);
    return { drafts: intent.drafts.map(source => ({ threadId: source.threadId, ids: source.uploads?.map(u => u.localId) })),
      queues: intent.queuedUploadSends?.map(source => source.text), missing };
  });
  expect(result).toEqual({ drafts: [{ threadId: 'queued', ids: ['retained-file'] }],
    queues: ['queued'], missing: 'Archived upload bytes are missing' });
});
