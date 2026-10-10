import { test, expect } from './fixtures';
import { assertHealthy, isMobileViewport, navigateToApp, waitForVisibleInput } from './helpers';

/** Tapping the composer's image button leaves the keyboard up.
 *
 *  Reported from an iPhone: the keyboard dropped as the photo picker opened.
 *  The phone row draws the plain button, whose tap moved focus off the prompt.
 *  The wide row's popover toggle already ran on `touchend` and kept it.
 *
 *  An emulator shows no keyboard, so this pins what decides it: the prompt
 *  still holds focus after the tap, and the tap still opened the picker. */
test.describe('the composer image button keeps the keyboard up', () => {
  // iPhone 15 Pro portrait points, the device the report came from.
  test.use({ viewport: { width: 393, height: 852 } });

  test('a touch tap opens the picker and leaves the prompt focused', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the phone row is a mobile-only rendering');
    // Chromium injects real touch input over CDP. WebKit exposes no equivalent
    // on a mobile context.
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    await navigateToApp(page);
    const input = await waitForVisibleInput(page);
    await input.focus();
    await input.fill('a caption for the photo');
    await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

    const attach = page.locator('.prompt-actions-row button[data-role="attach-image"]:visible').first();
    await expect(attach).toBeVisible({ timeout: 10_000 });
    const box = await attach.boundingBox();
    expect(box, 'the image button never rendered').not.toBeNull();

    const chooser = page.waitForEvent('filechooser', { timeout: 10_000 });
    const x = Math.round(box!.x + box!.width / 2);
    const y = Math.round(box!.y + box!.height / 2);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

    await chooser;
    await expect(input, 'the tap moved focus off the prompt, which drops the keyboard').toBeFocused();
    await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');
  });
});
