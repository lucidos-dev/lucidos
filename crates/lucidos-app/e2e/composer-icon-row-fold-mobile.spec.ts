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


const WIDTH_LADDER = [390, 340, 300, 270, 240];

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
    const home = ensureHomeThread();
    await navigateToApp(page);
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, home);
    const input = await waitForVisibleInput(page);
    await input.fill('a draft the folded menu will clear');

    const more = page.locator(COMPOSER_FOLD_TRIGGER);
    const attach = page.locator('.prompt-actions-row [data-role="attach-image"]:visible');
    const follow = followToggle(page);
    let foldedAt = 0;
    for (const width of WIDTH_LADDER) {
      await page.setViewportSize({ width, height: restore?.height ?? 812 });
      const folded = await expect.poll(async () => (await more.count()) > 0 && (await attach.count()) === 0, { timeout: 1_000 })
        .toBe(true).then(() => true, () => false);
      if (folded) { foldedAt = width; break; }
    }
    expect(foldedAt, `the row never folded, down to ${WIDTH_LADDER.at(-1)}px`).toBeGreaterThan(0);

    await expect(attach).toHaveCount(0);
    await expect(page.locator('.prompt-actions-row .control-menu:visible')).toHaveCount(0);
    await expect(await composerControl(page, '[data-role="call-toggle"]')).toHaveAttribute('role', 'menuitemcheckbox');
    await expect(page.locator('.thread-overflow-menu [data-role="follow-live-edge"]')).toHaveCount(0);
    await closeComposerFold(page);
    await expect(follow).toBeVisible();
    await expect(page.locator('.prompt-actions-row [data-role="follow-live-edge"]')).toHaveCount(0);
    await expect(page.locator('.send-cancel-morph:visible')).toHaveCount(1);

    const folded = await composerControl(page, '[data-role="attach-image"]');
    await expect(folded).toHaveAttribute('role', 'menuitem');
    await page.keyboard.press('Escape');

    const clear = await composerControl(page, 'button.prompt-clear');
    await clear.click();
    await expect.poll(() => input.inputValue()).toBe('');

    const startWidth = Math.max(restore?.width ?? 390, foldedAt);
    await page.setViewportSize({ width: startWidth, height: restore?.height ?? 812 });
    if (foldedAt < startWidth) await expect.poll(() => attach.count()).toBe(1);
    await expect(follow).toBeVisible();
    await expect(page.locator('.prompt-actions-row .rung-pill:visible')).toHaveCount(1);

    await assertHealthy(page);
  });
});
