import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

test.use({
  viewport: { width: 1280, height: 600 },
  launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
});

test('the settings column holds still as the page grows past the pane', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);

  const body = page.locator('.content-pane-body');
  const column = page.locator('.content-pane-body > .settings-panel');
  const overflows = () => body.evaluate(el => el.scrollHeight > el.clientHeight);
  const rightEdge = async () => {
    const box = await column.boundingBox();
    return box ? box.x + box.width : null;
  };

  await openSettingsView(page, 'locale');
  await expect(page.locator('[data-search-anchor="locale:timezone"]')).toBeVisible();
  await expect.poll(overflows, { message: 'the Locale page outgrew the pane' }).toBe(false);
  const fits = await rightEdge();

  await openSettingsView(page, 'appearance');
  await expect(page.locator('.theme-carousel[role="radiogroup"]')).toBeVisible();
  await expect.poll(overflows, { message: 'Appearance never outgrew the pane' }).toBe(true);
  expect(await rightEdge(), 'the column moved as the page outgrew the pane').toBe(fits);
});
