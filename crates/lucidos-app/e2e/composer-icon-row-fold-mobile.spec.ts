import { test, expect } from './fixtures';
import {
  COMPOSER_FOLD_TRIGGER,
  assertHealthy,
  closeComposerFold,
  composerControl,
  followToggle,
  navigateToApp,
  setVoiceEnabled,
  waitForVisibleInput,
} from './helpers';
import { ensureHomeThread } from './db-helpers';

/** The composer row folds from its left end into a ⋯ menu when it runs short
 *  of room.
 *
 *  A browser test, because the decision is a MEASUREMENT. The math is unit
 *  tested in `src/hooks/usePromptActionCollapse.test.ts`, and the wiring by
 *  source scans beside the component. Neither can see what this can: whether a
 *  real row at a real width actually folds, and whether a folded control still
 *  works when the user reaches it through the menu.
 *
 *  It also pins the order: the call toggle stands before attach image, so it
 *  folds before it. Clear draft stands last among the icons. The follow toggle
 *  floats over the composer, so it stays put and never reaches the ⋯.
 *
 *  Voice is ON for this file, which is the reported case: a call toggle is one
 *  more box, and the row was reported crowded with one standing. It goes off
 *  again in `afterAll`, per the helper's own rule.
 *
 *  Plan: `docs/plans/2026-09-19-composer-icon-row-overflow-menu.md`. */

/** Widths to try, in order, until the row folds.
 *
 *  A ladder rather than one number. The fold width follows the root font size
 *  and whatever the thread is showing. So a constant here would fail on a
 *  retuned icon box rather than on anything this spec is about. It still fails
 *  loudly if nothing folds at all. */
const WIDTH_LADDER = [390, 340, 300, 270, 240];

// Mobile only. The fold follows room rather than a breakpoint, and a desktop
// pane at its own minimum still holds every glyph. Shrinking a desktop window
// would cross into the mobile layout and remount the lot.
test.describe('the composer row folds its middle when it runs out of room', () => {
  test.beforeAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, true);
    await page.close();
  });

  test.afterAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, false);
    await page.close();
  });

  test('folds the leading toggles before attach image, and still clears the draft', async ({ page }) => {
    const restore = page.viewportSize();
    // The call toggle stands only on the home thread (ADR 0362).
    const home = ensureHomeThread();
    await navigateToApp(page);
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, home);
    const input = await waitForVisibleInput(page);
    // A draft, so the clear action joins the foldable list and there is
    // something for the folded one to act on.
    await input.fill('a draft the folded menu will clear');

    const more = page.locator(COMPOSER_FOLD_TRIGGER);
    const attach = page.locator('.prompt-actions-row [data-role="attach-image"]:visible');
    const follow = followToggle(page);
    let foldedAt = 0;
    for (const width of WIDTH_LADDER) {
      await page.setViewportSize({ width, height: restore?.height ?? 812 });
      // The fold follows a resize measurement, so it lands a frame later.
      const folded = await expect.poll(async () => (await more.count()) > 0 && (await attach.count()) === 0, { timeout: 1_000 })
        .toBe(true).then(() => true, () => false);
      if (folded) { foldedAt = width; break; }
    }
    expect(foldedAt, `the row never folded, down to ${WIDTH_LADDER.at(-1)}px`).toBeGreaterThan(0);

    // The fold takes a prefix: once attach leaves, the leading toggle has left
    // too. The action slot stays on the right, and the follow toggle stands.
    await expect(attach).toHaveCount(0);
    await expect(page.locator('.prompt-actions-row .control-menu:visible')).toHaveCount(0);
    await expect(await composerControl(page, '[data-role="call-toggle"]')).toHaveAttribute('role', 'menuitemcheckbox');
    await expect(page.locator('.thread-overflow-menu [data-role="follow-live-edge"]')).toHaveCount(0);
    await closeComposerFold(page);
    await expect(follow).toBeVisible();
    await expect(page.locator('.prompt-actions-row [data-role="follow-live-edge"]')).toHaveCount(0);
    await expect(page.locator('.send-cancel-morph:visible')).toHaveCount(1);

    // The same selector reaches a folded action behind the ⋯, which is the
    // whole point of `data-role` travelling with it.
    const folded = await composerControl(page, '[data-role="attach-image"]');
    await expect(folded).toHaveAttribute('role', 'menuitem');
    await page.keyboard.press('Escape');

    // And a folded action still acts. Clear is last in the list, so it may be
    // standing or folded at this width; the helper takes either.
    const clear = await composerControl(page, 'button.prompt-clear');
    await clear.click();
    await expect.poll(() => input.inputValue()).toBe('');

    // Widening hands the icons back, when the row folded only below the start
    // width. A phone row can fold at the start width already, since the rung
    // pill outlasts the icons. The ⋯ may stay for the same reason.
    // The ladder opens above a phone's own width, so the row may have folded at
    // a width the viewport never held. Widen to at least that one.
    const startWidth = Math.max(restore?.width ?? 390, foldedAt);
    await page.setViewportSize({ width: startWidth, height: restore?.height ?? 812 });
    if (foldedAt < startWidth) await expect.poll(() => attach.count()).toBe(1);
    await expect(follow).toBeVisible();
    // The rung pill stands right of the icons, and the fold takes a prefix.
    await expect(page.locator('.prompt-actions-row .rung-pill:visible')).toHaveCount(1);

    await assertHealthy(page);
  });
});
