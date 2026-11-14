/** Real browser IndexedDB contract, isolated behind a synthetic routed origin.
 * No engine, live device store, registry, secrets or workspace database. */
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
        // Abort the transaction after its first queued write. Both active and source
        // history must roll back together; a retry after commit must deduplicate.
        const db = await new Promise<IDBDatabase>(resolve => { const r = indexedDB.open(name, 3); r.onsuccess = () => resolve(r.result); });
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
    // Settled and superseded revisions leave history; only the live one stays.
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
    const migrated = await a.listUploadHistory();
    const refused = await new Promise(resolve => {
      const r = indexedDB.open(name, 2);
      r.onerror = () => resolve(r.error.name);
      r.onsuccess = () => { r.result.close(); resolve('unexpected success'); };
    });
    await a.patchUpload('image', { ownerId: 'adopter', landedHash: 'landed' });
    const current = { ...old, ownerId: 'current', sourceGeneration: 'after', bytes: new Uint8Array([4, 5, 6]).buffer };
    await Promise.all([a.putUpload(old), b.putUpload(current)]);
    await a.deleteUpload('image');
    await b.putUpload(current);
    // A quota/abort during archive insertion must roll back the active put too.
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
    return { closed, refused, migrated: summarize(migrated), aborted, collisionRefused,
      activeAfterAbort: summarize(activeAfterAbort), active: await a.listUploads(), history: summarize(await a.listUploadHistory()) };
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
