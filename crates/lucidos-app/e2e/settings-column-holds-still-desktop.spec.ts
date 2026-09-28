/**
 * The settings column holds still as the theme gallery unfolds and folds. The
 * page grows past the pane and shrinks back. A classic scrollbar coming and
 * going would slide the whole column sideways as the roll lands. The pane
 * reserves the scroll gutter for a column view (panels/shell.css).
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

// Headless Chromium launches with `--hide-scrollbars`, which removes the
// scrollbar whose arrival this spec measures.
test.use({
  viewport: { width: 1280, height: 900 },
  launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
});

test('the settings column holds still as the theme gallery unfolds and folds', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);
  const nav = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(nav.ok(), `POST /api/v1/ui/navigate -> ${nav.status()}`).toBeTruthy();

  const column = page.locator('.content-pane-body > .settings-panel');
  const rightEdge = async () => {
    const box = await column.boundingBox();
    return box ? box.x + box.width : null;
  };
  const rolling = page.locator('.disclosure.is-rolling');
  const radios = page.locator('.theme-card[role="radio"]');
  await expect(page.locator('.theme-toggle')).toBeVisible();
  const shut = await rightEdge();

  await page.locator('.theme-toggle').click();
  await expect(radios.first()).toBeVisible();
  await expect(rolling).toHaveCount(0);
  // Proves the page outgrew the pane, or the check below proves nothing.
  const overflows = await page.locator('.content-pane-body').evaluate(el => el.scrollHeight > el.clientHeight);
  expect(overflows, 'the unfolded gallery never outgrew the pane').toBe(true);
  expect(await rightEdge(), 'the column moved as the gallery unfolded').toBe(shut);

  await page.locator('.settings-section-title', { hasText: 'Typography' }).click();
  await expect(radios).toHaveCount(0);
  await expect(rolling).toHaveCount(0);
  expect(await rightEdge(), 'the column moved as the gallery folded').toBe(shut);
});
