import { test, expect, type Page, type FrameLocator } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, hideDashboardRail, navigateToApp, waitForEventStream } from './helpers';

const REPORT_PATH = 'artifacts/e2e-sandbox/report.html';
const NOTE_PATH = 'artifacts/e2e-sandbox/note.md';
const PIXEL_PATH = 'artifacts/e2e-sandbox/pixel.png';
const FORGER_PATH = 'artifacts/e2e-sandbox/forger.html';
const DIR = resolve(WORKSPACE, 'data/artifacts/e2e-sandbox');
const NOTE_TEXT = 'The sibling note the report links to.';

const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

const SHELL_KEY = 'e2e-sandbox-secret';

const REPORT = `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>sandbox report</title></head>
<body style="margin:0">
<p><a id="to-far" href="#far">Jump to the end</a> · <a id="to-note" href="note.md">Read the note</a></p>
<img id="pixel" src="pixel.png" alt="">
<div id="box" style="width:100px;height:20px;background:#333"></div>
<pre id="probe" data-state="pending">pending</pre>
<div style="height:4000px"></div>
<h2 id="far">The end</h2>
<script>
(function () {
  var out = {};
  try { parent.document.title = 'PWNED'; out.title = 'written'; } catch (e) { out.title = 'blocked'; }
  try { out.parentStorage = String(parent.localStorage.getItem(${JSON.stringify(SHELL_KEY)})); }
  catch (e) { out.parentStorage = 'blocked'; }
  try { localStorage.getItem('x'); out.ownStorage = 'readable'; } catch (e) { out.ownStorage = 'blocked'; }
  var api = document.baseURI.split('/data/')[0] + '/api/v1/threads/list';
  fetch(api).then(function (r) { out.api = r.ok ? 'reached' : 'refused-' + r.status; })
    .catch(function () { out.api = 'blocked'; })
    .then(function () {
      var probe = document.querySelector('#probe');
      probe.textContent = JSON.stringify(out);
      probe.setAttribute('data-state', 'done');
    });
})();
</script>
</body>
</html>
`;

const FORGER = `<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>forger</title></head>
<body><a id="bait" href="note.md">note</a><pre id="forged">pending</pre>
<script>
(function () {
  var stolen = [];
  // A setter on the prototype would catch a nonce assigned onto a message.
  Object.defineProperty(Object.prototype, 'nonce', {
    configurable: true,
    set: function (v) { stolen.push(v); },
    get: function () { return undefined; },
  });
  var markup = document.documentElement.outerHTML;
  var inMarkup = /"nonce":"[0-9a-f]+"/.test(markup);
  // A scripted click and a synthetic chord are not the user's. The bait keeps
  // its own frame in place, so only the bridge could act on the click.
  var bait = document.querySelector('#bait');
  bait.addEventListener('click', function (e) { e.preventDefault(); });
  bait.click();
  document.dispatchEvent(new KeyboardEvent('keydown', { key: '=', ctrlKey: true, bubbles: true }));
  // A message with a guessed nonce.
  parent.postMessage({ type: 'lucidos:preview-frame', nonce: 'guess', kind: 'link', href: 'note.md', baseUri: '' }, '*');
  document.querySelector('#forged').textContent = JSON.stringify({ inMarkup: inMarkup, stolen: stolen.length });
})();
</script>
</body></html>
`;

const FRAME = '.file-preview-inline:visible iframe';

function frame(page: Page): FrameLocator {
  return page.frameLocator(FRAME).first();
}

async function openFile(page: Page, path: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: path } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
}

async function openReport(page: Page): Promise<void> {
  await openFile(page, REPORT_PATH);
  await expect(frame(page).locator('#probe')).toHaveAttribute('data-state', 'done', { timeout: 20_000 });
}

async function inFrame<T>(page: Page, fn: () => T): Promise<T | null> {
  const handle = await page.locator(FRAME).first().elementHandle();
  const inner = await handle?.contentFrame();
  if (!inner) throw new Error('the preview iframe has no frame');
  try {
    return await inner.evaluate(fn);
  } catch (e) {
    if (e instanceof Error && e.message.includes('Execution context was destroyed')) return null;
    throw e;
  }
}

