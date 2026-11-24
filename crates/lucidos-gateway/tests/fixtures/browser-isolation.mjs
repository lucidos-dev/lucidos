import assert from 'node:assert/strict';
import { chromium, webkit } from 'playwright';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';

const [origin, cookie, credential] = process.argv.slice(2);
const failures = [];
for (const [name, browserType] of [['chromium', chromium], ['mobile-webkit', webkit]]) {
  mkdirSync('.lucidos', {recursive: true});
  const profile = mkdtempSync(join('.lucidos', 'isolation-browser-'));
  const context = await browserType.launchPersistentContext(profile, {
      headless: true,
      ignoreHTTPSErrors: true,
      ...(name === 'mobile-webkit' ? {
        viewport: {width: 390, height: 844}, isMobile: true, hasTouch: true,
        proxy: {server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1'},
      } : {}),
    });
  try {
    await context.addCookies([{name: cookie, value: credential, url: origin}]);
    const page = await context.newPage();
    await page.goto(`${origin}/~/`, {waitUntil: 'domcontentloaded'});
    await page.evaluate(() => localStorage.setItem('gateway-a-secret', 'private'));
    await page.evaluate(async () => {
      await window.lucidosRemoteClientReady;
      const cache = await caches.open('legacy-shared-shell');
      await cache.put('/dev@cloud/', (await fetch('/~/manifest.json')).clone());
      await cache.put('/local/data/keep.txt', (await fetch('/~/manifest.json')).clone());
      const reg = await navigator.serviceWorker.register('/dev@cloud/legacy-worker.js', {scope: '/dev@cloud/'});
      const worker = reg.installing || reg.waiting || reg.active;
      if (worker.state !== 'activated') await new Promise((resolve, reject) => {
        worker.addEventListener('statechange', () => {
          if (worker.state === 'activated') resolve();
          if (worker.state === 'redundant') reject(new Error('legacy worker did not activate'));
        });
      });
    });
    await page.goto(`${origin}/dev@cloud/`, {waitUntil: 'load'});
    await page.waitForFunction(() => navigator.serviceWorker.controller !== null);
    await page.reload({waitUntil: 'load'});
    assert.equal(await page.title(), 'foreign cached shell');
    await page.goto(`${origin}/~/?pick`, {waitUntil: 'load'});
    await page.evaluate(() => window.lucidosRemoteClientReady);
    const retired = await page.evaluate(async () => ({
      registrations: (await navigator.serviceWorker.getRegistrations()).map(r => r.scope),
      remoteCached: !!await caches.match('/dev@cloud/'),
      local: await (await caches.match('/local/data/keep.txt')).text(),
    }));
    assert.deepEqual(retired.registrations, []);
    assert.equal(retired.remoteCached, false);
    assert.ok(retired.local.includes('start_url'));
    await page.goto(`${origin}/dev@cloud/`, {waitUntil: 'load'});
    assert.equal(await page.locator('[data-trusted-client]').count(), 1);
    assert.equal(await page.evaluate(() => navigator.serviceWorker.controller), null);
    console.log(JSON.stringify({browser: name, legacyWorkerRetired: true, localCachePreserved: true}));

    await page.goto(`${origin}/dev@cloud/api/v1/files/document.html`, {waitUntil: 'load'});
    const result = await page.evaluate(async () => {
      if (!window.attack) throw new Error('malicious fixture did not execute');
      return window.attack;
    });
    console.log(JSON.stringify({browser: name, ...result}));
    for (const [key, value] of Object.entries(result)) {
      if (value) failures.push(`${name}: foreign ${key} succeeded`);
    }
    assert.equal(await page.evaluate(async () => {
      const module = await import('/dev@cloud/api/v1/sdk.js');
      return module.publicModule;
    }), 'opaque app module loaded');
    if (result.worker) {
      await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
      await page.goto(`${origin}/dev@cloud/`, {waitUntil: 'load'});
      const poisoned = (await page.title()) === 'foreign cached shell';
      console.log(JSON.stringify({browser: name, workerControlledNavigation: poisoned}));
      if (poisoned) failures.push(`${name}: foreign worker replaced navigation`);
    }
  } finally {
    await context.close();
    rmSync(profile, {recursive: true, force: true});
  }
}
assert.deepEqual(failures, [], failures.join('\n'));
