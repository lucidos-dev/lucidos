// @vitest-environment node
/** Two real pages, real IndexedDB transactions and Web Locks. Only the native
 * hide/show bridge is simulated, since Chromium has no AppKit window. */
import { afterAll, beforeAll, expect, it } from 'vitest';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';

let server: ViteDevServer;
let browser: Browser;
let origin: string;
const fixture = `
import { installWindowPeers, announceWindow } from '/src/store/windowPeers.ts';
import { createInteractiveDeliveryBackend } from '/src/store/interactiveDeliveryRecords.ts';
import { interactiveDeliveryName } from '/src/store/interactiveDeliveryDevice.ts';
import { WORKSPACE_ID } from '/src/utils/basePath.ts';
import { createDeviceInteractiveDelivery } from '/src/store/interactiveDeliveryDevice.ts';
const params = new URLSearchParams(location.search);
const callbacks = new Map();
let nextCallback = 0;
let seedResolve;
window.seedAttempts = 0;
window.setShown = shown => {
  for (const callback of callbacks.values()) callback({ payload: shown });
  seedResolve?.(shown);
};
if (!params.has('browser')) window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: window.name ||= crypto.randomUUID() } },
  transformCallback: callback => { callbacks.set(++nextCallback, callback); return nextCallback; },
  invoke: async (cmd, args) => {
    if (cmd === 'plugin:event|listen') return args.handler;
    if (cmd === 'plugin:event|unlisten') { callbacks.delete(args.eventId); return; }
    if (cmd !== 'get_native_window_shown') throw new Error('Unexpected IPC: ' + cmd);
    window.seedAttempts++;
    if (params.has('legacy-acl')) throw 'Command get_native_window_shown not allowed by ACL';
    if (params.has('legacy-missing')) throw 'Command get_native_window_shown not found';
    if (params.has('seed-failed') && !window.recovered) throw 'IPC connection closed';
    if (params.has('seed-delayed')) return new Promise(resolve => { seedResolve = resolve; });
    return true;
  },
};
window.listenerCount = () => callbacks.size;
window.accepted = []; window.uncertain = []; window.errors = [];
window.accept = true;
window.thread = params.get('thread');
if (params.has('no-locks')) Object.defineProperty(navigator, 'locks', { value: undefined });
installWindowPeers(() => ({ focusedThreadId: window.thread, appId: null, authorizing: null }));
window.delivery = createDeviceInteractiveDelivery({
  deviceId: params.get('device') || 'test-device',
  present: r => { if (!window.accept) return false; window.accepted.push(r.id); return true; },
  uncertain: r => window.uncertain.push(r.id), failed: e => window.errors.push(String(e)),
});
window.publish = () => { announceWindow(); return window.delivery.publish(); };
window.seedPending = (id, claimed = false) => createInteractiveDeliveryBackend(interactiveDeliveryName(WORKSPACE_ID, params.get('device') || 'test-device')).update(j => {
 j.requests[id] = { request: { id, kind: 'navigation', payload: {}, preference: { threadId: 'source' } }, delivery: claimed ? { status: 'claimed', ownerId: 'dead-owner' } : { status: 'pending', declined: {} } };
});
window.submit = id => window.delivery.submit({ id, kind: 'navigation', payload: {}, preference: { threadId: 'source' } });
await window.delivery.publish().catch(e => window.errors.push(String(e)));
window.ready = true;
`;

beforeAll(async () => {
  server = await createServer({
    configFile: false, root: process.cwd(), logLevel: 'error', optimizeDeps: { noDiscovery: true, entries: [] },
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'delivery-browser-native-boundary', enforce: 'pre',
      configureServer(vite) {
        vite.middlewares.use((req, res, next) => {
          if (req.url?.startsWith('/delivery-fixture')) {
            const workspace = new URL(req.url, 'http://localhost').searchParams.get('workspace') || 'workspace-a';
            res.setHeader('Content-Type', 'text/html');
            res.end('<base href="/' + workspace + '/"><script type="module">' + fixture + '</script>');
          } else next();
        });
      },
    }],
  });
  await server.listen();
  const address = server.httpServer!.address();
  if (!address || typeof address === 'string') throw new Error('Missing browser fixture address');
  origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch();
}, 30_000);
afterAll(async () => { await browser?.close(); await server?.close(); });

