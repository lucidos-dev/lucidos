import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy } from './helpers';

test.describe('Deep-link landing clears the scroll-to-top chevron (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('.chat-exchange scroll-margin-top clears the up-chevron footprint + focus-marker reach', async ({ page }) => {

    await navigateToApp(page);
    await sendMessage(page, 'Say OK.');
    await waitForResponse(page);

    const metrics = await page.evaluate(() => {
      const exchange = document.querySelector('.chat-exchange');
      const chevron = document.querySelector('.scroll-to-top');
      if (!exchange || !chevron) return null;
      const px = (v: string) => parseFloat(v) || 0;
      const pxOfVar = (name: string) => {
        const probe = document.createElement('div');
        probe.style.position = 'absolute';
        probe.style.visibility = 'hidden';
        probe.style.height = `var(${name})`;
        document.body.appendChild(probe);
        const h = probe.getBoundingClientRect().height;
        probe.remove();
        return h;
      };
      return {
        scrollMarginTop: px(getComputedStyle(exchange).scrollMarginTop),
        chevronTop: px(getComputedStyle(chevron).top),
        chevronHeight: chevron.getBoundingClientRect().height,
        navFocusReach: pxOfVar('--nav-focus-reach'),
      };
    });

    expect(metrics, 'expected a .chat-exchange and a .scroll-to-top in the DOM').not.toBeNull();
    const { scrollMarginTop, chevronTop, chevronHeight, navFocusReach } = metrics!;

    const chevronBottom = chevronTop + chevronHeight;
    expect(
      scrollMarginTop,
      `landed element (scroll-margin-top=${scrollMarginTop}px) tucks under the chevron (bottom=${chevronBottom}px)`,
    ).toBeGreaterThanOrEqual(chevronBottom);
    expect(
      scrollMarginTop,
      `focus-marker highlight lands behind the chevron (footprint=${chevronBottom + navFocusReach}px)`,
    ).toBeGreaterThanOrEqual(chevronBottom + navFocusReach);
  });
});
