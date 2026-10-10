import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { test, expect, type Page } from '@playwright/test';

// Canary for `docs/temporary-measures.md` § Shared-fetch reload of app frames.
//
// WebKit shares one in-flight fetch of a subresource between sandboxed frames.
// Removing the frame that started it fails the fetch for every sibling still
// waiting. `appFrameSharedLoads.ts` reloads those siblings. When this spec
// fails because the siblings keep their files, WebKit has fixed it, and the
// measure can go. The second test pins that a reload recovers them.
//
// Its own server, not `page.route`: routing disables the HTTP cache, and with
// it the shared fetch this spec needs. Plain Playwright, not `./fixtures`:
// the page never touches the app, and the fixtures' WebKit preflight needs it.

const SLOW_MS = 1200;
const REMOVE_AFTER_MS = 300;

type FrameReport = { id: string; js: boolean; css: string };

let server: http.Server;
let origin: string;

test.beforeAll(async () => {
  server = http.createServer((req, res) => {
    const url = new URL(req.url ?? '/', 'http://canary');
    if (url.pathname === '/frame.html') {
      const id = url.searchParams.get('id') ?? '';
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end(`<!doctype html><head><link rel="stylesheet" href="/slow.css"><script src="/slow.js"></script></head>
<body><script>
  const css = getComputedStyle(document.body).getPropertyValue('--probe').trim();
  parent.postMessage({ id: ${JSON.stringify(id)}, js: typeof sdkLoaded !== 'undefined', css }, '*');
</script></body>`);
      return;
    }
    if (url.pathname === '/slow.js' || url.pathname === '/slow.css') {
      const js = url.pathname === '/slow.js';
      setTimeout(() => {
        res.writeHead(200, { 'content-type': js ? 'application/javascript' : 'text/css', 'cache-control': 'no-cache' });
        res.end(js ? 'var sdkLoaded = true;' : 'body { --probe: yes; }');
      }, SLOW_MS);
      return;
    }
    const reload = url.searchParams.get('reload') === '1';
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(`<!doctype html><body><script>
  window.reports = {};
  addEventListener('message', (e) => { reports[e.data.id] = e.data; });
  const frame = (id) => {
    const f = document.createElement('iframe');
    f.sandbox = 'allow-scripts';
    f.src = '/frame.html?id=' + id;
    document.body.append(f);
    return f;
  };
  const first = frame('first');
  frame('second');
  frame('third');
  // A replaced frame can still deliver the report it queued as its fetch
  // failed, so a fresh element reports under its own id.
  setTimeout(() => {
    first.remove();
    if (${reload}) for (const f of [...document.querySelectorAll('iframe')]) {
      f.remove();
      frame(new URL(f.src).searchParams.get('id') + '-again');
    }
  }, ${REMOVE_AFTER_MS});
</script></body>`);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

test.afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
});

async function siblingReports(page: Page, reload: boolean): Promise<FrameReport[]> {
  await page.goto(`${origin}/?reload=${reload ? 1 : 0}`);
  const ids = ['second', 'third'].map((id) => (reload ? `${id}-again` : id));
  await page.waitForFunction(
    (wanted) => wanted.every((id) => id in (window as unknown as { reports: object }).reports),
    ids,
    { timeout: SLOW_MS * 5 },
  );
  const reports = await page.evaluate(() => (window as unknown as { reports: Record<string, FrameReport> }).reports);
  return ids.map((id) => reports[id]);
}

test.describe('WebKit shared subresource fetch', () => {
  test.beforeEach(({ browserName }) => {
    test.skip(browserName !== 'webkit', 'Only WebKit shares the fetch between frames');
  });

  test('removing the frame that started it strands its siblings', async ({ page }) => {
    for (const report of await siblingReports(page, false)) {
      expect(report, 'WebKit no longer fails the shared fetch: retire the measure').toEqual({ id: report.id, js: false, css: '' });
    }
  });

  test('a fresh element for each sibling gets the files back', async ({ page }) => {
    for (const report of await siblingReports(page, true)) {
      expect(report).toEqual({ id: report.id, js: true, css: 'yes' });
    }
  });
});
