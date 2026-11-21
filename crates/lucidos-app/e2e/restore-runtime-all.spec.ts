/** Production runtime, real IndexedDB/Web Locks and actual page reloads.
 * The routed origin is disposable; no engine or user workspace is contacted. */
import { test, expect } from '@playwright/test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

declare const restore: typeof import('../src/store/restoreRuntime')
  & typeof import('../src/utils/restoreAdmission')
  & typeof import('../src/utils/restoreSourceGeneration')
  & typeof import('../src/store/composeDrafts')
  & typeof import('../src/store/restoreGenerationRecords')
  & typeof import('../src/api/client/_core');
const origin = 'http://localhost:43119';
let bundle: string;
test.beforeAll(async () => {
  const result = await build({
    stdin: { contents: `export * from './src/store/restoreRuntime';
      export * from './src/utils/restoreAdmission';
      export * from './src/utils/restoreSourceGeneration';
      export * from './src/store/composeDrafts';
      export * from './src/store/restoreGenerationRecords';
      export * from './src/api/client/_core';`,
      resolveDir: fileURLToPath(new URL('..', import.meta.url)), loader: 'ts' },
    bundle: true, write: false, format: 'iife', globalName: 'restore',
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

test('first and same null hello admit durably; superseded connection cannot admit', async ({ page }) => {
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    const old = restore.beginRestoreConnection();
    const before = restore.restoreAdmissionReady();
    const current = restore.beginRestoreConnection();
    const stale = await restore.acceptRestoreHello(old, 'obsolete');
    const first = await restore.acceptRestoreHello(current, null);
    const same = await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    const store = restore.createRestoreGenerationStore(null, 'inspector');
    return { before, stale, first, same, ready: restore.restoreAdmissionReady(),
      generation: await store.readGeneration(), records: await store.readRecoveries() };
  })).toEqual({ before: false, stale: false, first: true, same: true, ready: true, generation: null, records: [] });
});

for (const baseline of [undefined, null, 'previous-generation']) {
  test(`generation change from ${String(baseline)} captures before one real reload and recovers without sending`, async ({ page }) => {
    const requests: Array<Record<string, unknown>> = [];
    await page.route(`${origin}/api/v1/threads`, route => {
      requests.push(route.request().postDataJSON());
      return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
    });
    if (baseline !== undefined) await page.evaluate(async generation => {
      await restore.createRestoreGenerationStore(null, 'baseline').observe(generation);
    }, baseline);
    let reloads = 0;
    page.on('framenavigated', frame => { if (frame === page.mainFrame()) reloads++; });
    await page.evaluate(() => {
      restore.installRestoreRuntime();
      restore.patchDraft('old-composer', { text: 'keep this local edit' });
      // The reload destroys this execution context; report durable results from
      // the new page rather than intercepting or replacing location.reload.
      void restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
    });
    await expect.poll(() => reloads).toBe(1);
    await page.waitForLoadState();
    await page.addScriptTag({ content: bundle });
    expect(await page.evaluate(async () => {
      restore.installRestoreRuntime();
      const admitted = await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
      const records = await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries();
      return { admitted, ready: restore.restoreAdmissionReady(), texts: records.flatMap(r => r.intent.drafts.map(d => d.draft.text)) };
    })).toEqual({ admitted: true, ready: true, texts: ['keep this local edit'] });
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({ recovered_compose: { generation: 'restored-generation', text: 'keep this local edit' } });
    const destination = requests[0].id;
    expect(destination).not.toBe('old-composer');
    await page.reload();
    await page.addScriptTag({ content: bundle });
    expect(await page.evaluate(async () => {
      restore.installRestoreRuntime();
      return restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
    })).toBe(true);
    expect(requests).toHaveLength(1);
    expect(reloads).toBe(2); // exactly the restore reload and the explicit reload
  });
}

test('lost recovery reply retries the durable destination without replacing an edited draft', async ({ page }) => {
  const attempts: string[] = [];
  const server = new Map<string, string>();
  await page.route(`${origin}/api/v1/threads`, async route => {
    const body = route.request().postDataJSON();
    attempts.push(body.id);
    if (!server.has(body.id)) server.set(body.id, body.recovered_compose.text);
    if (attempts.length === 1) {
      server.set(body.id, 'edited after server commit');
      return route.abort('failed');
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
  });
  await page.evaluate(async () => {
    await restore.createRestoreGenerationStore(null, 'closed-page').preserve('restored-generation', {
      captureOwnerId: 'closed-page',
      drafts: [{ threadId: 'source', draft: { text: 'original local edit', image_hashes: [], mode: null }, selection: {} }],
      pendingSelection: {},
    });
    restore.installRestoreRuntime();
  });
  expect(await page.evaluate(async () => {
    try { await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation'); }
    catch { return { failed: true, ready: restore.restoreAdmissionReady() }; }
    return { failed: false, ready: restore.restoreAdmissionReady() };
  })).toEqual({ failed: true, ready: false });
  expect(attempts).toHaveLength(1);
  await page.reload();
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    return restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  })).toBe(true);
  expect(attempts).toEqual([attempts[0], attempts[0]]);
  expect([...server.values()]).toEqual(['edited after server commit']);
  expect(await page.evaluate(async () => {
    const records = await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries();
    return records.flatMap(r => r.intent.drafts.map(d => d.draft.text));
  })).toEqual(['original local edit']);
});

