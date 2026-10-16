import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { apiRequest, navigateToApp, waitForEventStream } from './helpers';

// App storage in a real isolated app frame. The frame's own localStorage
// throws, so `lucidos.storage.local` is served from the host. Two promises are
// checked end to end: a value survives a reload, and one app never sees
// another's keys. The apps open through the shell's own AppUiInline, so the
// frame carries production's sandbox and the bridge is the real one.

const APP_A = 'e2e-storage-a';
const APP_B = 'e2e-storage-b';

/** Each app reports what it found once storage is ready, then claims the key
 *  if nobody had. */
const appHtml = (id: string) => `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><script src="/api/v1/sdk.js"></script></head>
<body>
<div id="seen"></div>
<div id="count"></div>
<script>
  lucidos.storage.ready.then(function () {
    var seen = lucidos.storage.local.getItem('who');
    document.getElementById('count').textContent = String(lucidos.storage.local.length);
    if (seen === null) lucidos.storage.local.setItem('who', '${id}');
    document.getElementById('seen').textContent = seen === null ? 'nothing' : seen;
  });
</script>
</body>
</html>
`;

let fixtures: Array<{ cleanup: () => void }> = [];

/** The frame running `id`, picked by its `src`. So a check never reads one
 *  app's frame while believing it is the other's. */
const frameOf = (page: Page, id: string) =>
  page.frameLocator(`iframe[data-role="app-ui-frame"][src*="/app/${id}/"]:visible`);

async function openApp(page: Page, id: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'app', params: { app_id: id } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate app ${id} -> ${res.status()}`).toBeTruthy();
}

/** Wait until the host has stored `id`'s claim, so a reload cannot race it.
 *  The suffix pins the stored key layout the host writes. */
async function claimStored(page: Page, id: string): Promise<void> {
  await expect.poll(() => page.evaluate((app) => {
    for (let i = 0; i < localStorage.length; i++) {
      if (localStorage.key(i)?.endsWith(`appbridge:${app}:app:who`)) return true;
    }
    return false;
  }, id)).toBe(true);
}

test.describe('app storage in an isolated app frame', () => {
  test.beforeAll(() => {
    fixtures = [APP_A, APP_B].map((id) => createIframeAppFixture(id, {
      manifest: { id, name: `Storage ${id}`, description: 'e2e fixture' },
      html: appHtml(id),
      js: '',
    }));
  });

  test.afterAll(() => {
    for (const f of fixtures) f.cleanup();
  });

  test('a value survives a reload, and no app sees another\'s keys', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);

    await openApp(page, APP_A);
    await expect(frameOf(page, APP_A).locator('#seen')).toHaveText('nothing', { timeout: 15_000 });
    await claimStored(page, APP_A);

    // The shell restores the open app on reload, and the frame primes again.
    await page.reload();
    await expect(frameOf(page, APP_A).locator('#seen')).toHaveText(APP_A, { timeout: 15_000 });

    await waitForEventStream(page);
    await openApp(page, APP_B);
    await expect(frameOf(page, APP_B).locator('#seen')).toHaveText('nothing', { timeout: 15_000 });
    await expect(frameOf(page, APP_B).locator('#count')).toHaveText('0');
    await claimStored(page, APP_B);

    await openApp(page, APP_A);
    await expect(frameOf(page, APP_A).locator('#seen')).toHaveText(APP_A, { timeout: 15_000 });
    await expect(frameOf(page, APP_A).locator('#count')).toHaveText('1');
  });
});
