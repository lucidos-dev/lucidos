import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput, waitForResponse } from './helpers';

/**
 * Keyboard-first composer focus, desktop only.
 *
 * Both behaviors here are about the caret being where the user's next keystroke
 * expects it. Enter-to-submit is itself desktop-only (on mobile Enter inserts a
 * newline and Send is a button), and the header-click half deliberately does not
 * run on mobile either: raising the keyboard over the conversation on a header
 * tap is not what a phone user asked for.
 */
test.describe('Composer focus (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('sending with Enter leaves the caret in the composer', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.fill('Hello from the composer focus spec');
    await input.press('Enter');
    await waitForResponse(page);

    // The send re-parents the prompt (the compose→docked FLIP), which drops
    // focus on its own, so this is a real restore rather than "focus never
    // moved". A follow-up is usually the next thing typed.
    await expect.poll(
      () => page.evaluate(() =>
        (document.activeElement as HTMLElement | null)?.dataset?.role ?? null),
      { intervals: [200], timeout: 10_000 },
    ).toBe('prompt-input');
  });

  test('clicking the thread pane header puts the caret in the composer', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    // Park focus somewhere else first, so the assertion cannot pass by the
    // composer simply never having lost it.
    await input.evaluate((el: HTMLTextAreaElement) => el.blur());
    await expect.poll(() => page.evaluate(() =>
      (document.activeElement as HTMLElement | null)?.dataset?.role ?? null)).not.toBe('prompt-input');

    // The header's own chrome, not a control inside it. Dispatched at the brand
    // container rather than clicked by coordinate: every visible pixel of that
    // band belongs to some control (nav buttons, the brand label that toggles
    // the control panel, Search, compose), so a positional click would land on
    // one of those and prove the wrong thing. Those controls own their click and
    // must NOT pull focus here, which is the other half of the handler.
    await page.locator('.desktop-header .pane-header-brand:visible').first()
      .dispatchEvent('click');

    await expect.poll(
      () => page.evaluate(() =>
        (document.activeElement as HTMLElement | null)?.dataset?.role ?? null),
      { intervals: [200], timeout: 5_000 },
    ).toBe('prompt-input');
  });
});
