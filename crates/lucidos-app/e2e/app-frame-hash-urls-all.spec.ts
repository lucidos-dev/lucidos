import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry, ensureMobileView, waitForPaneAtRest } from './helpers';

// Behind a gateway the engine stamps a `<base href>` carrying the frame
// capability (ADR 0238), so the base path differs from the document path. A
// bare `#x` resolves against the base. The frame is sandboxed (ADR 0227), so
// WebKit refuses `history.replaceState` to another path: iOS Safari threw on
// `replaceState(null, '', '#30d')`. Chromium moved the frame onto the base URL.
//
// The gateway binary refuses to boot from a worktree (ADR 0021), and a direct
// engine stamps no pass. So the fixture declares the same base the engine
// stamps, which puts the frame in exactly the state the gateway produces.

const APP_ID = 'e2e-hash-urls-test';
const CAPABILITY_BASE = `/e2e/~cap/6a0b1c2d~${APP_ID}~00112233445566778899aabbccddeeff/app/${APP_ID}/`;
let fixture: { cleanup: () => void };

test.describe('App frame hash URLs under a capability base', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Hash URLs test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Hash URLs test</title>
<base href="${CAPABILITY_BASE}">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="ready">ready</div>
<p style="text-align: center"><a id="in-page" href="#section">Jump</a></p>
<p style="text-align: center"><a id="cancelled" href="#cancelled-target">Cancelled</a></p>
<p id="shadow-host" style="text-align: center"></p>
<div style="height: 200vh"></div>
<div id="section">Section</div>
<div id="cancelled-target">Cancelled target</div>
<div id="shadow-target">Shadow target</div>
<script>
// Added after the SDK loaded, so it runs after the SDK's own window listener.
window.addEventListener('click', function (e) {
  if (e.target.closest && e.target.closest('#cancelled')) e.preventDefault();
});
document.getElementById('shadow-host').attachShadow({ mode: 'open' }).innerHTML =
  '<a id="shadow-link" href="#shadow-target">Shadow</a>';
</script>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test('replaceState, pushState and in-page links keep the document path', async ({ page }) => {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    await ensureMobileView(page, 'content');
    await waitForPaneAtRest(page);

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.app-ui-cover')).toHaveCount(0, { timeout: 10_000 });
    // The visible frame's own document, which is the one the click lands in.
    const frame = await page.locator('iframe[data-role="app-ui-frame"]:visible').elementHandle()
      .then((handle) => handle?.contentFrame());
    expect(frame, 'the app frame is attached').toBeTruthy();

    const before = await frame!.evaluate(() => ({ base: document.baseURI, path: location.pathname }));
    expect(before.base, 'the base differs from the document, as behind a gateway').toContain('/~cap/');
    expect(before.path).toBe(`/app/${APP_ID}/`);

    const replaced = await frame!.evaluate(() => {
      try {
        history.replaceState(null, '', '#probe');
        return { error: null, hash: location.hash, path: location.pathname };
      } catch (err) {
        return { error: String(err), hash: location.hash, path: location.pathname };
      }
    });
    expect(replaced).toEqual({ error: null, hash: '#probe', path: before.path });

    const pushed = await frame!.evaluate(() => {
      try {
        history.pushState(null, '', '#pushed');
        return { error: null, hash: location.hash, path: location.pathname };
      } catch (err) {
        return { error: String(err), hash: location.hash, path: location.pathname };
      }
    });
    expect(pushed).toEqual({ error: null, hash: '#pushed', path: before.path });

    // A marker on the window tells a fragment navigation from a reload, which
    // is what the link did when it resolved against the base. The link is
    // centred, clear of the host's left edge-swipe zone on a phone.
    await frame!.evaluate(() => { (window as unknown as { marker: number }).marker = 1; });
    await appFrame.locator('#in-page').click();
    await expect.poll(() => frame!.evaluate(() => location.hash)).toBe('#section');
    const after = await frame!.evaluate(() => ({
      path: location.pathname,
      marker: (window as unknown as { marker?: number }).marker,
    }));
    expect(after).toEqual({ path: before.path, marker: 1 });

    // An app's own window listener, added after the SDK, still cancels a link.
    await appFrame.locator('#cancelled').click();
    expect(await frame!.evaluate(() => location.hash)).toBe('#section');

    // A link inside a shadow root is followed on this document too.
    await appFrame.locator('#shadow-link').click();
    await expect.poll(() => frame!.evaluate(() => location.hash)).toBe('#shadow-target');
    const shadowed = await frame!.evaluate(() => ({
      path: location.pathname,
      marker: (window as unknown as { marker?: number }).marker,
    }));
    expect(shadowed).toEqual({ path: before.path, marker: 1 });
  });
});
