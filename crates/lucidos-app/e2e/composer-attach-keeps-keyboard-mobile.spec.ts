import { test, expect } from './fixtures';
import { assertHealthy, isMobileViewport, navigateToApp, waitForVisibleInput } from './helpers';

test.describe('the composer image button keeps the keyboard up', () => {
  test.use({ viewport: { width: 393, height: 852 } });

  test('a touch tap opens the picker and leaves the prompt focused', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the phone row is a mobile-only rendering');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    await navigateToApp(page);
    const input = await waitForVisibleInput(page);
    await input.focus();
    await input.fill('a caption for the photo');
    await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '75%'));

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
