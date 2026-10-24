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
//
// Every step waits on the step before it, never on a timer. A cold WebKit can
// take longer than any fixed delay to start the fetch, and a frame removed
// before its siblings join strands nobody.

/** After both siblings reach their subresource tags, the time they get to ask
 *  for them. A sibling asks in the same parser pass, so this is slack. */
const JOIN_MS = 200;
/** Between the removal and the answer, so the removal fails the fetch first. */
const RELEASE_AFTER_MS = 300;
const REPORT_TIMEOUT_MS = 6000;

type FrameReport = { id: string; js: boolean; css: string };

let server: http.Server;
let origin: string;
/** Subresource requests the server has seen since the page loaded. */
let requested: string[] = [];
/** Subresource answers held until the page asks for them. */
let held: (() => void)[] = [];
let released = false;

test.beforeAll(async () => {
  server = http.createServer((req, res) => {
    const url = new URL(req.url ?? '/', 'http://canary');
    if (url.pathname === '/frame.html') {
      const id = url.searchParams.get('id') ?? '';
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end(`<!doctype html><head><script>parent.postMessage({ parsing: ${JSON.stringify(id)} }, '*');</script>
<link rel="stylesheet" href="/slow.css"><script src="/slow.js"></script></head>
<body><script>
  const css = getComputedStyle(document.body).getPropertyValue('--probe').trim();
  parent.postMessage({ id: ${JSON.stringify(id)}, js: typeof sdkLoaded !== 'undefined', css }, '*');
</script></body>`);
      return;
    }
    if (url.pathname === '/slow.js' || url.pathname === '/slow.css') {
      const js = url.pathname === '/slow.js';
      requested.push(url.pathname);
      const answer = () => {
        res.writeHead(200, { 'content-type': js ? 'application/javascript' : 'text/css', 'cache-control': 'no-cache' });
        res.end(js ? 'var sdkLoaded = true;' : 'body { --probe: yes; }');
      };
      if (released) answer();
      else held.push(answer);
      return;
    }
    if (url.pathname === '/requested') {
      res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
      res.end(JSON.stringify(requested.length));
      return;
    }
    if (url.pathname === '/release') {
      released = true;
      for (const answer of held.splice(0)) answer();
      res.writeHead(204, { 'cache-control': 'no-store' });
      res.end();
      return;
    }
    // Only the page resets the state, never a stray request such as a favicon.
    if (url.pathname !== '/') {
      res.writeHead(404);
      res.end();
      return;
    }
    requested = [];
    held = [];
    released = false;
    const reload = url.searchParams.get('reload') === '1';
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(`<!doctype html><body><script>
  window.reports = {};
  const parsing = new Set();
  addEventListener('message', (e) => {
    if ('parsing' in e.data) parsing.add(e.data.parsing);
    else reports[e.data.id] = e.data;
  });
  const frame = (id) => {
    const f = document.createElement('iframe');
    f.sandbox = 'allow-scripts';
    f.src = '/frame.html?id=' + id;
    document.body.append(f);
    return f;
  };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const requested = () => fetch('/requested', { cache: 'no-store' }).then((r) => r.json());
  const until = async (done) => { while (!(await done())) await sleep(25); };
  (async () => {
    // Alone, the first frame starts both fetches, so it owns them.
    const first = frame('first');
    await until(async () => (await requested()) === 2);
    frame('second');
    frame('third');
    await until(async () => parsing.has('second') && parsing.has('third'));
    await sleep(${JOIN_MS});
    window.requestedAtRemoval = await requested();
    first.remove();
    // A replaced frame can still deliver the report it queued as its fetch
    // failed, so a fresh element reports under its own id.
    if (${reload}) for (const f of [...document.querySelectorAll('iframe')]) {
      f.remove();
      frame(new URL(f.src).searchParams.get('id') + '-again');
    }
    await sleep(${RELEASE_AFTER_MS});
    await fetch('/release', { cache: 'no-store' });
  })();
</script></body>`);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

test.afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
});

async function siblingReports(page: Page, reload: boolean): Promise<{ reports: FrameReport[]; requestedAtRemoval: number }> {
  await page.goto(`${origin}/?reload=${reload ? 1 : 0}`);
  const ids = ['second', 'third'].map((id) => (reload ? `${id}-again` : id));
  await page.waitForFunction(
    (wanted) => wanted.every((id) => id in (window as unknown as { reports: object }).reports),
    ids,
    { timeout: REPORT_TIMEOUT_MS },
  );
  const seen = await page.evaluate(() => {
    const w = window as unknown as { reports: Record<string, FrameReport>; requestedAtRemoval: number };
    return { reports: w.reports, requestedAtRemoval: w.requestedAtRemoval };
  });
  return { reports: ids.map((id) => seen.reports[id]), requestedAtRemoval: seen.requestedAtRemoval };
}

test.describe('WebKit shared subresource fetch', () => {
  test.beforeEach(({ browserName }) => {
    test.skip(browserName !== 'webkit', 'Only WebKit shares the fetch between frames');
  });

  test('removing the frame that started it strands its siblings', async ({ page }) => {
    const { reports, requestedAtRemoval } = await siblingReports(page, false);
    expect(requestedAtRemoval, 'WebKit no longer shares the fetch: retire the measure').toBe(2);
    for (const report of reports) {
      expect(report, 'WebKit no longer fails the shared fetch: retire the measure').toEqual({ id: report.id, js: false, css: '' });
    }
  });

  test('a fresh element for each sibling gets the files back', async ({ page }) => {
    const { reports } = await siblingReports(page, true);
    for (const report of reports) {
      expect(report).toEqual({ id: report.id, js: true, css: 'yes' });
    }
  });
});
