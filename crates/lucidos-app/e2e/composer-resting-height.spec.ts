import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, waitForVisibleInput } from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * An empty composer rests inside a line, on every engine we ship to.
 *
 * It declares no floor of its own: the shared shell's is what it rests at
 * (`min-height: 2.75rem`, panels/previews.css, and `2.25rem` under the mobile
 * breakpoint in mobile.css). Both are a line plus the box's padding at the
 * type scale of their own breakpoint.
 *
 * A multi-line floor in chat/input-messages.css is a settled no (docs/adr/0221),
 * and this is what holds it: the box a follow-up goes in stays the size of the
 * text in it, and grows on typing rather than in advance.
 *
 * So the bound is the element's own computed line-height, never a px constant.
 * The three projects run at three different roots, and the mobile composer
 * sets its own font-size. One constant would be right for at most one of them.
 * The content box is compared, since `clientHeight` carries the padding, which
 * is the same comparison `coding-agent-question.spec.ts` makes of the box once
 * a question is answered.
 */
test.describe('Composer resting height', () => {
  test('an empty composer rests inside a line', async ({ page }) => {
    // Pristine, so the compose view opens on a box with no draft in it. An
    // earlier spec can leave a thread focused, and the height of its draft is
    // not the RESTING height this measures.
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

    // Two lines of content, so a floor of three cannot pass and the shell's own
    // slack over a single line does not fail. The placeholder is one line wide
    // on every project, so nothing legitimately wraps here.
    const arithmetic = `its content box is ${box!.content.toFixed(1)}px, against a `
      + `${box!.line.toFixed(1)}px line and ${box!.padding.toFixed(1)}px of padding, `
      + `at ${box!.width}px wide`;
    expect(box!.content, `the empty composer rests two lines tall or more: ${arithmetic}`)
      .toBeLessThan(box!.line * 2);
    expect(box!.content, `the empty composer has no room for its own line: ${arithmetic}`)
      .toBeGreaterThanOrEqual(box!.line - 1);
  });
});
