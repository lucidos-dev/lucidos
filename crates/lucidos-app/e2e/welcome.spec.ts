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
    // The e2e mock model configures a provider, so WelcomeMessage renders the
    // starter-suggestions hero variant ("Hi, there!"), not the no-provider
    // ProviderSetupWelcome ("Welcome to Lucidos"). See WelcomeMessage.tsx.
    await expect(welcome.getByText('Hi, there!')).toBeVisible();

    // The entrance fades the welcome in (sequenced after the prompt move) from a
    // base `opacity: 0`. Assert it ENDS fully visible — guards against the base
    // rule ever permanently hiding the surface (the inverse of the clipping bug)
    // if the `.welcome-revealing` reveal class fails to land.
    await expect
      .poll(async () => welcome.evaluate((el) => getComputedStyle(el).opacity), { timeout: 5_000 })
      .toBe('1');

    // The clipping bug left this wrap at ~0 height. `toBeVisible` alone can't
    // catch it (the welcome's own box is non-zero even when an ancestor clips
    // it), so assert the container actually has vertical space.
    const wrap = page.locator('.thread-content-wrap:visible').first();
    const box = await wrap.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.height).toBeGreaterThan(100);
  });

  test('clicking a suggestion prefills it into the prompt (not sent)', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    // The carousel suggestion is a button — clicking it drops the text into the
    // prompt via applySuggestion so the user can edit/send, rather than read-only copy.
    const suggestion = page.locator('.welcome-carousel-item:visible').first();
    await expect(suggestion).toBeVisible();
    const text = (await suggestion.innerText()).trim();
    expect(text.length).toBeGreaterThan(0);

    await suggestion.click();

    // Lands in the prompt textarea, ready to edit/send — NOT sent, so the
    // welcome stays (no exchange yet) and the text matches the suggestion.
    const input = await waitForVisibleInput(page, 10_000);
    await expect.poll(async () => (await input.inputValue()).trim(), { timeout: 5_000 }).toBe(text);
    await expect(welcome).toBeVisible();
  });

  test('draft in progress: clicking a suggestion confirms, then overrides the prompt text', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    // Start typing a draft first — the override must not silently blow it away.
    const input = await waitForVisibleInput(page, 10_000);
    await input.click();
    await input.fill('my own half-typed idea');
    await expect.poll(async () => (await input.inputValue()).trim()).toBe('my own half-typed idea');

    const suggestion = page.locator('.welcome-carousel-item:visible').first();
    const suggestionText = (await suggestion.innerText()).trim();
    expect(suggestionText.length).toBeGreaterThan(0);

    // Click 1: a confirm guards the override. Declining keeps the draft.
    await suggestion.click();
    const dialog = page.locator('.confirm-dialog');
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await dialog.getByRole('button', { name: 'Keep my draft' }).click();
    await expect(dialog).toHaveCount(0);
    await expect.poll(async () => (await input.inputValue()).trim()).toBe('my own half-typed idea');

    // Click 2: accepting overrides the prompt text. This is the bug guard — the
    // draft/drawer updated but the visible prompt stayed stale before the fix
    // (the sync skips a focused, non-empty textarea to protect in-flight typing).
    await suggestion.click();
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await dialog.getByRole('button', { name: 'Replace' }).click();
    await expect(dialog).toHaveCount(0);
    await expect
      .poll(async () => (await input.inputValue()).trim(), { timeout: 5_000 })
      .toBe(suggestionText);
    // Still a draft (not sent) — the welcome stays.
    await expect(welcome).toBeVisible();
  });

  test('carousel viewport height is stable across all suggestions (chevrons do not bounce)', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    // Let the entrance settle before measuring: it fades in AND slides up 0.5rem,
    // so an early sample reads a transient vertical offset (the slide, not a real
    // per-slide bounce). opacity reaches 1 exactly as the slide ends (same
    // keyframe), so this is the settle signal — same wait the first test uses.
    await expect
      .poll(async () => welcome.evaluate((el) => getComputedStyle(el).opacity), { timeout: 5_000 })
      .toBe('1');

    // Suggestions vary in length, so a viewport sized to the *visible* card would
    // change height per slide and the vertically-centered chevrons would bounce.
    // The fix stacks every suggestion in one grid cell so the viewport sizes to
    // the tallest card and stays constant. Walk every slide and assert the
    // carousel height + a chevron's Y never move (this would fail on the old
    // single-card layout, where the long e-bike suggestion wraps to more lines
    // than the short ones).
    const carousel = page.locator('.welcome-carousel:visible').first();
    await expect(carousel).toBeVisible();
    const next = carousel.getByRole('button', { name: 'Next suggestion' });
    const prevChevron = carousel.getByRole('button', { name: 'Previous suggestion' });

    const heights: number[] = [];
    const chevronYs: number[] = [];
    for (;;) {
      const cbox = await carousel.boundingBox();
      const pbox = await prevChevron.boundingBox();
      expect(cbox).not.toBeNull();
      expect(pbox).not.toBeNull();
      heights.push(Math.round(cbox!.height));
      chevronYs.push(Math.round(pbox!.y));
      if (await next.isDisabled()) break;
      await next.click();
    }

    // More than one slide, or the stability assertion below is vacuous.
    expect(heights.length).toBeGreaterThan(1);
    // ±1px tolerance for sub-pixel rounding across slides.
    for (const h of heights) expect(Math.abs(h - heights[0])).toBeLessThanOrEqual(1);
    for (const y of chevronYs) expect(Math.abs(y - chevronYs[0])).toBeLessThanOrEqual(1);
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
