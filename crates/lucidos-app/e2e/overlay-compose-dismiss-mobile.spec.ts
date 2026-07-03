import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, enableMobileHeaderSticky } from './helpers';

// Touch-only regression: when an overlay is open, tapping a sibling button that
// runs its action on `touchend` (the iOS keyboard-nudge pattern in
// `composeHandlers`) must NOT fire that action. The reported case: with the
// search palette open, the first tap on the compose / "New thread" button fired
// the compose action on touch, instead of the tap just dismissing the palette.
//
// Mechanism: the compose button preventDefaults the synthetic click, so the
// dismiss contract's click-swallow never sees a click. The contract now also
// swallows the `touchend` (capture phase) when an outside pointerdown armed the
// suppressor — see makeDismissHandlers / useDismissOnOutside.
//
// Two mechanisms now protect this, and the test guards both: while the palette
// is open the compose button (a non-anchor sibling) is inert (`data-overlay-open`
// → `.app-shell > *` pointer-events:none, which it inherits), so the tap lands on
// `.app-shell` and dismisses; and on the dismiss the paired-event swallow eats
// the touchend so the button can't fire during the re-enable race. Observable:
// `composeHandlers`' onTouchEnd
// calls `focusPromptNow()` (focusing the prompt textarea) before its action, so
// "did compose fire?" reduces to "did the prompt input get focused?". Real
// `.tap()` (not synthetic .click()) is required — the bug only manifests with
// touch events. `-mobile.spec.ts` so it runs only on the touch-enabled projects.
test.describe('Overlay swallows a touch-driven sibling action (compose)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Pin the header so the palette's auto-focused input can't slide the
    // "New thread" button off-screen — the default state, but an earlier test
    // that disabled the global pin leaks it here. Must precede navigate.
    await enableMobileHeaderSticky(page);
  });

  test('tapping New thread while the search palette is open dismisses it without firing compose', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('[data-role="search-everywhere-toggle"]:visible').first();
    const searchInput = page.locator('.search-everywhere-input');
    const composeBtn = page.locator('.mobile-thread-header button[aria-label="New thread"]').first();

    // Open the search palette (its input only renders while open).
    await toggle.tap();
    await expect(searchInput).toBeVisible();

    // Tap the compose button. As a non-anchor sibling it's inert behind the open
    // palette, so the tap lands on `.app-shell` and dismisses; `force` past the
    // actionability check (a real finger lands the same way). Either the inert
    // state or the paired-event swallow must keep composeHandlers' onTouchEnd
    // (focusPromptNow + unfocusThread) from running.
    await composeBtn.tap({ force: true });

    // Palette closed (its input unmounts on close) — focus has settled by now.
    await expect(searchInput).toHaveCount(0);

    // … and compose did NOT fire: the prompt textarea was never focused (the
    // buggy path would have called focusPromptNow() inside the compose button's
    // onTouchEnd before unfocusThread()).
    const promptFocused = await page.evaluate(
      () => (document.activeElement as HTMLElement | null)?.dataset?.role === 'prompt-input',
    );
    expect(promptFocused).toBe(false);
  });
});
