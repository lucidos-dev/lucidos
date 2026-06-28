import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy, waitForEventStream } from './helpers';

/** Thread Queue panel end-to-end: hold all background admission via the
 *  capacity policy (`max_concurrent_total: 0`), fire an event trigger so a
 *  real entry lands in the queue, then drive the panel — the queued row must
 *  render, and Drop must remove it without running. No LLM call happens:
 *  the fire never gets admitted, which is exactly the point. */

const PROBE_EVENT = 'E2eThreadQueueProbe';
const TRIGGER_NAME = 'E2E thread-queue probe';

/** Navigate to the Thread Queue panel. Thread Queue moved under Settings →
 *  System (8c7818a10), so it's no longer a top-level drawer item; deep-link to
 *  it via the stable `thread-queue` NavigateTarget — the engine emits
 *  NavigationRequested over SSE and the page's handleNavigationRequest lands on
 *  Settings → System → Thread Queue (revealing the content pane on mobile too).
 *  Same SSE-delivered path as settings-backup-navigation-desktop.spec.ts. */
async function openThreadQueuePanel(page: Page): Promise<void> {
  await waitForEventStream(page);
  const res = await page.request.post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'thread-queue' },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  // Section headers render only once the queue state has loaded.
  await expect(page.locator('.thread-queue-section-title').first()).toBeVisible({
    timeout: 10_000,
  });
}

/** Restore the default capacity policy. `CapacityPolicy` is
 *  `#[serde(default)]`, so an empty body resets every field. */
async function restoreDefaultPolicy(page: Page): Promise<void> {
  const resp = await page.request.put('/api/v1/thread-queue/policy', { data: {} });
  expect(resp.ok()).toBeTruthy();
}

/** Delete the probe trigger if a previous (failed) run left it behind. */
async function deleteProbeTrigger(page: Page): Promise<void> {
  const list = await page.request.get('/api/v1/triggers');
  if (!list.ok()) return;
  const body = (await list.json()) as { triggers: Array<{ id: string; name: string }> };
  for (const t of body.triggers ?? []) {
    if (t.name === TRIGGER_NAME) {
      await page.request.delete(`/api/v1/triggers?id=${encodeURIComponent(t.id)}`);
    }
  }
}

test.describe('thread queue panel', () => {
  test.afterEach(async ({ page }) => {
    // Cleanup must run even when the assertion path failed mid-way: the
    // policy is persisted (CapacityPolicyChanged) and would otherwise hold
    // every background spawn of subsequent tests.
    await restoreDefaultPolicy(page);
    await deleteProbeTrigger(page);
  });

  test('queued trigger fire shows in the panel and Drop removes it', async ({ page }) => {
    await navigateToApp(page);
    await assertHealthy(page);

    // Hold ALL background admission so the fire queues instead of running.
    const policyResp = await page.request.put('/api/v1/thread-queue/policy', {
      data: { max_concurrent_total: 0 },
    });
    expect(policyResp.ok()).toBeTruthy();

    // A real event trigger — its fire is what lands in the queue.
    const createResp = await page.request.post('/api/v1/triggers', {
      data: {
        name: TRIGGER_NAME,
        run: { type: 'intent', intent: 'Say hello.' },
        cron_expressions: [],
        on: [{ event_type: PROBE_EVENT }],
      },
    });
    expect(createResp.ok()).toBeTruthy();

    // Fire the matching domain event.
    const emitResp = await page.request.post('/api/v1/events/emit', {
      data: { event_type: PROBE_EVENT, payload: { probe: true } },
    });
    expect(emitResp.ok()).toBeTruthy();

    await openThreadQueuePanel(page);

    // The queued entry renders with the trigger's name and Queued status.
    const row = page
      .locator('[data-role="thread-queue-row"]')
      .filter({ hasText: TRIGGER_NAME })
      .first();
    await expect(row).toBeVisible({ timeout: 15_000 });
    await expect(row).toContainText('Queued');
    await expect(row).toContainText('Event trigger');

    // Drop it (guarded by the confirm dialog) — the row disappears and the
    // queued empty-state returns. Nothing ever ran: admission stayed at 0.
    await row.locator('.action-btn-danger').click();
    await page.locator('.confirm-dialog').waitFor({ state: 'visible', timeout: 10_000 });
    await page.locator('.confirm-dialog .confirm-btn-ok:visible').first().click();

    await expect(
      page.locator('[data-role="thread-queue-row"]').filter({ hasText: TRIGGER_NAME }),
    ).toHaveCount(0, { timeout: 15_000 });
    await expect(page.locator('[data-role="thread-queue-empty"]')).toBeVisible();
  });
});
