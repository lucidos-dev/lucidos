import { test, expect } from './fixtures';
import { addTriggerCard, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';


test.describe('The keyboard band holds the scroll anchor', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a button below a focused field stays under the press and takes the click', async ({ page }) => {
    await navigateToApp(page);
    await openTriggersPanel(page);
    await addTriggerCard(page).click();
    const form = page.locator('.inline-form:visible').first();
    await expect(form).toBeVisible({ timeout: 10_000 });
    const scroller = await page.locator('.mobile-swipe-pane .content-pane-body', { has: form }).elementHandle();
    if (!scroller) throw new Error('the form has no pane scroller');

    await form.locator('.prompt-textarea').fill('Send me a hello every morning');
    await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight; });
    const bandPx = await scroller.evaluate((el) => parseFloat(getComputedStyle(el).paddingBottom));
    expect(bandPx, 'the focus reserved no band, so this spec proves nothing').toBeGreaterThan(0);

    const cancel = form.locator('.btn-cancel');
    const before = await cancel.boundingBox();
    if (!before) throw new Error('Cancel has no box');
    await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
    await page.mouse.down();
    await expect(form.locator('.prompt-textarea')).not.toBeFocused();
    const during = await cancel.boundingBox();
    expect(during?.y).toBe(before.y);
    await page.mouse.up();

    await expect(form).toHaveCount(0, { timeout: 5_000 });

    await expect.poll(() => scroller.evaluate((el) => (el as HTMLElement).style.getPropertyValue('--keyboard-band-hold')))
      .toBe('');
  });
});
