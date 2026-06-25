import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';
import { clearAllThreads, resetWelcomePreference } from './db-helpers';

/**
 * Welcome surface — show until dismissed, and (the regression this guards) it
 * must render with real height in the compose-empty layout.
 *
 * The bug: `.thread-content` is `position: absolute; inset: 0`, and in the
 * compose-empty layout its wrap is `flex: 0 0 auto`. An absolutely-positioned
 * child gives the wrap no height, so the welcome rendered into a zero-height,
 * overflow-hidden box and was clipped to nothing — "no welcome on a fresh
 * install". The fix (shell.css) gives the wrap the column space when it holds the
 * welcome, so a `boundingBox().height` assertion on the wrap is the discriminator
 * (≈0 before, hundreds of px after).
 */
test.describe('Welcome surface', () => {
  test.beforeEach(async ({ page }) => {
    // Pristine: no threads, welcome never dismissed → the welcome must show.
    clearAllThreads();
    resetWelcomePreference();
    await assertHealthy(page);
  });

  test('shows on a fresh compose view with real (unclipped) height', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });
    await expect(welcome.getByText('Welcome to Lucidos')).toBeVisible();

    // The clipping bug left this wrap at ~0 height. `toBeVisible` alone can't
    // catch it (the welcome's own box is non-zero even when an ancestor clips
    // it), so assert the container actually has vertical space.
    const wrap = page.locator('.thread-content-wrap:visible').first();
    const box = await wrap.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.height).toBeGreaterThan(100);
  });

  test('"Don\'t show this again" dismisses it and it stays dismissed after reload', async ({ page }) => {
    await navigateToApp(page);

    // The dismiss control lives on the starter-suggestions variant (a provider
    // is configured under the e2e mock model, so this is the rendered variant).
    const dismiss = page.locator('.welcome-dismiss:visible').first();
    await expect(dismiss).toBeVisible({ timeout: 10_000 });
    await dismiss.click();

    // Gone immediately (reactive on the preference write).
    await expect(page.locator('.welcome-message')).toHaveCount(0, { timeout: 5_000 });

    // Stays gone after reload — the dismissal is the DB-backed
    // welcome_suggestions_dismissed preference, not just in-memory state.
    await page.reload();
    await navigateToApp(page);
    await waitForVisibleInput(page);
    await expect(page.locator('.welcome-message')).toHaveCount(0, { timeout: 10_000 });
  });
});
