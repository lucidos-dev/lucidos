import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, enableMobileHeaderSticky, ensureMobileView } from './helpers';

// Touch-only regression: when an overlay is open, tapping a sibling button that
// runs its action on `touchend` (the iOS keyboard-nudge pattern in
// `composeHandlers`) must NOT fire that action. The reported case: with an
// overlay open, the first tap on such a button fired its action on touch,
// instead of the tap just dismissing the overlay.
//
// Mechanism: a composeHandlers button preventDefaults the synthetic click, so
// the dismiss contract's click-swallow never sees a click. The contract also
// swallows the `touchend` (capture phase) when an outside pointerdown armed the
// suppressor. See makeDismissHandlers / useDismissOnOutside.
//
// Two mechanisms protect this and the test guards both: while the overlay is
// open the sibling button (a non-anchor) is inert (`data-overlay-open` makes
// `.app-shell > *` pointer-events:none, which it inherits), so the tap lands on
// `.app-shell` and dismisses; and on the dismiss the paired-event swallow eats
// the touchend so the button can't fire during the re-enable race.
//
// The pairing is the THREADS pane: the Lucidos mark beside the Search threads
// button, whose `openSearchHandlers` is a composeHandlers pair. The thread
// header's own compose and search buttons moved INTO the mark's menu, so they
// are no longer siblings of an open overlay and cannot express this case; this
// row still can. Observable: composeHandlers' onTouchEnd focuses the search
// input before running its action, so "did it fire?" reduces to "did the
// thread-search input get focused?".
//
// The menu is a centred modal now, and its scrim is `pointer-events: none`, so
// the tap still reaches `.app-shell` underneath and the contract under test is
// unchanged: the sibling is inert, and the paired event is swallowed. That is
// the point of asserting it here rather than trusting the shape.
//
// Real `.tap()` (not synthetic .click()) is required: the bug only manifests
// with touch events. `-mobile.spec.ts` so it runs only on touch-enabled projects.
test.describe('Overlay swallows a touch-driven sibling action', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Pin the header so nothing slides the sibling button off-screen. The
    // default state, but an earlier test that disabled the global pin leaks
    // here. Must precede navigate.
    await enableMobileHeaderSticky(page);
  });

  test('tapping Search threads while the Lucidos menu is open dismisses it without opening search', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'threads');

    const mark = page.locator('.mobile-threads-header [data-role="brand-menu-toggle"]').first();
    const menu = page.locator('.brand-menu');
    const searchBtn = page.locator('.mobile-threads-header button[aria-label="Search threads"]').first();

    await mark.tap();
    await expect(menu).toBeVisible();

    // Tap the search button. As a non-anchor sibling it is inert behind the
    // open menu, so the tap lands on `.app-shell` and dismisses; `force` past
    // the actionability check (a real finger lands the same way). Either the
    // inert state or the paired-event swallow must keep its onTouchEnd from
    // running.
    await searchBtn.tap({ force: true });

    // Menu closed, so focus has settled by now.
    await expect(menu).toHaveCount(0);

    // And the search did NOT open: its input was never focused (the buggy path
    // would have focused it inside the button's onTouchEnd).
    const searchFocused = await page.evaluate(() => {
      const active = document.activeElement as HTMLElement | null;
      return !!active?.closest('.mobile-thread-search-bar');
    });
    expect(searchFocused).toBe(false);
  });
});
