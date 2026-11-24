import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, waitForVisibleInput } from './helpers';
import { clearAllThreads } from './db-helpers';

test.describe('Composer resting height', () => {
  test('an empty composer rests inside a line', async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForVisibleInput(page);

    const box = await page.evaluate(() => {
      const el = Array.from(document.querySelectorAll<HTMLTextAreaElement>('.prompt-row .prompt-textarea'))
        .find((t) => t.getBoundingClientRect().width > 0);
      if (!el) return null;
      const cs = getComputedStyle(el);
      const padding = (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
      return {
        content: el.clientHeight - padding,
        line: parseFloat(cs.lineHeight),
        padding,
        value: el.value,
        width: window.innerWidth,
      };
    });

    expect(box, 'no composer textarea was on screen').not.toBeNull();
    expect(box!.value, 'the box must be empty for this to be the RESTING height').toBe('');
    expect(Number.isFinite(box!.line), 'line-height did not resolve to a length').toBe(true);

    const arithmetic = `its content box is ${box!.content.toFixed(1)}px, against a `
      + `${box!.line.toFixed(1)}px line and ${box!.padding.toFixed(1)}px of padding, `
      + `at ${box!.width}px wide`;
    expect(box!.content, `the empty composer rests two lines tall or more: ${arithmetic}`)
      .toBeLessThan(box!.line * 2);
    expect(box!.content, `the empty composer has no room for its own line: ${arithmetic}`)
      .toBeGreaterThanOrEqual(box!.line - 1);
  });
});