// Browser globals are intentionally accessed through evaluate so no production
// test hook or debug global becomes part of the app.
async function run(page: Page, code: string) { return page.evaluate(code); }
async function open(page: Page, query = '') {
  await page.goto(`${origin}/delivery-fixture?${query}`);
  await page.waitForFunction('window.ready === true', null, { timeout: 5000 });
}

it('delivers once through native hide/show gaps, duplicate frames, dead owners and reloads', async () => {
  const context = await browser.newContext();
  try {
    const owner = await context.newPage();
    const other = await context.newPage();
    await open(owner, 'thread=source');
    await open(other, 'thread=elsewhere');
    await run(owner, 'window.setShown(false)');
    await Promise.all([run(owner, "window.submit('hidden')"), run(other, "window.submit('hidden')")]);
    await other.waitForFunction("window.accepted.includes('hidden')", null, { timeout: 5000 });
    expect(await run(owner, 'window.accepted')).toEqual([]);
    await run(owner, 'window.setShown(true)');
    await Promise.all([run(owner, "window.submit('shown')"), run(other, "window.submit('shown')")]);
    await owner.waitForFunction("window.accepted.includes('shown')", null, { timeout: 5000 });
    expect(await run(other, 'window.accepted')).toEqual(['hidden']);
    // Retained receipts survive a page reload and duplicate SSE input.
    await owner.reload();
    await owner.waitForFunction('window.ready');
    await run(owner, "window.submit('shown')");
    expect(await run(owner, 'window.accepted')).toEqual([]);
    // A declined pending request is adopted when its preferred owner dies.
    await run(owner, "window.seedPending('orphan')");
    await owner.close();
    await other.waitForFunction("window.accepted.includes('orphan')", null, { timeout: 5000 });
    expect(await run(other, 'window.errors')).toEqual([]);
  } finally { await context.close(); }
}, 30_000);

it('isolates workspace/device journals and retains resolution-before-submit', async () => {
  const context = await browser.newContext();
  try {
    const a = await context.newPage();
    const device = await context.newPage();
    const workspace = await context.newPage();
    await open(a, 'thread=source');
    await open(device, 'device=another-device');
    await open(workspace, 'workspace=workspace-b');
    await run(a, "window.delivery.resolve('form:resolved'); window.submit('own')");
    await a.waitForFunction("window.accepted.includes('own')");
    await run(a, "window.delivery.submit({ id: 'form:resolved', kind: 'form', payload: {}, preference: { threadId: 'source' } })");
    expect(await run(a, 'window.accepted')).toEqual(['own']);
    expect(await run(device, 'window.accepted')).toEqual([]);
    expect(await run(workspace, 'window.accepted')).toEqual([]);
  } finally { await context.close(); }
}, 30_000);

it('recovers persisted ambiguous claims without automatic replay, and refuses unavailable locks', async () => {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await open(page, 'thread=source');
    await run(page, "window.seedPending('ambiguous', true)");
    await page.reload();
    await page.waitForFunction('window.ready');
    expect(await run(page, 'window.accepted')).toEqual([]);
    expect(await run(page, 'window.uncertain')).toEqual(['ambiguous']);
    await run(page, "window.delivery.retry('ambiguous')");
    await run(page, "window.delivery.retry('ambiguous')");
    expect(await run(page, 'window.accepted')).toEqual(['ambiguous']);
    const unavailable = await context.newPage();
    await open(unavailable, 'no-locks&device=unavailable');
    expect(await run(unavailable, 'window.errors.join()')).toContain('Web Locks');
    expect(await run(unavailable, 'window.accepted')).toEqual([]);
  } finally { await context.close(); }
}, 30_000);

