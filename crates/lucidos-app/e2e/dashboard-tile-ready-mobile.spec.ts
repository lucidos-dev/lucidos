import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, ensureOnThreadPane, gotoWithRetry, waitForVisibleInput, waitForWorkspaceReady } from './helpers';
import { ensureHomeThread } from './db-helpers';

/**
 * An on-ready tile on the phone dashboard lifts its cover well before the 15 s
 * ready fuse, and logs no `slow load` line. The widget reads a data file over
 * the bridge, as Thread cost does. A stall in the bridge or the host shows as a
 * late cover or a logged line.
 */

/** The cover must lift well under the 15 s ready fuse. */
const READY_BUDGET_MS = 5_000;

/** Enough rows that the read is a real transfer, not a one-packet reply. */
const DATA_ROWS = 4_000;

let written: string[] = [];
let added: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

const readyWidget = (dataPath: string) => '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body>'
  + '<p id="rows"></p><script>'
  + `lucidos.data.read(${JSON.stringify(dataPath)}).then((text) => {`
  + 'document.getElementById("rows").textContent = String(JSON.parse(text).length);'
  + 'lucidos.ui.ready();'
  + '});</script></body></html>';

test.describe('A dashboard tile that reveals on ready, on a phone', () => {
  let appId: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    ensureHomeThread();
    written = [];
    added = [];
    appId = `e2e-tile-ready-${randomUUID().slice(0, 8)}`;
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    for (const id of added) {
      await apiRequest(page).post('/api/v1/widgets/dashboard-remove', { data: { app_id: id, dashboard: 'phone' } });
    }
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  test('lifts its cover within the budget and logs no slow load', async ({ page }) => {
    const dataPath = `artifacts/${appId}/rows.json`;
    const rows = Array.from({ length: DATA_ROWS }, (_, i) => ({ i, title: `row ${i}`, note: 'x'.repeat(40) }));
    await writeFile(page, dataPath, JSON.stringify(rows));
    await writeFile(page, `apps/${appId}/index.html`, readyWidget(dataPath));
    await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
      name: appId, description: 'E2E on-ready tile', kind: 'widget', reusable: true, reveal: 'on-ready', sizes: ['medium', 'small'],
    }));
    const resp = await apiRequest(page).post('/api/v1/widgets/dashboard-add', { data: { app_id: appId, dashboard: 'phone' } });
    expect(resp.ok(), `add ${appId}: ${await resp.text()}`).toBeTruthy();
    added.push(appId);

    const slowLines: unknown[] = [];
    page.on('request', (req) => {
      if (!req.url().endsWith('/api/v1/internal/client-log')) return;
      const body = req.postDataJSON() as { category?: string; message?: string; data?: { app?: string } } | null;
      if (body?.category === 'app-frame' && body.message?.startsWith('slow load') && body.data?.app === appId) slowLines.push(body);
    });

    await page.addInitScript(() => localStorage.removeItem('lucidos-focused-thread'));
    await gotoWithRetry(page, '/');
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
    await waitForWorkspaceReady(page);
    await page.locator('.mobile-thread-header .header-mark-group .dashboard-panel-toggle:visible').click();

    const tile = page.locator(`.dashboard-panel .dashboard-tile[data-dashboard-tile^="${appId}?"]`);
    await expect(tile).toBeVisible();
    await expect(tile.frameLocator('iframe').locator('#rows')).toHaveText(String(DATA_ROWS), { timeout: READY_BUDGET_MS });
    await expect(tile.locator('.app-ui-cover')).toHaveCount(0, { timeout: READY_BUDGET_MS });
    expect(slowLines, 'a slow-load line was posted').toEqual([]);
  });
});
