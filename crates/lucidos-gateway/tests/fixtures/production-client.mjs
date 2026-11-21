import assert from 'node:assert/strict';
import { chromium, webkit } from 'playwright';
import { mkdirSync, mkdtempSync, rmSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

const [origin, cookie, credential, route, installedDir] = process.argv.slice(2);
const workerFile = join(installedDir, 'sw.js');
const installedWorker = readFileSync(workerFile, 'utf8');
for (const [name, browserType] of [['chromium', chromium], ['mobile-webkit', webkit]]) {
  mkdirSync('.lucidos', {recursive: true});
  const profile = mkdtempSync(join('.lucidos', 'production-browser-'));
  const context = await browserType.launchPersistentContext(profile, {
    headless: true, ignoreHTTPSErrors: true,
    ...(name === 'mobile-webkit' ? {
      viewport: {width: 390, height: 844}, isMobile: true, hasTouch: true,
      proxy: {server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1'},
    } : {}),
  });
  try {
    writeFileSync(workerFile, installedWorker);
    await context.addCookies([{name: cookie, value: credential, url: origin}]);
    const page = await context.newPage();
    const requests = [];
    page.on('request', request => requests.push(request.url()));
    page.on('pageerror', error => console.error(name, error.message));
    page.on('response', response => {
      if (response.status() >= 400 || response.url().includes('/events/')) console.error(name, response.status(), response.url());
    });
    page.on('requestfailed', request => console.error(name, request.failure()?.errorText, request.url()));
    page.on('console', message => {
      if (['error', 'warning'].includes(message.type())) console.error(name, message.text());
    });
    await page.goto(`${origin}${route}/`, {waitUntil: 'domcontentloaded'});
    await page.locator('[data-role="prompt-input"]:visible').first().waitFor({timeout: 30000});
    assert.equal(await page.locator('meta[name="lucidos-remote-client"]').getAttribute('content'), '1');
    assert.equal(await page.evaluate(() => document.querySelector('base').getAttribute('href')), `${route}/`);
    const marker = `Remote production continuity ${name}`;
    await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected');
    const message = await page.evaluate(async ({route, marker}) => {
      const threadId = crypto.randomUUID();
      const response = await fetch(`${route}/api/v1/chat/stream`, {
        method: 'POST', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({mode: 'human', message: marker, thread_id: threadId, new_thread: true}),
      });
      if (!response.ok) throw new Error(`chat ${response.status}: ${await response.text()}`);
      const ack = await response.json();
      if (!ack.event_id) throw new Error(`chat acknowledgement missing event_id: ${JSON.stringify(ack)}`);
      return {thread_id: threadId};
    }, {route, marker});
    assert.ok(message.thread_id, JSON.stringify(message));
    await page.goto(`${origin}${route}/#thread=${message.thread_id}`, {waitUntil: 'domcontentloaded'});
    await page.getByText(marker, {exact: true}).first().waitFor({timeout: 30000});
    await page.evaluate(marker => localStorage.setItem('continuity-sentinel', marker), marker);
    await page.reload({waitUntil: 'domcontentloaded'});
    await page.getByText(marker, {exact: true}).first().waitFor({timeout: 30000});
    await page.goto(`${origin}/~/remote-client?target=${encodeURIComponent(route + '/?continuity=1')}#thread=${message.thread_id}`, {waitUntil: 'domcontentloaded'});
    await page.getByText(marker, {exact: true}).first().waitFor({timeout: 30000});
    assert.equal(await page.evaluate(() => localStorage.getItem('continuity-sentinel')), marker);
    await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected');
    // Reconnect a real EventSource, then prove transient navigation still arrives.
    await page.evaluate(async () => {
      const response = await fetch('/__fixture/disconnect-streams', {method: 'POST'});
      if (!response.ok) throw new Error(`disconnect fixture ${response.status}`);
    });
    await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream !== 'connected');
    await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected', undefined, {timeout: 30000});
    await page.evaluate(async route => {
      const response = await fetch(`${route}/api/v1/ui/navigate`, {
        method: 'POST', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({target: 'settings', params: {settings_view: 'appearance'}}),
      });
      if (!response.ok) throw new Error(`navigate ${response.status}`);
    }, route);
    await page.locator('.settings-panel:visible [data-search-anchor="appearance:ui-scale"]').waitFor({timeout: 30000});
    const appId = `remote-continuity-${name}`;
    try {
      await page.evaluate(async ({route, appId}) => {
        const files = {
          'manifest.json': JSON.stringify({id: appId, name: 'Remote continuity probe'}),
          'index.html': `<!doctype html><title>Remote app probe</title><script src="/api/v1/sdk.js"></script><script type="module" src="./probe.js"></script>`,
          'probe.js': `const font = new FontFace('Probe', 'url(/${route.split('/').pop()}/api/v1/fonts/fira-code-6.2.woff2)'); await font.load(); window.appProbe = {font: font.status, module: true, sdk: typeof window.lucidos};`,
        };
        for (const [path, body] of Object.entries(files)) {
          const response = await fetch(`${route}/api/v1/data/apps/${appId}/${path}`, {method: 'PUT', body});
          if (!response.ok) throw new Error(`app fixture ${path}: ${response.status} ${await response.text()}`);
        }
      }, {route, appId});
      const app = await context.newPage();
      await app.goto(`${origin}${route}/app/${appId}/`, {waitUntil: 'load'});
      await app.waitForFunction(() => window.appProbe?.font === 'loaded');
      assert.deepEqual(await app.evaluate(() => window.appProbe), {font: 'loaded', module: true, sdk: 'object'});
      await app.close();
    } finally {
      await page.evaluate(async ({route, appId}) => {
        for (const path of ['probe.js', 'index.html', 'manifest.json']) {
          const response = await fetch(`${route}/api/v1/data/apps/${appId}/${path}`, {method: 'DELETE'});
          if (!response.ok) throw new Error(`app cleanup ${path}: ${response.status}`);
        }
      }, {route, appId});
    }
    assert.ok(requests.some(url => url.includes(`${route}/assets/`)), 'production hashed assets loaded');
    assert.ok(requests.some(url => url.includes(`${route}/api/v1/`) && url.includes('events')), 'remote event stream requested');
    assert.ok(!requests.some(url => url === `${origin}${route}/sw.js`), 'remote worker never requested');
    assert.ok(requests.filter(url => new URL(url).pathname.includes('/api/v1/') && !new URL(url).pathname.startsWith('/~/')).every(url => new URL(url).pathname.startsWith(`${route}/api/v1/`)), 'workspace requests stay selected');
    const worker = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.ready;
      return {scope: registration.scope, script: registration.active.scriptURL};
    });
    assert.equal(worker.scope, `${origin}${route}/`);
    assert.equal(worker.script, `${origin}/~/sw.js?workspace=${encodeURIComponent(route.slice(1))}`);
    // Advance A's actual installed worker generation while this client stays
    // loaded. Detection must read A's code, never B's blocked worker endpoint.
    const nextBuild = createHash('sha256').update(installedWorker + name).digest('hex').slice(0, 12);
    const advancedWorker = installedWorker.replace(/const BUILD_ID = '[^']+';/, `const BUILD_ID = '${nextBuild}';`);
    assert.notEqual(advancedWorker, installedWorker);
    writeFileSync(workerFile, advancedWorker);
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await page.getByText('New version available — refresh to sync', {exact: true}).first().waitFor({timeout: 30000});
    assert.ok(requests.includes(`${origin}/~/sw.js`), 'version check reads A installed generation');
    console.log(JSON.stringify({browser: name, productionClient: true, persistedRemoteMessage: true}));
  } finally {
    writeFileSync(workerFile, installedWorker);
    await context.close();
    rmSync(profile, {recursive: true, force: true});
  }
}
