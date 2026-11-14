import { test, expect } from './fixtures';
import type { Frame, Page } from '@playwright/test';
import { createIframeAppFixture } from './db-helpers';
import { clickHeaderAction, gotoWithRetry } from './helpers';

// Find in app, end to end. The host's find bar cannot read the isolated app
// frame, so the SDK inside it matches, highlights and scrolls. The unit floor
// is `store/actions/find-bar.test.ts` and `packages/lucidos-sdk/src/find.test.ts`.
// This is what only a browser can say: Mod+F reaches the host from inside the
// frame, the Highlight API really paints, and the frame really scrolls.
//
// `-desktop`, because the mobile projects park the frame off-screen in the
// swipe container and press no Mod+F.
// Plan: `docs/plans/2026-10-07-find-in-app.md`.

const APP_ID = 'e2e-find-app';
const BARE_APP_ID = 'e2e-find-bare-app';

const filler = Array.from({ length: 60 }, (_, i) => `<p>Filler paragraph ${i}.</p>`).join('\n');

let fixtures: Array<{ cleanup: () => void }> = [];

/** Open `id` through the real `AppUiInline`, as a restored app window. */
async function openApp(page: Page, id: string): Promise<Frame> {
  await page.addInitScript((appId) => {
    // An init script runs in every frame, and the isolated app frame's storage
    // throws. Only the top frame is the shell.
    if (window.parent !== window) return;
    localStorage.setItem('app-window-open', appId);
  }, id);
  await gotoWithRetry(page, '/');
  await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toHaveCount(1, { timeout: 15_000 });
  await expect.poll(() => page.frames().some((f) => f.url().includes(`/app/${id}/`)), { timeout: 15_000 }).toBe(true);
  const frame = page.frames().find((f) => f.url().includes(`/app/${id}/`))!;
  await frame.waitForSelector('#ready');
  return frame;
}

const status = (page: Page) => page.locator('[data-role="find-status"]:visible');
const findInput = (page: Page) => page.locator('[data-role="find-input"]:visible');

test.describe('find in app', () => {
  test.beforeAll(() => {
    fixtures = [
      createIframeAppFixture(APP_ID, {
        manifest: { id: APP_ID, name: 'Find app', description: 'e2e fixture' },
        html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8"><title>Find app</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="ready">The first needle sits at the top.</div>
${filler}
<p>A <b>needle</b> in the middle.</p>
${filler}
<pre id="last">${'A long log line.\n'.repeat(120)}The last needle, at the foot of a block taller than the screen.</pre>
<p style="display: none">A hidden needle that nobody sees.</p>
</body>
</html>
`,
        js: '',
      }),
      createIframeAppFixture(BARE_APP_ID, {
        manifest: { id: BARE_APP_ID, name: 'Bare app', description: 'e2e fixture' },
        html: '<!DOCTYPE html><html><body><div id="ready">a needle, and no SDK</div></body></html>',
        js: '',
      }),
    ];
  });

  test.afterAll(() => {
    for (const f of fixtures) f.cleanup();
  });

  test('Mod+F inside an app finds, highlights and steps through its text', async ({ page }) => {
    const frame = await openApp(page, APP_ID);

    // Focus inside the app, as a reader who has been clicking around in it.
    await frame.locator('#ready').click();
    await page.keyboard.press('ControlOrMeta+f');
    await expect(findInput(page)).toBeFocused();

    await page.keyboard.type('needle');
    await expect(status(page)).toHaveText('1 of 3');
    await expect.poll(() => frame.evaluate(() => (CSS.highlights.get('lucidos-find') as Highlight | undefined)?.size ?? 0))
      .toBe(3);

    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('2 of 3');
    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('3 of 3');
    // The last match is at the foot of a block taller than the screen. The
    // match itself must scroll into view, not its block.
    await expect.poll(() => frame.evaluate(() => {
      const current = CSS.highlights.get('lucidos-find-current') as Highlight | undefined;
      const range = current ? [...current][0] as Range : undefined;
      const r = range?.getBoundingClientRect();
      return !!r && r.top >= 0 && r.bottom <= window.innerHeight;
    })).toBe(true);

    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('1 of 3');
    await page.keyboard.press('Shift+Enter');
    await expect(status(page)).toHaveText('3 of 3');

    await page.keyboard.press('Escape');
    await expect(page.locator('[data-role="find-bar"]')).toHaveCount(0);
    await expect.poll(() => frame.evaluate(() => CSS.highlights.has('lucidos-find'))).toBe(false);
  });

  test('an app without the SDK says it cannot be searched', async ({ page }) => {
    await openApp(page, BARE_APP_ID);
    // The header button, or its row in the ⋯ menu on a narrow pane.
    await clickHeaderAction(page, '.find-btn');
    await findInput(page).fill('needle');
    await expect(status(page)).toHaveText('This app can’t be searched', { timeout: 10_000 });
  });
});
