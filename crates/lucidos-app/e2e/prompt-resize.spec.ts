import { test, expect } from './fixtures';
import { navigateToApp, waitForVisibleInput, assertHealthy } from './helpers';

test.describe('Prompt textarea resize', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('grows to fit content when long text is pasted into empty input', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    // Pasted via setting value + dispatching input to mimic the real paste
    // path that originally triggered the bug — locator.fill() also works,
    // but value+dispatch isolates the resize handler from focus side effects.
    const longText = 'Aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk llll mmmm nnnn oooo pppp qqqq rrrr ssss tttt uuuu vvvv wwww xxxx yyyy zzzz aaaa bbbb cccc dddd';
    await input.evaluate((el: HTMLTextAreaElement, text) => {
      el.focus();
      el.value = text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }, longText);

    // Wait for Preact to flush hasText so the layout has reached its final form.
    // Both desktop and mobile layout copies render simultaneously and each carries
    // its own Send button — `:visible` picks the one in the active layout.
    await expect(page.locator('button[aria-label="Send message"]:visible:not(.invisible)').first())
      .toBeVisible({ timeout: 2_000 });

    const dims = await input.evaluate((el: HTMLTextAreaElement) => ({
      clientHeight: el.clientHeight,
      scrollHeight: el.scrollHeight,
      overflow: el.style.overflowY,
    }));
    // Sized text stays well below the 40vh cap on every viewport this suite
    // runs at, so no overflow scrollbar should appear.
    expect(dims.clientHeight, 'all pasted text must be rendered, none clipped').toBeGreaterThanOrEqual(dims.scrollHeight);
    expect(dims.overflow).toBe('hidden');
  });
});
