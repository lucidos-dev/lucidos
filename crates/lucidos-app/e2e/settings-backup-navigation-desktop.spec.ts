import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, clickVisibleElement, navigateToApp, waitForEventStream } from './helpers';

const BACKUP_PANEL_ANCHOR = '[data-search-anchor="backup:provider"]';

test.describe('navigate_ui → Settings > Backup', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('POST /api/v1/ui/navigate { settings, backup } renders the Backup panel and activates the Settings menu item', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);

    const res = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: 'backup' } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    await page.waitForFunction((sel) => {
      const panel = document.querySelector('.settings-panel');
      if (!panel) return false;
      const anchor = panel.querySelector(sel);
      if (!anchor) return false;
      const rect = anchor.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }, BACKUP_PANEL_ANCHOR, { timeout: 15_000 });

    await expect(page.locator('.pane-header-content-title').first())
      .toContainText('Backup', { timeout: 10_000 });

    await clickVisibleElement(page, '.hamburger-panel');
    await page.waitForFunction(() => {
      const active = Array.from(document.querySelectorAll('.drawer-item.active'));
      return active.length > 0 && active.every((el) => (el.textContent ?? '').trim() === 'Settings');
    }, undefined, { timeout: 10_000 });
  });
});
