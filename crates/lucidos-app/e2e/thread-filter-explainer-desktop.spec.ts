import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';
import { clearAllThreads } from './db-helpers';

// The shared **explainer** (components/shared/Explainer.tsx), exercised through
// its first consumer: the "Include deleted" checkbox in the thread filter panel.
//
// The unit tripwires (`components/shared/__tests__/explainer.test.ts`) can only
// scan source, because this project runs Vitest with no jsdom. So the actual
// behaviour lives here. It opens at its icon and carries the explanation.
// Escape, an outside click and a second tap on the icon each close it. A tap on
// the copy does NOT toggle the checkbox the explainer is nested inside: that is
// the wrapping-`<label>` hazard the portal exists to prevent.
//
// Desktop-only for the same reason as `threads-header-filter-desktop.spec.ts`:
// the `.threads-header` that opens the filter panel renders only on desktop, and
// mobile-emulated projects ignore `setViewportSize()`.

test.describe('Explainer in the thread filter panel: desktop layout', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  const openFilterPanel = async (page: import('@playwright/test').Page) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);
    await openThreadDrawer(page);
    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
    return filterBtn;
  };

  test('the info icon opens a popover explaining Include deleted, and Escape closes it', async ({ page }) => {
    await openFilterPanel(page);

    // The icon-only button's accessible name is its only name, and it is derived
    // from the title so the two cannot drift.
    const info = page.locator('.thread-filter-panel button[aria-label="About Include deleted"]');
    await expect(info).toBeVisible();
    await expect(page.locator('.explainer-popover')).toHaveCount(0);

    await info.click();
    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();
    await expect(popover).toHaveAttribute('role', 'dialog');
    await expect(popover.locator('.surface-title')).toHaveText('Include deleted');
    await expect(popover.locator('.explainer-body')).toContainText('(deleted)');
    // Focus moves into the popover, onto its X.
    await expect(popover.getByRole('button', { name: 'Close Include deleted' })).toBeFocused();

    // It opens AT the icon, inside the drawer holding it, with no scrim over
    // the app.
    await expect(page.locator('.modal-overlay')).toHaveCount(0);
    const icon = (await info.boundingBox())!;
    const panel = (await popover.boundingBox())!;
    const pane = (await page.locator('.thread-drawer').boundingBox())!;
    const below = panel.y - (icon.y + icon.height);
    const above = icon.y - (panel.y + panel.height);
    expect(Math.min(Math.abs(below), Math.abs(above)), 'panel hangs off the icon').toBeLessThan(12);
    expect(panel.x).toBeGreaterThanOrEqual(pane.x);
    expect(panel.x + panel.width).toBeLessThanOrEqual(pane.x + pane.width + 0.5);

    // Escape routes through the central overlay stack, like every <Overlay>.
    await page.keyboard.press('Escape');
    await expect(popover).toHaveCount(0);
    // ...and it closed the explainer only, not the filter panel underneath it
    // (LIFO: the newest overlay goes first).
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
  });

  test('tapping the explanation does not toggle the checkbox it is nested inside', async ({ page }) => {
    await openFilterPanel(page);

    const checkbox = page
      .locator('.thread-filter-panel label.thread-filter-option', { hasText: 'Include deleted' })
      .locator('input[type="checkbox"]');
    await expect(checkbox).not.toBeChecked();

    await page.locator('button[aria-label="About Include deleted"]').click();
    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();

    // The explainer lives inside a wrapping <label>. A label forwards activation
    // to its control for clicks on any NON-interactive descendant, so an inline
    // popover would flip "Include deleted" on every tap of a paragraph. The
    // popover is portaled to <body> precisely so this click bubbles nowhere near
    // the label.
    await popover.locator('.explainer-body p').first().click();
    await expect(popover).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    // The X in the head is the explicit way out.
    await popover.getByRole('button', { name: 'Close Include deleted' }).click();
    await expect(popover).toHaveCount(0);
    await expect(checkbox).not.toBeChecked();
  });

  test('a click outside dismisses it without reaching the panel behind, and the icon toggles it', async ({ page }) => {
    await openFilterPanel(page);

    const info = page.locator('button[aria-label="About Include deleted"]');
    const popover = page.locator('.explainer-popover');

    // The icon is the anchor, so a second press on it closes the popover
    // through its own handler.
    await info.click();
    await expect(popover).toBeVisible();
    await info.click();
    await expect(popover).toHaveCount(0);

    await info.click();
    await expect(popover).toBeVisible();
    // The whole UI behind goes inert while it is open.
    await expect(page.locator('html[data-overlay-open]')).toHaveCount(1);

    // A raw click well clear of the popover, in the Canvas pane. Behind an open
    // overlay it lands on `.app-shell`, which the dismiss contract reads as
    // outside.
    const canvas = (await page.locator('.split-layout > .pane-content').boundingBox())!;
    await page.mouse.click(canvas.x + canvas.width / 2, canvas.y + canvas.height / 2);
    await expect(popover).toHaveCount(0);

    // The click was swallowed: the filter panel is untouched, and nothing in
    // it changed. No type narrows the list, and "Include deleted" is still off.
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
    // "Filters", not "Thread filters": the pane is already the Threads pane, so
    // the row says the short form (AppHeader, and the mobile row matching it).
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(page
      .locator('.thread-filter-panel .thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' })
      .locator('input[type="checkbox"]')).toBeChecked();
    await expect(page
      .locator('.thread-filter-panel label.thread-filter-option', { hasText: 'Include deleted' })
      .locator('input[type="checkbox"]')).not.toBeChecked();

    // And it reopens at once: the dismiss spent its one swallow on the click it
    // was armed for, so the next press on the icon lands.
    await info.click();
    await expect(popover).toBeVisible();
  });
});
