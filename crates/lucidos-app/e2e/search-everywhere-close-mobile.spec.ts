import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, enableMobileHeaderSticky, ensureMobileView } from './helpers';

// Touch-only regression: tapping an anchored overlay's toggle again (while the
// overlay is open) closes it and does NOT reopen. The toggle is the overlay's
// anchor, so while the overlay is open it stays interactive
// (`[data-overlay-anchor]` keeps it `pointer-events: auto` even though the rest
// of `.app-shell`'s children go inert), so the tap lands on the toggle and its
// OWN handler closes it. The anchor exemption keeps the dismiss contract from
// racing the toggle: the outside-dismiss doesn't fire on the anchor, so the
// toggle never re-flips the signal back open (the original compose/search
// reopen bug). No `force` needed: the anchor is a real actionable target, which
// is the regression guard.
//
// The mobile subject is now the LUCIDOS MARK. Search everywhere left the header
// row for the mark's menu, so the mark is the anchored overlay this viewport
// has, and Search everywhere is reached through it. Both hops are covered
// below, because the second one is a toggle-opened overlay opened from inside
// another one, which is exactly where an anchor mix-up would hide.
//
// `-mobile.spec.ts` so it only runs on the touch-enabled (`hasTouch`) projects;
// desktop chromium has no touch and `.tap()` would throw.
test.describe('Lucidos mark menu (touch)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Pin the header so opening an overlay (which may auto-focus an input)
    // can't slide the toggle off-screen. The default state, but an earlier test
    // that disabled the global pin leaks it here. Must precede navigate.
    await enableMobileHeaderSticky(page);
  });

  test('tapping the mark again closes its menu', async ({ page }) => {
    await navigateToApp(page);
    // Pin the pane, so `:visible` resolves to one known mark. Both panes carry
    // a menu toggle (HeaderMark's two placements), and only the visible pane's
    // is a real tap target, so leaving the starting pane to chance is the kind
    // of ambient dependency that fails on one project and not another.
    await ensureMobileView(page, 'thread');

    const mark = page.locator('[data-role="brand-menu-toggle"]:visible').first();
    const menu = page.locator('.brand-menu');

    await mark.tap();
    await expect(menu).toBeVisible();

    // Tap the mark again. As the overlay's anchor it stays interactive, so the
    // tap hits the mark and its own handler closes the menu. The anchor
    // exemption stops the outside-dismiss from racing it open again. Must
    // close, not reopen.
    await mark.tap();
    await expect(menu).toHaveCount(0);
  });

  test('Search everywhere opens from the menu', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');

    const mark = page.locator('[data-role="brand-menu-toggle"]:visible').first();
    const input = page.locator('.search-everywhere-input');

    await mark.tap();
    await page.locator('.brand-menu-item', { hasText: 'Search everywhere' }).tap();

    // The palette opens and the menu that launched it is gone: a menu item runs
    // its action AND closes, rather than leaving two overlays stacked.
    await expect(input).toBeVisible();
    await expect(page.locator('.brand-menu')).toHaveCount(0);
  });
});
