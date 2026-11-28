import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { apiRequest, navigateToApp, waitForEventStream } from './helpers';


const APP_A = 'e2e-storage-a';
const APP_B = 'e2e-storage-b';

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

const frameOf = (page: Page, id: string) =>
  page.frameLocator(`iframe[data-role="app-ui-frame"][src*="/app/${id}/"]:visible`);

async function openApp(page: Page, id: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'app', params: { app_id: id } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate app ${id} -> ${res.status()}`).toBeTruthy();
}

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
