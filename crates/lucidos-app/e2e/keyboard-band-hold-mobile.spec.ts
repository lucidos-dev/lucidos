import { test, expect } from './fixtures';
import { addTriggerCard, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';

/** The band hold (docs/glossary.md § Focused-field reveal).
 *
 *  Focusing a field reserves `--keyboard-band` as padding at the foot of the
 *  pane's scroller. Pressing a button blurs the field and drops the band. A
 *  reader scrolled into that padding had their scrollTop clamped by the drop.
 *  The button slid away between press and release, and the click missed it.
 *  A Save on the trigger form did nothing and said nothing.
 *
 *  See docs/plans/2026-09-24-the-keyboard-band-holds-the-scroll-anchor.md. */

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
    // A handle, not a locator: the form this one is found by closes below.
    const scroller = await page.locator('.mobile-swipe-pane .content-pane-body', { has: form }).elementHandle();
    if (!scroller) throw new Error('the form has no pane scroller');

    // Focus reserves the band; scroll to the end, which is inside it.
    await form.locator('.prompt-textarea').fill('Send me a hello every morning');
    await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight; });
    const bandPx = await scroller.evaluate((el) => parseFloat(getComputedStyle(el).paddingBottom));
    expect(bandPx, 'the focus reserved no band, so this spec proves nothing').toBeGreaterThan(0);

    const cancel = form.locator('.btn-cancel');
    const before = await cancel.boundingBox();
    if (!before) throw new Error('Cancel has no box');
    await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
    await page.mouse.down();
    // The press has blurred the field and dropped the band by now.
    await expect(form.locator('.prompt-textarea')).not.toBeFocused();
    const during = await cancel.boundingBox();
    expect(during?.y).toBe(before.y);
    await page.mouse.up();

    // The click reached Cancel, so the form closed.
    await expect(form).toHaveCount(0, { timeout: 5_000 });

    // The form it anchored is gone, so the hold let go with it: no blank band
    // is left under the reader, and they did not have to scroll for it.
    await expect.poll(() => scroller.evaluate((el) => (el as HTMLElement).style.getPropertyValue('--keyboard-band-hold')))
      .toBe('');
  });
});