test('two live tabs preserve both composers and share durable recovery destinations', async ({ page, context }) => {
  const other = await context.newPage();
  await other.route(`${origin}/**`, route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>second tab</title>' }));
  await other.goto(origin);
  await other.addScriptTag({ content: bundle });
  const server = new Map<string, string>();
  const tabs = [page, other];
  for (const [index, tab] of tabs.entries()) {
    await tab.route(`${origin}/api/v1/threads`, route => {
      const body = route.request().postDataJSON();
      if (!server.has(body.id)) server.set(body.id, body.recovered_compose.text);
      return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
    });
    expect(await tab.evaluate(async index => {
      restore.installRestoreRuntime();
      const ready = await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
      restore.patchDraft(`composer-${index}`, { text: `local edit ${index}` });
      return ready;
    }, index)).toBe(true);
  }
  const navigated = tabs.map(tab => tab.waitForEvent('framenavigated', frame => frame === tab.mainFrame()));
  await Promise.all(tabs.map(tab => tab.evaluate(() => {
    void restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  })));
  await Promise.all(navigated);
  for (const tab of tabs) {
    await tab.waitForLoadState();
    await tab.addScriptTag({ content: bundle });
  }
  expect(await Promise.all(tabs.map(tab => tab.evaluate(async () => {
    restore.installRestoreRuntime();
    return restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  })))).toEqual([true, true]);
  expect([...server.values()].sort()).toEqual(['local edit 0', 'local edit 1']);
  expect(await page.evaluate(async () => {
    const records = await restore.createRestoreGenerationStore(null, 'inspector').readRecoveries();
    return records.flatMap(r => r.intent.drafts.map(d => d.draft.text)).sort();
  })).toEqual(['local edit 0', 'local edit 1']);
  await other.close();
});

test('aborted generation commit retains the old observation and retries before reload', async ({ page }) => {
  expect(await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.patchDraft('composer', { text: 'survives transaction abort' });
    const original = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (...args) {
      const request = original.apply(this, args);
      if (this.name === 'observations') this.transaction.abort();
      return request;
    };
    let failed = false;
    try { await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation'); }
    catch { failed = true; }
    finally { IDBObjectStore.prototype.put = original; }
    const store = restore.createRestoreGenerationStore(null, 'inspector');
    return { failed, ready: restore.restoreAdmissionReady(), inert: document.body.inert,
      generation: await store.readGeneration(), records: await store.readRecoveries() };
  })).toEqual({ failed: true, ready: false, inert: false, generation: null, records: [] });
  // No reload follows a failed commit, so editing resumes until the retry
  // captures again.
  const navigation = page.waitForEvent('framenavigated', frame => frame === page.mainFrame());
  await page.evaluate(() => {
    void restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  });
  await navigation;
  await page.waitForLoadState();
  await page.addScriptTag({ content: bundle });
  expect(await page.evaluate(async () => {
    const store = restore.createRestoreGenerationStore(null, 'inspector');
    return { generation: await store.readGeneration(), texts: (await store.readRecoveries()).flatMap(r => r.intent.drafts.map(d => d.draft.text)) };
  })).toEqual({ generation: 'restored-generation', texts: ['survives transaction abort'] });
});

