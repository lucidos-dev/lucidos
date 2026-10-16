/**
 * No focus ring is cut by a box that scrolls or clips. Each page is walked
 * whole, so the header, drawer and composer beside it are checked too, and
 * each sample dialog is walked on its own. The walk lives in focusRingClip.ts.
 */
import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';
import { formatClipped, walkFocusRings } from './focusRingClip';

const SETTINGS_VIEWS = [
  'main', 'system', 'system-overview', 'models', 'appearance', 'memory', 'devices',
  'accounts', 'backup', 'coding-agents', 'locale', 'marketplaces', 'permissions',
  'mcp', 'keyboard-shortcuts', 'access', 'webhooks', 'environment-variables',
  'thread-queue', 'release-notices', 'debugging', 'communication-surfaces',
];

const SAMPLE_DIALOGS = [
  'Confirm, danger', 'Confirm, default', 'Thread wants to open', 'Prompt',
  'Cannot deliver', 'Applied, deferred', 'Not served yet',
];

async function settled(page: Page): Promise<void> {
  await expect(page.locator('.content-pane-body .settings-panel')).toBeVisible();
  await expect(page.locator('.content-pane-body .sk-bar')).toHaveCount(0, { timeout: 15_000 });
}

async function expectNoCutRings(page: Page, where: string, scope?: string): Promise<void> {
  const walk = await walkFocusRings(page, scope);
  expect(walk.checked, `${where}: the walk found no controls`).toBeGreaterThan(0);
  expect.soft(walk.notFocusVisible, `${where}: focused without :focus-visible`).toEqual([]);
  expect.soft(walk.clipped, `${where}:\n${formatClipped(walk.clipped)}`).toEqual([]);
}

test.describe('focus rings are never clipped', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('on every settings page', async ({ page }) => {
    const paneText = () => page.locator('.content-pane-body').innerText();
    for (const view of SETTINGS_VIEWS) {
      const before = await paneText();
      await openSettingsView(page, view);
      await expect.poll(paneText, { message: `settings/${view} never rendered` }).not.toBe(before);
      await settled(page);
      await expectNoCutRings(page, `settings/${view}`);
      const overflowsSideways = await page.locator('.content-pane-body')
        .evaluate(el => el.scrollWidth > el.clientWidth);
      expect.soft(overflowsSideways, `settings/${view} scrolls sideways`).toBe(false);
    }
  });

  test('in the forms a settings row opens', async ({ page }) => {
    await openSettingsView(page, 'environment-variables');
    await settled(page);
    await page.locator('.list-row-add-card', { hasText: 'Add Environment Variable' }).click();
    await expectNoCutRings(page, 'environment variable form', '.repo-add-form');

    await openSettingsView(page, 'webhooks');
    await settled(page);
    await page.locator('.list-row-add-card', { hasText: 'Add Webhook' }).click();
    await page.locator('.accent-link', { hasText: 'This sender signs its deliveries' }).click();
    await expectNoCutRings(page, 'webhook form', '.list-row:has(.device-name-input)');

    await openSettingsView(page, 'devices');
    await settled(page);
    await page.locator('.device-current .device-name').click();
    await expect(page.locator('.device-current .device-name-input')).toBeFocused();
    await expectNoCutRings(page, 'device rename', '.device-current');
  });

  test('in every sample dialog', async ({ page }) => {
    await openSettingsView(page, 'communication-surfaces');
    await settled(page);
    const DIALOG = '[data-overlay-panel][aria-modal="true"]';
    const dialog = page.locator(DIALOG);
    for (const label of SAMPLE_DIALOGS) {
      await page.locator('.surfaces-sample-buttons .action-btn', { hasText: label }).click();
      await expect(dialog).toBeVisible();
      await expectNoCutRings(page, `dialog "${label}"`, DIALOG);
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
    }
  });
});
