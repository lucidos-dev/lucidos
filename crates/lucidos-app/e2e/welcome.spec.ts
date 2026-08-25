import { test, expect, Locator, Page } from './fixtures';
import { navigateToApp, assertHealthy, assertUserMessagesVisible, waitForVisibleInput, isMobileViewport } from './helpers';
import { clearAllThreads, resetWelcomePreference } from './db-helpers';

/** Press the setup-interview button the way the running device would.
 *
 *  A touch project TAPS. `composeHandlers` (promptFocus.ts) runs the action on
 *  `touchend`, the gesture a phone actually produces, and the only one that
 *  survives a reflow mid-press.
 *
 *  A mouse press does not survive it. Pressing the button blurs the composer,
 *  the composer resizes, and at a phone width the welcome below it moves before
 *  the browser synthesizes the `click`. That press lands on `.thread-content`,
 *  so the button never fires. Desktop has no touch, so `tap()` would throw and
 *  the mouse path is the real one there. */
async function pressInterview(page: Page, button: Locator): Promise<void> {
  if (isMobileViewport(page)) await button.tap();
  else await button.click();
}

/**
 * Welcome surface: show until dismissed, its one action (the setup interview),
 * and (the regression this guards) it must render with real height in the
 * compose-empty layout.
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
    // setup-interview hero variant ("Hi, there!"), not the no-provider
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

  test('the setup-interview button is the only action, and it sends on one click', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    // One action, nothing else to weigh it against: the starter suggestions and
    // their "Or ask me anything" lead-in were removed, so a newcomer has exactly
    // one thing to press. Counting the surface's BUTTONS is what pins that (the
    // interview plus the dismiss pill, and nothing else). A locator for the
    // deleted carousel classes would pass trivially and pin nothing, and the
    // unit suite already asserts their absence from the markup.
    const start = page.locator('.welcome-setup-interview-btn:visible').first();
    await expect(start).toBeVisible();
    await expect(start).toHaveText(/Help me get the most out of Lucidos/);
    await expect(welcome.locator('button')).toHaveCount(2);

    // It SENDS rather than prefilling (startSetupInterview), so the interview
    // starts on one gesture: the seeded sentence lands in the transcript as a
    // user message and the prompt is left empty.
    await pressInterview(page, start);
    await assertUserMessagesVisible(page, ['Help me get the most out of Lucidos']);
    const input = await waitForVisibleInput(page, 10_000);
    await expect.poll(async () => (await input.inputValue()).trim(), { timeout: 5_000 }).toBe('');
  });

  test('draft in progress: the interview confirms before it replaces the typed text', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    // Start typing a draft first. The interview seeds the prompt through
    // applySuggestion, which REPLACES the whole input, so the click must not
    // silently blow away typed text.
    const input = await waitForVisibleInput(page, 10_000);
    await input.click();
    await input.fill('my own half-typed idea');
    await expect.poll(async () => (await input.inputValue()).trim()).toBe('my own half-typed idea');

    // Click 1: declining keeps the draft AND sends nothing (startSetupInterview
    // bails on a false return from applySuggestion).
    const start = page.locator('.welcome-setup-interview-btn:visible').first();
    await pressInterview(page, start);
    const dialog = page.locator('.confirm-dialog');
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await dialog.getByRole('button', { name: 'Keep my draft' }).click();
    await expect(dialog).toHaveCount(0);
    await expect.poll(async () => (await input.inputValue()).trim()).toBe('my own half-typed idea');
    await expect(welcome).toBeVisible();

    // Click 2: accepting replaces the draft and sends. The bug guard is that the
    // VISIBLE prompt has to give way to the seeded sentence: the normal compose
    // sync skips a focused, non-empty textarea to protect in-flight typing, so
    // applySuggestion force-syncs it (requestPromptOverrideSync).
    await pressInterview(page, start);
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await dialog.getByRole('button', { name: 'Replace' }).click();
    await expect(dialog).toHaveCount(0);
    await assertUserMessagesVisible(page, ['Help me get the most out of Lucidos']);
  });

  test('its text column lands on the composer box, both edges', async ({ page }) => {
    // The welcome docks directly above the composer on the compose-empty view,
    // so the two share one content edge. `.welcome-message` owns that box for
    // both variants, which is what this measures: it wears `.response-content`
    // (`max-width: 100%`), and that beats the `.thread-content > *` cap on
    // source order, so without its own cap the surface runs the full pane.
    // Only the real cascade at a real pane width resolves that, which is why
    // the unit guard in styles/__tests__/welcome-content-box.test.ts is not
    // enough on its own.
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    const edges = await page.evaluate(() => {
      const visible = <T extends Element>(els: NodeListOf<T>): T | null => {
        for (let i = els.length - 1; i >= 0; i--) {
          if (els[i].getBoundingClientRect().width > 0) return els[i];
        }
        return null;
      };
      const w = visible(document.querySelectorAll<HTMLElement>('.welcome-message'));
      const box = visible(document.querySelectorAll<HTMLElement>('.prompt-box'));
      if (!w || !box) return null;
      const wRect = w.getBoundingClientRect();
      const wStyle = getComputedStyle(w);
      const boxRect = box.getBoundingClientRect();
      return {
        // The welcome's CONTENT edges: its own inset is --turn-body-inset, the
        // same one `.prompt-input-container` puts around `.prompt-box`.
        left: wRect.left + parseFloat(wStyle.paddingLeft),
        right: wRect.right - parseFloat(wStyle.paddingRight),
        boxLeft: boxRect.left,
        boxRight: boxRect.right,
      };
    });
    expect(edges, 'welcome / prompt box not laid out').not.toBeNull();
    const e = edges!;

    expect(e.left, `welcome left=${e.left} vs composer left=${e.boxLeft}`)
      .toBeCloseTo(e.boxLeft, 0);
    expect(e.right, `welcome right=${e.right} vs composer right=${e.boxRight}`)
      .toBeCloseTo(e.boxRight, 0);
  });

  test('"Don\'t show this again" dismisses it and it stays dismissed after reload', async ({ page }) => {
    await navigateToApp(page);

    // The dismiss control lives on the setup-interview variant (a provider is
    // configured under the e2e mock model, so this is the rendered variant).
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