test('page death after commit retains recovery and discards stale HTTP work before reload', async ({ page, context }) => {
  await page.evaluate(async () => {
    restore.installRestoreRuntime();
    await restore.acceptRestoreHello(restore.beginRestoreConnection(), null);
    restore.patchDraft('composer', { text: 'survives page death before reload' });
  });
  const pendingRead = page.waitForRequest(`${origin}/api/v1/stale-read`);
  const pendingWrite = page.waitForRequest(`${origin}/api/v1/stale-write`);
  let answerRead!: () => Promise<void>;
  let dropWrite!: () => Promise<void>;
  const requests: string[] = [];
  await page.route(`${origin}/api/v1/stale-*`, route => {
    requests.push(route.request().url());
    expect(route.request().headers()['x-lucidos-restore-generation']).toBe('none');
    if (route.request().url().endsWith('stale-read')) {
      answerRead = () => route.fulfill({ contentType: 'application/json', body: '{"archived":true}' });
    } else {
      dropWrite = () => route.abort('failed');
    }
  });
  await page.evaluate(() => {
    Object.assign(window, { staleResults: [] });
    const results = (window as unknown as { staleResults: string[] }).staleResults;
    void restore.json('/api/v1/stale-read').then(() => results.push('read resolved'), () => results.push('read rejected'));
    void restore.mutatingFetchIdempotent('/api/v1/stale-write', { method: 'PATCH' })
      .then(() => results.push('write resolved'), () => results.push('write rejected'));
  });
  await Promise.all([pendingRead, pendingWrite]);
  await page.evaluate(() => { restore.beginRestoreConnection(); });
  // Responses and retryable transport failures from the old invocation must
  // neither update callers nor retry a mutation while restore admission is shut.
  await answerRead();
  await dropWrite();
  await page.evaluate(() => {
    const results = (window as unknown as { staleResults: string[] }).staleResults;
    void restore.mutatingFetch('/api/v1/stale-queued', { method: 'POST' })
      .then(() => results.push('queued resolved'), () => results.push('queued rejected'));
  });
  await page.waitForTimeout(150);
  expect(await page.evaluate(() => ({
    results: (window as unknown as { staleResults: string[] }).staleResults,
    ready: restore.restoreAdmissionReady(),
  }))).toEqual({ results: [], ready: false });
  expect(requests.sort()).toEqual([`${origin}/api/v1/stale-read`, `${origin}/api/v1/stale-write`]);
  // Hold the real reload's document request after the runtime's durable commit.
  // Closing the page here kills its runtime and releases its Web Lock without
  // letting a new document boot or recover anything in the original page.
  let reachedReload!: () => void;
  const reloadRequested = new Promise<void>(resolve => { reachedReload = resolve; });
  let releaseReload!: () => void;
  const released = new Promise<void>(resolve => { releaseReload = resolve; });
  await page.route(`${origin}/`, async route => {
    reachedReload();
    await released;
    await route.abort().catch(() => {}); // the page may already be gone
  });
  await page.evaluate(() => {
    void restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  });
  await reloadRequested;
  await page.close();
  releaseReload();

  const replacement = await context.newPage();
  const recoveries: Array<Record<string, unknown>> = [];
  await replacement.route(`${origin}/**`, route => {
    if (route.request().resourceType() === 'document') return route.fulfill({
      contentType: 'text/html', body: '<!doctype html><title>replacement page</title>',
    });
    if (route.request().url() === `${origin}/api/v1/threads`) {
      recoveries.push(route.request().postDataJSON());
      return route.fulfill({ contentType: 'application/json', body: '{}' });
    }
    return route.abort();
  });
  await replacement.goto(origin);
  await replacement.addScriptTag({ content: bundle });
  expect(await replacement.evaluate(async () => {
    const store = restore.createRestoreGenerationStore(null, 'inspector');
    return { generation: await store.readGeneration(), records: (await store.readRecoveries())
      .flatMap(r => r.intent.drafts.map(d => d.draft.text)) };
  })).toEqual({ generation: 'restored-generation', records: ['survives page death before reload'] });
  expect(recoveries).toHaveLength(0);
  expect(await replacement.evaluate(async () => {
    restore.installRestoreRuntime();
    const before = restore.restoreAdmissionReady();
    const admitted = await restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
    return { before, admitted, ready: restore.restoreAdmissionReady() };
  })).toEqual({ before: false, admitted: true, ready: true });
  expect(recoveries).toHaveLength(1);
  expect(recoveries[0]).toMatchObject({ recovered_compose: {
    generation: 'restored-generation', text: 'survives page death before reload',
  } });
  await replacement.reload();
  await replacement.addScriptTag({ content: bundle });
  expect(await replacement.evaluate(async () => {
    restore.installRestoreRuntime();
    return restore.acceptRestoreHello(restore.beginRestoreConnection(), 'restored-generation');
  })).toBe(true);
  expect(recoveries).toHaveLength(1);
  await replacement.close();
});

