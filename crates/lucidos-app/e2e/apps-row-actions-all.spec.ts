/**
 * An apps panel row acts like a thread drawer row. A pin toggle is always
 * drawn, and a press on it pins in place without opening the app. Edit and
 * Delete sit in the row's menu. On desktop the row draws its ⋯, which the
 * keyboard reaches, and a right-click opens the menu too. On a phone the row
 * draws no ⋯ and a long press opens the menu.
 */
import { test, expect, type Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';

const APP_ID = 'e2e-apps-row-actions';
const APP_NAME = 'Row Actions Fixture';

let fixture: { cleanup: () => void } | null = null;

async function openAppsPanel(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'apps', params: {} },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate apps -> ${res.status()}`).toBeTruthy();
}

const row = (page: Page) => page.locator('.apps-view .app-row:visible', { hasText: APP_NAME });
const pin = (page: Page) => row(page).locator('button[aria-pressed]');
const trigger = (page: Page) => row(page).getByRole('button', { name: `Actions for ${APP_NAME}` });
const menu = (page: Page) => page.getByRole('menu');
const appFrame = (page: Page) => page.locator(`[data-role="app-ui-frame"][src*="/app/${APP_ID}/"]:visible`);
const MENU_ITEMS = ['Edit', /^Add to dashboard/, 'Delete'];

/** Leaves the fixture unpinned, whatever the test before it did. */
async function unpin(page: Page): Promise<void> {
  if ((await pin(page).getAttribute('aria-pressed')) === 'true') {
    await pin(page).click();
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'false');
  }
}

/** Holds the row's description past the long-press threshold with the pointer
 *  still. Playwright cannot hold a touchscreen tap. `useLongPress` reads no
 *  `pointerType`, so a real mouse press runs the same path. It also goes
 *  through real hit-testing, and the browser pairs its own click with the lift.
 *  A phone slides the content pane in, so a box read at once can sit off
 *  screen. `hover()` waits until the row is still and takes the hit there. */
async function longPressRow(page: Page): Promise<void> {
  await row(page).locator('.app-row-description').hover();
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
}

test.describe('the apps panel row actions', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      html: '<!doctype html><html><body><script src="script.js"></script></body></html>',
      js: '',
      manifest: { id: APP_ID, name: APP_NAME, description: 'e2e fixture' },
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
    await openAppsPanel(page);
    await expect(row(page)).toBeVisible({ timeout: 15_000 });
    await unpin(page);
  });

  test('the row draws an outline pin toggle named for its app, and no marker', async ({ page }) => {
    await expect(pin(page)).toBeVisible();
    await expect(pin(page)).toHaveAccessibleName(`Pin ${APP_NAME}`);
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'false');
    await expect(pin(page).locator('svg')).toHaveAttribute('fill', 'none');
    await expect(row(page).getByRole('img', { name: 'Pinned' })).toHaveCount(0);
    for (const name of ['Edit', 'Delete']) {
      await expect(row(page).getByRole('button', { name, exact: true })).toHaveCount(0);
    }
  });

  test('the pin toggles in place, fills with the accent, and does not open the app', async ({ page }) => {
    await pin(page).click();
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'true');
    await expect(pin(page)).toHaveAccessibleName(`Unpin ${APP_NAME}`);
    await expect(pin(page).locator('svg')).toHaveAttribute('fill', 'currentColor');
    // The button's colour transitions, so wait for it to settle on the accent.
    await expect.poll(() => page.evaluate(() => {
      const probe = document.createElement('span');
      probe.style.color = 'var(--accent)';
      document.body.appendChild(probe);
      const accent = getComputedStyle(probe).color;
      probe.remove();
      const pinned = document.querySelector('.apps-view .app-row button[aria-pressed="true"]');
      return pinned !== null && getComputedStyle(pinned).color === accent;
    })).toBe(true);
    await expect(menu(page)).toHaveCount(0);
    await expect(appFrame(page)).toHaveCount(0);
    await expect(row(page)).toBeVisible();

    await pin(page).click();
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'false');
    await expect(pin(page)).toHaveAccessibleName(`Pin ${APP_NAME}`);
    await expect(appFrame(page)).toHaveCount(0);
  });

  test('a press on the row still opens the app', async ({ page }) => {
    await row(page).locator('.app-row-description').click();
    await expect(appFrame(page)).toHaveCount(1, { timeout: 15_000 });
  });

  test('a phone row draws no ⋯, and a long press opens the menu without opening the app', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'a desktop row keeps its ⋯ and has no hold');
    await expect(trigger(page)).toHaveCount(0);
    await longPressRow(page);
    await expect(menu(page).getByRole('menuitem')).toHaveText(MENU_ITEMS);
    await expect(appFrame(page)).toHaveCount(0);

    await menu(page).getByRole('menuitem', { name: 'Edit', exact: true }).click();
    await expect(menu(page)).toHaveCount(0);
    await expect(page.locator('.inline-form:visible input').first()).toHaveValue(APP_NAME, { timeout: 10_000 });
  });

  test('the desktop ⋯ opens the menu, and Edit opens the edit form', async ({ page }) => {
    test.skip(isMobileViewport(page), 'a phone row draws no ⋯');
    await trigger(page).click();
    await expect(menu(page).getByRole('menuitem')).toHaveText(MENU_ITEMS);
    await expect(trigger(page)).toHaveAttribute('aria-expanded', 'true');

    await menu(page).getByRole('menuitem', { name: 'Edit', exact: true }).click();
    await expect(menu(page)).toHaveCount(0);
    await expect(page.locator('.inline-form:visible input').first()).toHaveValue(APP_NAME, { timeout: 10_000 });
  });

  test('a right-click on the row opens the same menu', async ({ page }) => {
    test.skip(isMobileViewport(page), 'a phone has no right-click');
    await row(page).locator('.app-row-description').click({ button: 'right' });
    await expect(menu(page).getByRole('menuitem')).toHaveText(MENU_ITEMS);
    await page.keyboard.press('Escape');
    await expect(menu(page)).toHaveCount(0);
    await expect(appFrame(page)).toHaveCount(0);
  });

  test('the keyboard toggles the pin, opens the menu from the ⋯, and Escape hands focus back', async ({ page }) => {
    test.skip(isMobileViewport(page), 'a phone has no keyboard focus to rove');
    await pin(page).focus();
    await page.keyboard.press('Space');
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'true');
    await expect(pin(page)).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(pin(page)).toHaveAttribute('aria-pressed', 'false');
    await expect(appFrame(page)).toHaveCount(0);

    await page.keyboard.press('Tab');
    await expect(trigger(page)).toBeFocused();
    await page.keyboard.press('Enter');
    const items = menu(page).getByRole('menuitem');
    await expect(items.first()).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(items.nth(1)).toBeFocused();
    await expect(items.nth(1)).toHaveText(/^Add to dashboard/);
    await page.keyboard.press('ArrowDown');
    await expect(items.nth(2)).toBeFocused();
    await expect(items.nth(2)).toHaveText('Delete');
    await page.keyboard.press('Escape');
    await expect(menu(page)).toHaveCount(0);
    await expect(trigger(page)).toBeFocused();
  });
});
