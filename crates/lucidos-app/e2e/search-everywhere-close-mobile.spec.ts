import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, enableMobileHeaderSticky } from './helpers';

// Touch-only regression: tapping the search toggle again (while the palette is
// open) closes it and does NOT reopen. The toggle is the overlay's anchor, so
// while the palette is open it stays interactive (`[data-overlay-anchor]` keeps
// it `pointer-events: auto` even though the rest of `.app-shell`'s children go
// inert) — the tap lands on the toggle and its OWN handler closes the palette.
// The anchor exemption keeps the dismiss contract from racing the toggle: the
// outside-dismiss doesn't fire on the anchor, so the toggle never re-flips the
// signal back open (the original compose/search reopen bug). No `force` needed —
// the anchor is a real actionable target, which is the regression guard.
//
// `-mobile.spec.ts` so it only runs on the touch-enabled (`hasTouch`) projects;
// desktop chromium has no touch and `.tap()` would throw.
test.describe('Search everywhere toggle (touch)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Pin the header so opening the palette (which auto-focuses its input)
    // can't slide the toggle off-screen — the default state, but an earlier
    // test that disabled the global pin leaks it here. Must precede navigate.
    await enableMobileHeaderSticky(page);
  });

  test('tapping the search toggle again closes the modal', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('[data-role="search-everywhere-toggle"]:visible').first();
    const input = page.locator('.search-everywhere-input');

    // First tap opens the modal (its input only renders while open).
    await toggle.tap();
    await expect(input).toBeVisible();

    // Tap the toggle again. As the overlay's anchor it stays interactive, so the
    // tap hits the toggle and its own handler closes the palette. The anchor
    // exemption stops the outside-dismiss from racing it open again. Must close,
    // not reopen.
    await toggle.tap();
    await expect(input).toHaveCount(0);
  });
});