for (const previous of [null, '00000000-0000-4000-8000-000000000001']) {
  test(`startup hello rejects waiting and retrying HTTP from ${String(previous)} after reopening`, async ({ page }) => {
    const current = '00000000-0000-4000-8000-000000000002';
    const requests: string[] = [];
    let dropWrite!: () => Promise<void>;
    await page.route(`${origin}/api/v1/fenced-*`, route => {
      requests.push(route.request().url());
      expect(route.request().headers()['x-lucidos-restore-generation']).toBe(previous ?? 'none');
      if (requests.length === 1) dropWrite = () => route.abort('failed');
      else return route.fulfill({ contentType: 'application/json', body: '{}' });
    });
    const firstWrite = page.waitForRequest(`${origin}/api/v1/fenced-retry`);
    await page.evaluate(async ({ previous, current }) => {
      // A durable observation may predate this page's first stream hello.
      // Keep invocations alive in this document to prove the generation guard
      // rejects them, rather than letting navigation cancel their promises.
      await restore.createRestoreGenerationStore(null, 'earlier-page').observe(current);
      restore.observeSourceGeneration(previous);
      const results: Array<{ kind: string; code: number }> = [];
      Object.assign(window, { fencedResults: results });
      void restore.mutatingFetchIdempotent('/api/v1/fenced-retry', { method: 'PATCH' })
        .then(() => results.push({ kind: 'retry', code: 200 }),
          error => results.push({ kind: 'retry', code: error.httpCode }));
    }, { previous, current });
    await firstWrite;
    await page.evaluate(() => {
      restore.installRestoreRuntime();
      const results = (window as unknown as { fencedResults: Array<{ kind: string; code: number }> }).fencedResults;
      void restore.mutatingFetch('/api/v1/fenced-compose', { method: 'PUT', body: '{"text":"old intent"}' })
        .then(() => results.push({ kind: 'compose', code: 200 }),
          error => results.push({ kind: 'compose', code: error.httpCode }));
    });
    await dropWrite();
    await page.waitForTimeout(150);
    expect(await page.evaluate(() => ({
      ready: restore.restoreAdmissionReady(),
      results: (window as unknown as { fencedResults: unknown[] }).fencedResults,
    }))).toEqual({ ready: false, results: [] });
    expect(await page.evaluate(async current =>
      restore.acceptRestoreHello(restore.beginRestoreConnection(), current), current)).toBe(true);
    await expect.poll(() => page.evaluate(() =>
      (window as unknown as { fencedResults: Array<{ kind: string; code: number }> }).fencedResults
        .slice().sort((a, b) => a.kind.localeCompare(b.kind))))
      .toEqual([{ kind: 'compose', code: 409 }, { kind: 'retry', code: 409 }]);
    expect(await page.evaluate(() => restore.restoreAdmissionReady())).toBe(true);
    expect(requests).toEqual([`${origin}/api/v1/fenced-retry`]);
  });
}
