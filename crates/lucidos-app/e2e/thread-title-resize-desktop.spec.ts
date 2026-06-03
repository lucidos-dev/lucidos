import { test, expect } from '@playwright/test';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  clickVisibleElement,
  waitForThreadTitle,
  waitForTitleInput,
  getDesktopTitleHeight,
} from './helpers';

// Desktop-only resize test — depends on `page.setViewportSize()` actually
// changing the rendered layout, which mobile-emulated projects (mobile,
// mobile-webkit) ignore because they pin the iPhone viewport via
// `isMobile: true`. Lives in a `-desktop.spec.ts` file so playwright.config
// excludes it from those projects (`testIgnore: /-desktop\.spec\.ts$/`).
//
// The non-resize "Thread title editing — desktop" tests stay in
// thread-title-edit.spec.ts because they DO pass under mobile emulation.

test.describe('Thread title editing — desktop resize', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('display height re-fits when container widens after a wrapped narrow measurement', async ({ page }) => {
    // Bug: autoResizeTextarea() only re-runs on [title, editing] dep changes.
    // If the textarea was measured while the container was narrow (title wraps
    // to multiple lines), the inline style.height stays pinned to the wrapped
    // value when the container later widens — header balloons until the user
    // renames or reloads. A ResizeObserver on the textarea catches container
    // width changes (drawer toggle, divider drag, window resize) and re-fits.
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-resize-stuck');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    // Set a moderately long title — wraps at narrow widths, single-line at wide.
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);
    const longTitle = 'A thread title that wraps on narrow desktop but fits wide';
    await input.fill(longTitle);
    await input.press('Enter');
    await page.waitForFunction((expected) => {
      const els = document.querySelectorAll('.thread-title-display');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && ((el as HTMLTextAreaElement).value ?? '').trim() === expected;
      });
    }, longTitle, { timeout: 10_000 });

    // Capture the natural one-line height at wide width as the baseline.
    const baselineHeight = await getDesktopTitleHeight(page);
    expect(baselineHeight, 'baseline: title fits on one line at 1600px').toBeGreaterThan(0);

    // Narrow the viewport — stay above 768px so the desktop layout (and the
    // same .thread-view-header textarea element) remains visible.
    await page.setViewportSize({ width: 800, height: 800 });

    // Force autoResize to re-run at the narrow width via the editing→false
    // transition (the [title, editing] effect calls autoResizeTextarea). The
    // bug is otherwise latent until something else fires the effect.
    await clickVisibleElement(page, '.thread-title-display');
    await waitForTitleInput(page);
    await page.keyboard.press('Escape');
    await page.waitForFunction(() =>
      document.querySelectorAll('.thread-title-edit.is-editing').length === 0,
    );

    const narrowHeight = await getDesktopTitleHeight(page);
    // Sanity: at narrow desktop the long title wraps → notably taller than baseline.
    expect(narrowHeight, 'sanity: title wrapped at narrow desktop').toBeGreaterThan(baselineHeight + 10);

    // Widen the viewport. Without the fix, no autoResize trigger fires and
    // style.height stays pinned to the narrow-width measurement.
    await page.setViewportSize({ width: 1600, height: 800 });

    // Allow ResizeObserver to fire and the layout to settle. Polls for the
    // re-fit; falls through to the assertion if the timeout elapses.
    await page.waitForFunction((baseline) => {
      const el = document.querySelector('.thread-view-header .thread-title-display') as HTMLTextAreaElement | null;
      return el ? el.getBoundingClientRect().height <= baseline + 1 : false;
    }, baselineHeight, { timeout: 2000 }).catch(() => { /* fall through to assertion */ });

    const wideHeight = await getDesktopTitleHeight(page);
    expect(wideHeight, 'title should re-fit to baseline single-line height after widening')
      .toBeLessThanOrEqual(baselineHeight + 1);
  });
});
