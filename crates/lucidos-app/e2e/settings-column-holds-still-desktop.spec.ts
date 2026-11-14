/**
 * The settings column holds still between a page that fits the pane and one
 * that outgrows it. A classic scrollbar coming and going would slide the whole
 * column sideways. The pane reserves the scroll gutter for a column view
 * (panels/shell.css).
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

// Headless Chromium launches with `--hide-scrollbars`, which removes the
// scrollbar whose arrival this spec measures. The height puts the System list
// inside the pane and Appearance past it.
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

  // The System list is a column of ten rows, inside the pane.
  await openSettingsView(page, 'system');
  await expect(page.locator('.settings-nav-row').first()).toBeVisible();
  await expect.poll(overflows, { message: 'the System list outgrew the pane' }).toBe(false);
  const fits = await rightEdge();

  // Appearance is several sections deep, past the pane.
  await openSettingsView(page, 'appearance');
  // The loaded strip only: the picker's reserve and skeleton draw one too.
  await expect(page.locator('.theme-carousel[role="radiogroup"]')).toBeVisible();
  // Proves the page outgrew the pane, or the check below proves nothing.
  await expect.poll(overflows, { message: 'Appearance never outgrew the pane' }).toBe(true);
  expect(await rightEdge(), 'the column moved as the page outgrew the pane').toBe(fits);
});