function boxWidth(): number {
  return document.querySelector('#box')!.getBoundingClientRect().width;
}

test.describe('a previewed HTML artifact', () => {
  test.beforeAll(() => {
    mkdirSync(DIR, { recursive: true });
    writeFileSync(resolve(WORKSPACE, 'data', REPORT_PATH), REPORT);
    writeFileSync(resolve(WORKSPACE, 'data', NOTE_PATH), `# Note\n\n${NOTE_TEXT}\n`);
    writeFileSync(resolve(WORKSPACE, 'data', PIXEL_PATH), PIXEL);
    writeFileSync(resolve(WORKSPACE, 'data', FORGER_PATH), FORGER);
  });

  test.afterAll(() => {
    rmSync(DIR, { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
    await navigateToApp(page);
    await page.evaluate((key) => localStorage.setItem(key, 'the shell only'), SHELL_KEY);
    await waitForEventStream(page);
  });

  test.afterEach(async ({ page }) => {
    await page.evaluate((key) => localStorage.removeItem(key), SHELL_KEY);
  });

  test('runs its scripts, but cannot reach the shell or its session', async ({ page }) => {
    const shellTitle = await page.title();
    await openReport(page);

    const probe = JSON.parse(await frame(page).locator('#probe').textContent() ?? '{}');
    expect(probe.title, 'wrote the shell document').toBe('blocked');
    expect(probe.parentStorage, 'read the shell storage').toBe('blocked');
    expect(probe.ownStorage).toBe('blocked');
    expect(probe.api, 'called the API as the user').not.toBe('reached');
    expect(await page.title()).toBe(shellTitle);

    const sandbox = await page.locator(FRAME).first().getAttribute('sandbox');
    expect(sandbox).toContain('allow-scripts');
    expect(sandbox).not.toContain('allow-same-origin');
  });

  test('loads its relative assets through the stamped base', async ({ page }) => {
    await openReport(page);
    await expect
      .poll(() => inFrame(page, () => (document.querySelector('#pixel') as HTMLImageElement).naturalWidth))
      .toBe(1);
  });

  test('scrolls to an in-page anchor without navigating the frame', async ({ page }) => {
    await openReport(page);
    expect(await inFrame(page, () => window.scrollY)).toBe(0);

    await frame(page).locator('#to-far').click();

    await expect.poll(() => inFrame(page, () => window.scrollY), { timeout: 10_000 }).toBeGreaterThan(1000);
    expect(await inFrame(page, () => document.title)).toBe('sandbox report');
  });

  test('opens a sibling file in the preview, through the host', async ({ page }) => {
    await openReport(page);

    await frame(page).locator('#to-note').click();

    await expect(page.locator('.file-preview-inline:visible .markdown-content'))
      .toContainText(NOTE_TEXT, { timeout: 15_000 });
  });

  test('gives a hostile script no way to navigate without a click', async ({ page }) => {
    await openFile(page, FORGER_PATH);
    await expect(frame(page).locator('#forged')).not.toHaveText('pending', { timeout: 20_000 });

    const seen = JSON.parse(await frame(page).locator('#forged').textContent() ?? '{}');
    expect(seen).toEqual({ inMarkup: false, stolen: 0 });

    await page.waitForTimeout(1_500);
    await expect(page.locator('.file-preview-inline:visible .markdown-content')).toHaveCount(0);
    await expect(frame(page).locator('#forged')).toBeVisible();
  });

  test('forwards the zoom shortcut out of the frame, and re-zooms the document', async ({ page }) => {
    await openReport(page);
    expect(await inFrame(page, boxWidth)).toBeCloseTo(100, 0);

    await frame(page).locator('#probe').click();
    try {
      await page.keyboard.press('Control+=');
      await expect.poll(() => inFrame(page, boxWidth), { timeout: 10_000 }).toBeCloseTo(112.5, 0);
    } finally {
      await expect(page.locator('.scale-modal')).toHaveCount(0, { timeout: 5_000 });
      await frame(page).locator('#probe').click();
      await page.keyboard.press('Control+0');
    }
    await expect.poll(() => inFrame(page, boxWidth), { timeout: 10_000 }).toBeCloseTo(100, 0);
  });
});
