/** Real Settings restore against the private gateway fixture. Only client
 * assets are fulfilled locally; every API request and SSE reaches its engine
 * through the gateway, as a paired device.
 * Run with E2E_SNAPSHOT_BROWSER=chromium scripts/e2e-api.sh --local -f
 * server::snapshot_roundtrip_tests::isolated_snapshot_cli_roundtrip after a
 * frontend build. The shared browser-suite workspace is never restored. */
import { chromium, expect } from '@playwright/test';
import { readFile, stat } from 'node:fs/promises';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const config = JSON.parse(process.env.E2E_SNAPSHOT_BROWSER_CONFIG ?? 'null');
if (!config?.workspace?.startsWith('snapshot-e2e-') || !config.credential || !config.handle) {
  throw new Error('Private snapshot browser fixture required');
}
const dist = resolve(dirname(fileURLToPath(import.meta.url)), '../dist');
// Engines trust a device only under the gateway's stamp, so the page is a
// paired device on the gateway origin. The shell carries what the engine
// stamps behind the gateway, and the SSE worker loads through the same prefix.
const prefix = `/${config.workspace}/`;
const origin = `http://127.0.0.1:${config.port}`;
const shell = (await readFile(join(dist, 'index.html'), 'utf8')).replace('<head>',
  `<head><base href="${prefix}"><meta name="lucidos-workspace-id" content="${config.workspace}"><meta name="lucidos-gateway-port" content="${config.port}">`);
const browser = await chromium.launch({
  headless: true, executablePath: process.env.E2E_SNAPSHOT_BROWSER_EXECUTABLE,
});
const context = await browser.newContext({ serviceWorkers: 'block', reducedMotion: 'reduce' });
await context.addCookies([{ name: config.cookie, value: config.credential, url: origin }]);
const page = await context.newPage();
const errors = [];
page.on('pageerror', error => errors.push(error.message));
let documents = 0;
page.on('request', request => {
  if (request.isNavigationRequest() && request.frame() === page.mainFrame()) documents++;
});
await page.route(`${origin}/**`, async route => {
  const request = route.request();
  const path = new URL(request.url()).pathname;
  if (path === prefix && request.resourceType() === 'document') {
    return route.fulfill({ contentType: 'text/html', body: shell });
  }
  if (path.startsWith(`${prefix}api/`) || path.startsWith('/~/api/')) return route.continue();
  const file = resolve(dist, decodeURIComponent(path.slice(prefix.length)));
  if (path.startsWith(prefix) && file.startsWith(`${dist}${sep}`)) {
    try {
      if ((await stat(file)).isFile()) return route.fulfill({ path: file });
    } catch { /* Unknown assets still receive the real server's response. */ }
  }
  return route.continue();
});
const api = `${origin}${prefix}api/v1`;
try {
  await page.goto(`${origin}${prefix}`);
  await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected',
    undefined, { timeout: 60_000 });
  const headers = { 'x-lucidos-restore-generation': config.generation };
  const navigation = await context.request.post(`${api}/ui/navigate`, {
    headers, data: { target: 'settings', params: { settings_view: 'backup' } },
  });
  expect(navigation.ok(), 'Settings navigation').toBeTruthy();
  const row = page.locator('.settings-section .list-row').filter({ hasText: config.label });
  await expect(row).toBeVisible({ timeout: 30_000 });
  const manifest = await context.request.get(`${api}/snapshots/${config.handle}`);
  expect(manifest.ok()).toBeTruthy();
  const snapshot = await manifest.json();
  expect(snapshot.workspace_id).toBe(config.workspace);
  const write = await context.request.put(`${api}/data/artifacts/browser-after-snapshot.txt`, {
    headers: { ...headers, 'content-type': 'text/plain' }, data: 'after browser snapshot\n',
  });
  expect(write.ok(), 'disposable later file').toBeTruthy();
  await row.getByRole('button', { name: 'Restore', exact: true }).click();
  const dialog = page.getByRole('dialog').filter({ hasText: 'Restore workspace' });
  await expect(dialog).toContainText(`Restore workspace "${snapshot.workspace_name}"`);
  await expect(dialog).toContainText('external side effects');
  const before = documents;
  const reloaded = page.waitForEvent('domcontentloaded', { timeout: 120_000 });
  const admitted = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `${prefix}api/v1/snapshots/${config.handle}/restore`);
  await dialog.locator('[data-role="confirm-ok"]').click();
  expect((await admitted).status(), 'one real restore admission').toBe(202);
  await expect.poll(() => documents, { timeout: 120_000 }).toBe(before + 1);
  await reloaded;
  await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected',
    undefined, { timeout: 60_000 });
  const missing = await context.request.get(`${api}/data/artifacts/browser-after-snapshot.txt`);
  expect(missing.status(), 'Settings restore replaced real files').toBe(404);
  await page.locator('.notifications-bell:visible').first().click();
  const notification = page.locator('.notification-item')
    .filter({ hasText: 'Workspace restored' }).filter({ hasText: config.handle });
  await expect(notification.first()).toBeVisible({ timeout: 30_000 });
  expect(documents, 'one generation reload').toBe(before + 1);
  console.log('Settings confirmation, real restore, one reload and restored notification passed');
} catch (error) {
  await page.screenshot({ path: join(process.env.HOME, '.lucidos/browser-failure.png'), fullPage: true });
  console.error('Browser page errors:', errors);
  throw error;
} finally {
  await context.close();
  await browser.close();
}
