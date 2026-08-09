import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';
import { clearAllThreads } from './db-helpers';

// The shared **explainer** (components/shared/Explainer.tsx), exercised through
// its first consumer: the "Include deleted" checkbox in the thread filter panel.
//
// The unit tripwires (`components/shared/__tests__/explainer.test.ts`) can only
// scan source, because this project runs Vitest with no jsdom. So the actual
// behaviour lives here: it opens, it carries the explanation, Escape closes it,
// an outside click closes it, and a tap on the copy does NOT toggle the checkbox
// the explainer is nested inside (the wrapping-`<label>` hazard the portal
// exists to prevent).
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

  test('the info icon opens a dialog explaining Include deleted, and Escape closes it', async ({ page }) => {
    await openFilterPanel(page);

    // The icon-only button's accessible name is its only name, and it is derived
    // from the title so the two cannot drift.
    const info = page.locator('.thread-filter-panel button[aria-label="About Include deleted"]');
    await expect(info).toBeVisible();
    await expect(page.locator('.explainer-dialog')).toHaveCount(0);

    await info.click();
    const dialog = page.locator('.explainer-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('role', 'dialog');
    await expect(dialog.locator('.explainer-title')).toHaveText('Include deleted');
    await expect(dialog.locator('.explainer-body')).toContainText('(deleted)');

    // Escape routes through the central overlay stack, like every <Overlay>.
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
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
    const dialog = page.locator('.explainer-dialog');
    await expect(dialog).toBeVisible();

    // The explainer lives inside a wrapping <label>. A label forwards activation
    // to its control for clicks on any NON-interactive descendant, so an inline
    // dialog would flip "Include deleted" on every tap of a paragraph. The
    // dialog is portaled to <body> precisely so this click bubbles nowhere near
    // the label.
    await dialog.locator('.explainer-body p').first().click();
    await expect(dialog).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    // The Close button is the explicit way out.
    await dialog.getByRole('button', { name: 'Close' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(checkbox).not.toBeChecked();
  });

  test('a click on the backdrop dismisses it without reaching the panel behind', async ({ page }) => {
    await openFilterPanel(page);

    const info = page.locator('button[aria-label="About Include deleted"]');
    const dialog = page.locator('.explainer-dialog');

    await info.click();
    await expect(dialog).toBeVisible();
    // The whole UI behind goes inert while it is open.
    await expect(page.locator('html[data-overlay-open]')).toHaveCount(1);

    // This is a BACKDROP modal, so the way out is the scrim, Escape, or Close.
    // Re-tapping the anchor is deliberately unreachable: `.modal-overlay` covers
    // the whole viewport, including the icon. (The anchor is still passed to
    // <Overlay> as the rule requires; it is simply never the exit here, unlike
    // an anchored popover.) Click the top-left corner of the scrim, well clear
    // of the centered panel.
    await page.locator('.modal-overlay').click({ position: { x: 5, y: 5 } });
    await expect(dialog).toHaveCount(0);

    // The scrim swallowed the click: the filter panel underneath is untouched,
    // and the status it was showing did not change.
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
    // "Filters", not "Thread filters": the pane is already the Threads pane, so
    // the row says the short form (AppHeader, and the mobile row matching it).
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(
      page.locator('.thread-filter-panel .drawer-view-option-active .drawer-view-label'),
    ).toHaveText('All statuses');

    // And it reopens, so the dismiss did not leave the toggle permanently
    // wedged.
    //
    // The wait is NOT flake padding and must not be deleted. A backdrop dismiss
    // arms two paired-click swallowers (the local `suppressNextClick` flag and
    // the document one-shot from `installPairedSwallow`, both in
    // `hooks/useAnchoredPopover.ts`), and only one of them consumes the click
    // the pair was armed for. The other stays armed until the one-shot's
    // 1500ms fuse, so a click inside that window is eaten wherever it lands.
    // Measured here: reopening immediately fails, reopening after the fuse
    // passes. That is pre-existing behavior of the shared dismiss contract,
    // affecting every backdrop overlay (confirm, prompt, image popup) and not
    // this component, whose files this branch does not touch. Waiting past the
    // fuse is what lets this spec assert the property it is actually about.
    await page.waitForTimeout(1700);
    await info.click();
    await expect(dialog).toBeVisible();
  });
});