it('waits for native visibility and reports failed readiness outside IndexedDB', async () => {
  const context = await browser.newContext();
  try {
    const delayed = await context.newPage();
    await delayed.goto(`${origin}/delivery-fixture?native&seed-delayed`);
    await delayed.waitForFunction('typeof window.setShown === "function"');
    await run(delayed, "window.pending = window.submit('native-request'); void 0");
    expect(await run(delayed, 'window.accepted')).toEqual([]);
    expect(await run(delayed, 'window.errors')).toEqual([]);
    await run(delayed, 'window.setShown(true)');
    await run(delayed, 'window.pending');
    expect(await run(delayed, 'window.accepted')).toEqual(['native-request']);
    const failed = await context.newPage();
    await open(failed, 'native&seed-failed&device=failed-seed');
    expect(await run(failed, 'window.errors.join()')).toContain('Could not initialize native window visibility');
    expect(await run(failed, 'window.errors.join()')).not.toContain('AbortError');
    expect(await run(failed, 'window.listenerCount()')).toBe(0);
    await run(failed, 'window.recovered = true; window.publish()');
    await run(failed, "window.submit('recovered')");
    expect(await run(failed, 'window.accepted')).toEqual(['recovered']);
    expect(await run(failed, 'window.listenerCount()')).toBe(1);
    expect(await run(failed, 'window.seedAttempts')).toBeGreaterThan(1);
  } finally { await context.close(); }
}, 30_000);

it('serializes form server reads across pages until reconciliation commits', async () => {
  const context = await browser.newContext();
  const first = await context.newPage();
  const second = await context.newPage();
  await open(first);
  await open(second);
  await run(first, "window.sync = window.delivery.synchronizeForms(async () => { window.entered = true; await new Promise(resolve => window.release = resolve); }); undefined;");
  await first.waitForFunction('window.entered === true');
  await run(second, "window.sync = window.delivery.synchronizeForms(async () => { window.entered = true; }); undefined;");
  expect(await run(second, 'window.entered')).toBeUndefined();
  await run(first, 'window.release(); window.sync');
  await run(second, 'window.sync');
  expect(await run(second, 'window.entered')).toBe(true);
  await context.close();
});

it.each(['legacy-acl', 'legacy-missing'])('coordinates legacy shells with current shells: %s', async (legacy) => {
  const context = await browser.newContext();
  try {
    const old = await context.newPage();
    const current = await context.newPage();
    await open(old, legacy + '&thread=source');
    await open(current, 'thread=elsewhere');
    expect(await run(old, 'window.errors')).toEqual([]);
    expect(await run(old, 'window.listenerCount()')).toBe(0);
    await Promise.all([run(old, "window.submit('legacy-request')"), run(current, "window.submit('legacy-request')")]);
    await old.waitForFunction("window.accepted.includes('legacy-request')");
    expect(await run(current, 'window.accepted')).toEqual([]);
    await run(old, "window.delivery.resolve('form:resolved')");
    await run(current, "window.delivery.submit({ id: 'form:resolved', kind: 'form', payload: {}, preference: {} })");
    expect(await run(current, 'window.accepted')).toEqual([]);
    await old.reload();
    await old.waitForFunction('window.ready');
    await run(old, "window.submit('legacy-request')");
    expect(await run(old, 'window.accepted')).toEqual([]);
    expect(await run(old, 'window.errors')).toEqual([]);
  } finally { await context.close(); }
}, 30_000);

it('keeps browser delivery independent of native IPC', async () => {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await open(page, 'browser');
    await run(page, "window.submit('browser-request')");
    expect(await run(page, 'window.accepted')).toEqual(['browser-request']);
    expect(await run(page, 'window.seedAttempts')).toBe(0);
    expect(await run(page, 'window.errors')).toEqual([]);
  } finally { await context.close(); }
});
