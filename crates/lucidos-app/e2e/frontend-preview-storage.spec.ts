import { test, expect } from '@playwright/test';
import { buildSync } from 'esbuild';
import { fileURLToPath } from 'node:url';
import { stampPreviewBase } from '../vite/frontendPreviewPath';

declare const previewStorage: {
  installWorkspaceStorage: typeof import('../src/utils/workspaceStorage').installWorkspaceStorage;
  deviceDatabaseName: typeof import('../src/store/deviceDatabase').deviceDatabaseName;
};
const bundle = buildSync({
  stdin: {
    contents: "export { installWorkspaceStorage } from './src/utils/workspaceStorage'; export { deviceDatabaseName } from './src/store/deviceDatabase';",
    resolveDir: fileURLToPath(new URL('..', import.meta.url)),
    loader: 'ts',
  },
  bundle: true, write: false, format: 'iife', globalName: 'previewStorage',
}).outputFiles[0].text;

test('a preview preserves the ordinary client drafts and IndexedDB outbox', async ({ page }) => {
  const html = '<!doctype html><head><title>Preview storage</title></head><body></body>';
  await page.route('http://preview.test/**', route => route.fulfill({
    body: route.request().url().includes('/~preview/') ? stampPreviewBase(html, '/demo/') : html,
    contentType: 'text/html',
  }));
  await page.goto('http://preview.test/demo/');
  await page.evaluate(async () => {
    localStorage.setItem('ws:demo:lucidos-draft', 'unsent ordinary draft');
    await new Promise<void>((resolve, reject) => {
      const request = indexedDB.open('lucidos-unsent-messages:demo', 1);
      request.onupgradeneeded = () => request.result.createObjectStore('messages');
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        const tx = db.transaction('messages', 'readwrite');
        tx.objectStore('messages').put('ordinary queued message', 'message');
        tx.oncomplete = () => { db.close(); resolve(); };
        tx.onabort = () => reject(tx.error);
      };
    });
  });
  await page.goto('http://preview.test/~preview/0123456789abcdef/demo/');
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(() => {
    previewStorage.installWorkspaceStorage(localStorage, 'demo');
    const before = localStorage.getItem('lucidos-draft');
    localStorage.setItem('lucidos-draft', 'preview draft');
    sessionStorage.setItem('lucidos-draft', 'preview session');
    return { before, database: previewStorage.deviceDatabaseName('lucidos-unsent-messages', 'demo') };
  })).toEqual({ before: null, database: 'preview:lucidos-unsent-messages:demo' });
  await page.goto('http://preview.test/demo/');
  expect(await page.evaluate(async () => {
    const message = await new Promise<unknown>((resolve, reject) => {
      const request = indexedDB.open('lucidos-unsent-messages:demo');
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        const read = db.transaction('messages').objectStore('messages').get('message');
        read.onsuccess = () => { db.close(); resolve(read.result); };
        read.onerror = () => reject(read.error);
      };
    });
    return { draft: localStorage.getItem('ws:demo:lucidos-draft'), message,
      preview: localStorage.getItem('preview:ws:demo:lucidos-draft'),
      session: sessionStorage.getItem('preview:ws:demo:lucidos-draft') };
  })).toEqual({ draft: 'unsent ordinary draft', message: 'ordinary queued message',
    preview: 'preview draft', session: 'preview session' });
});
