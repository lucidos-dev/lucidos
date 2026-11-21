import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

const MANAGE = '**/~/api/v1/remote-gateways/cloud/manage';

/** Serve the disposable engine as the remote workspace `notes@cloud`. The
 *  page's base names the remote route, and its requests reach the engine. */
async function servedAsRemoteWorkspace(page: Page): Promise<void> {
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.isNavigationRequest() && url.pathname === '/') {
      const response = await route.fetch();
      const html = (await response.text()).replace(/<base\s[^>]*>/g, '');
      await route.fulfill({ response, body: html.replace('<head>', '<head><base href="/notes@cloud/">') });
    } else if (url.pathname.startsWith('/notes@cloud/')) {
      url.pathname = url.pathname.slice('/notes@cloud'.length);
      await route.continue({ url: url.toString() });
    } else await route.fallback();
  });
}

/** Open Settings → Devices through Search Everywhere. The navigate API would
 *  need the event stream, which this rewritten route does not carry. */
async function openDevices(page: Page): Promise<void> {
  await page.keyboard.press('ControlOrMeta+k');
  const input = page.locator('.search-everywhere-input');
  await expect(input).toBeVisible();
  await input.fill('Devices');
  await page.locator('.search-everywhere-result', { hasText: 'Devices' }).first().click();
}

const sam = { id: 'u1', name: 'Sam', emails: [], device_ids: ['b1'] };
const laptop = { id: 'b1', label: 'Office laptop', paired_at: '2026-01-01T00:00:00Z' };

test("a remote workspace manages the remote gateway's devices through the hop", async ({ page }) => {
  await assertHealthy(page);
  await servedAsRemoteWorkspace(page);
  const revoked: string[] = [];
  let devices = [laptop];
  await page.route(`${MANAGE}/devices`, (route) => route.fulfill({ json: devices }));
  await page.route(`${MANAGE}/users`, (route) => route.fulfill({ json: { users: [sam], relayed: [] } }));
  await page.route(`${MANAGE}/devices/b1`, async (route) => {
    revoked.push(route.request().method());
    devices = [];
    await route.fulfill({ status: 204 });
  });
  await navigateToApp(page);
  await openDevices(page);
  const section = page.locator('.settings-section', { hasText: 'On cloud' });
  await expect(section.getByText('Office laptop')).toBeVisible();
  await section.getByRole('button', { name: 'Revoke' }).click();
  await page.getByRole('button', { name: 'Revoke', exact: true }).last().click();
  await expect.poll(() => revoked).toEqual(['DELETE']);
  await expect(section.getByText('No device is paired with cloud itself.')).toBeVisible();
});

test('a remote gateway without the grant says how to turn it on', async ({ page }) => {
  await assertHealthy(page);
  await servedAsRemoteWorkspace(page);
  const refusal = { error: 'not granted', reason_kind: 'manage-not-granted' };
  await page.route(`${MANAGE}/**`, (route) => route.fulfill({ status: 403, json: refusal }));
  await navigateToApp(page);
  await openDevices(page);
  await expect(page.locator('[data-role="manage-not-granted"]')).toContainText('Pair a gateway');
});
