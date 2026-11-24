import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';


const PROBE_EVENT = 'E2eThreadQueueProbe';
const TRIGGER_NAME = 'E2E thread-queue probe';

async function openThreadQueuePanel(page: Page): Promise<void> {
  await waitForEventStream(page);
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'thread-queue' },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.list-section-title .section-label', { hasText: 'Running' })).toBeVisible({
    timeout: 10_000,
  });
}

async function restoreDefaultPolicy(page: Page): Promise<void> {
  const resp = await apiRequest(page).put('/api/v1/thread-queue/policy', { data: {} });
  expect(resp.ok()).toBeTruthy();
}

async function deleteProbeTrigger(page: Page): Promise<void> {
  const list = await page.request.get('/api/v1/triggers');
  if (!list.ok()) return;
  const body = (await list.json()) as { triggers: Array<{ id: string; name: string }> };
  for (const t of body.triggers ?? []) {
    if (t.name === TRIGGER_NAME) {
      await apiRequest(page).delete(`/api/v1/triggers?id=${encodeURIComponent(t.id)}`);
    }
  }
}

test.describe('thread queue panel', () => {
  test.afterEach(async ({ page }) => {
    await restoreDefaultPolicy(page);
    await deleteProbeTrigger(page);
  });

  test('queued trigger fire shows in the panel and Drop removes it', async ({ page }) => {
    await navigateToApp(page);
    await assertHealthy(page);
    await waitForEventStream(page);

    const policyResp = await apiRequest(page).put('/api/v1/thread-queue/policy', {
      data: { max_concurrent_total: 0 },
    });
    expect(policyResp.ok()).toBeTruthy();

    const createResp = await apiRequest(page).post('/api/v1/triggers', {
      data: {
        name: TRIGGER_NAME,
        run: { type: 'intent', intent: 'Say hello.' },
        cron_expressions: [],
        on: [{ event_type: PROBE_EVENT }],
      },
    });
    expect(createResp.ok()).toBeTruthy();

    const emitResp = await apiRequest(page).post('/api/v1/events/emit', {
      data: { event_type: PROBE_EVENT, payload: { probe: true } },
    });
    expect(emitResp.ok()).toBeTruthy();

    await openThreadQueuePanel(page);

    const row = page
      .locator('[data-role="thread-queue-row"]')
      .filter({ hasText: TRIGGER_NAME })
      .first();
    await expect(row).toBeVisible({ timeout: 15_000 });
    await expect(row).toContainText('Queued');
    await expect(row).toContainText('Event trigger');

    await row.locator('.action-btn-danger').click();
    await page.locator('.confirm-dialog').waitFor({ state: 'visible', timeout: 10_000 });
    await page.locator('.confirm-dialog [data-role="confirm-ok"]:visible').first().click();

    await expect(
      page.locator('[data-role="thread-queue-row"]').filter({ hasText: TRIGGER_NAME }),
    ).toHaveCount(0, { timeout: 15_000 });
    await expect(page.locator('[data-role="thread-queue-empty"]')).toBeVisible();
  });
});
