import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

test('Sandbox agents fits the permissions row at the preferred UI scale', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
  });
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize)).toBe('18px');
  await openSettingsView(page, 'permissions');
  const row = page.locator('.settings-panel:visible [data-search-anchor="agent-sandbox:unsandboxed"]');
  await expect(row).toBeVisible();
  const toggle = row.getByRole('checkbox', { name: 'Sandbox agents' });
  await expect(toggle).toBeAttached();
  await row.scrollIntoViewIfNeeded();
  const label = row.locator('.settings-row-label');
  await expect(label).toHaveText('Sandbox agents');
  await expect.poll(() => row.evaluate(el => {
    const label = el.querySelector('.settings-row-label')!.getBoundingClientRect();
    const control = el.querySelector('.toggle-switch')!.getBoundingClientRect();
    return {
      labelFits: label.right <= control.left,
      rowFits: el.getBoundingClientRect().right <= window.innerWidth,
    };
  })).toEqual({ labelFits: true, rowFits: true });
});
