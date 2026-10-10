/**
 * Mobile: scrolling an element into view never moves the pane strip sideways.
 *
 * Regression. `.mobile-swipe-container` places its panes with a transform and
 * hides the rest. With `overflow: hidden` it was still a scroll container, so a
 * scroll into view also scrolled it sideways and pushed every pane off its
 * place. A JS reset put it back, but only on the next `scroll` event. A tap in
 * between landed on the wrong element.
 *
 * Playwright scrolls a target into view before it taps it. So the slider spec's
 * tap on the UI-scale button once hit the left edge-swipe zone instead, and the
 * scale panel never opened.
 *
 * The read happens in the same task as the scroll, before any reset can run, so
 * a strip that can scroll fails here every time.
 */
import { test, expect } from './fixtures';
import { assertHealthy, ensureMobileView, navigateToApp, openSettingsView, waitForPaneAtRest } from './helpers';

const UI_SCALE_BUTTON = '[data-search-anchor="appearance:ui-scale"] .settings-option';

test.describe('Mobile pane strip', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a scroll into view moves no pane sideways', async ({ page }) => {
    await navigateToApp(page);
    await openSettingsView(page, 'appearance');
    await ensureMobileView(page, 'content');
    await expect(page.locator(UI_SCALE_BUTTON)).toBeVisible();
    await waitForPaneAtRest(page);

    const moved = await page.evaluate((selector) => {
      const strip = document.querySelector('.mobile-swipe-container')!;
      const button = document.querySelector(selector)!;
      const left = () => button.getBoundingClientRect().left;
      const before = left();
      // `inline: 'start'` asks every scrollable ancestor to move the button to
      // the left edge, so a strip that can scroll does.
      button.scrollIntoView({ block: 'center', inline: 'start' });
      return { scrollLeft: strip.scrollLeft, shift: left() - before };
    }, UI_SCALE_BUTTON);

    expect(moved).toEqual({ scrollLeft: 0, shift: 0 });
  });
});
