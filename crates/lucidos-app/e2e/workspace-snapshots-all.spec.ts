import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, shellDeviceId, waitForEventStream } from './helpers';

test('Settings lists a captured snapshot and confirms its target before restore', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);
  const label = `Snapshot browser fixture ${Date.now()}`;
  const capture = await apiRequest(page).post('/api/v1/snapshots', {
    data: { label, keep: false },
  });
  expect(capture.status()).toBe(202);
  const job = await capture.json() as { handle: string; job_id: string };
  let handle: string | undefined = job.handle;
  const device = await shellDeviceId(page);
  let workspaceName = '';
  let restoreRequests = 0;
  page.on('request', request => {
    if (request.method() === 'POST' && /\/snapshots\/[^/]+\/restore$/.test(new URL(request.url()).pathname)) restoreRequests++;
  });
  try {
    let phase = 'running';
    await expect.poll(async () => {
      const response = await page.request.get(`/api/v1/snapshots/jobs/${job.job_id}`, { headers: { 'x-lucidos-device-id': device } });
      expect(response.ok()).toBeTruthy();
      phase = (await response.json() as { state: { phase: string } }).state.phase;
      return phase;
    }, { timeout: 60_000 }).not.toBe('running');
    expect(phase).toBe('completed');
    await expect.poll(async () => {
      const response = await page.request.get('/api/v1/snapshots', { headers: { 'x-lucidos-device-id': device } });
      expect(response.ok()).toBeTruthy();
      const entries = await response.json() as Array<{ handle: string; workspace_name: string }>;
      const entry = entries.find(entry => entry.handle === handle);
      workspaceName = entry?.workspace_name ?? '';
      return entry !== undefined;
    }, { timeout: 60_000 }).toBeTruthy();
    const navigation = await apiRequest(page).post('/api/v1/ui/navigate', {
      data: { target: 'settings', params: { settings_view: 'backup' } },
    });
    expect(navigation.ok()).toBeTruthy();
    const row = page.locator('.settings-section .list-row').filter({ hasText: label });
    await expect(row).toBeVisible();
    await row.getByRole('button', { name: 'Keep', exact: true }).click();
    await expect(row.getByRole('button', { name: 'Unkeep', exact: true })).toBeEnabled();
    await row.getByRole('button', { name: 'Unkeep', exact: true }).click();
    await expect(row.getByRole('button', { name: 'Keep', exact: true })).toBeEnabled();
    await row.getByRole('button', { name: 'Restore', exact: true }).click();
    // The harness attaches a gateway identity only when a gateway lists this
    // engine (scripts/lib/e2e_gateway.sh). Without one, restore is refused.
    const gatewayIdentity = await page.locator('meta[name="lucidos-workspace-id"]').count() > 0;
    if (gatewayIdentity) {
      const confirmation = page.getByRole('dialog').filter({ hasText: 'Restore workspace' });
      await expect(confirmation).toContainText(`Restore workspace "${workspaceName}"`);
      await expect(confirmation).toContainText('external side effects');
      await expect(confirmation).toContainText('Running work will stop');
      await confirmation.locator('[data-role="confirm-cancel"]').click();
      await expect(confirmation).not.toBeVisible();
    } else {
      await expect(page.locator('.toast-container')).toContainText('Restoring needs the Lucidos gateway');
    }
    expect(restoreRequests).toBe(0);
    await row.getByRole('button', { name: 'Delete', exact: true }).click();
    await page.locator('[data-role="confirm-ok"]').click();
    await expect(row).toHaveCount(0);
    handle = undefined;
  } finally {
    if (handle) {
      await expect.poll(async () => {
        const removed = await apiRequest(page).delete(`/api/v1/snapshots/${handle}`);
        return removed.ok() || removed.status() === 404;
      }, { timeout: 60_000 }).toBeTruthy();
    }
  }
});
