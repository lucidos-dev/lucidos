import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

/** The composer attach menu's File item must open the OS file chooser.
 *
 *  It didn't, from 2026-05-19 to 2026-08-13, on every client. The item calls
 *  `.click()` on the persistent hidden `<input type="file">`, which lives in
 *  `.prompt-box` rather than in the menu panel so the menu's re-render can't
 *  unmount it mid-tap. That nested synthetic click landed OUTSIDE the open
 *  overlay's panel, so the dismiss contract's synthetic-click fallback read it
 *  as an outside click and `preventDefault()`ed it, and showing the file
 *  chooser is the cancelable DEFAULT ACTION of a click on a file input. The
 *  tap therefore did nothing, with nothing logged and nothing shown.
 *
 *  Nothing in the unit suite can see that: `preventDefault()` on a mocked
 *  event is only a spy call, and the picker is what the user came for. So the
 *  assertion here is the `filechooser` event itself.
 *
 *  Desktop-only by filename: the narrow branch of `PromptInput` replaces the
 *  menu with a single Attach button that clicks the same input with no overlay
 *  open, which is why the mobile clients were never affected. */
test.describe('Composer attach menu (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('File opens the file chooser and closes the menu', async ({ page }) => {
    await navigateToApp(page);

    await page.locator('.image-attach-anchor button[aria-label="Attach image"]').click();
    const menu = page.locator('.image-attach-menu');
    await expect(menu).toHaveCount(1);

    // A real click, so the whole gesture runs: pointerdown and click inside the
    // panel, then the item's handler dispatching the nested click on the input.
    const [chooser] = await Promise.all([
      page.waitForEvent('filechooser'),
      menu.locator('button', { hasText: 'File' }).click(),
    ]);

    // It is the composer's own input: multiple images, not one arbitrary file.
    expect(chooser.isMultiple()).toBe(true);
    await expect(menu).toHaveCount(0);
  });
});
