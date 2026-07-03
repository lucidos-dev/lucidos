import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

/** Regression guard: on desktop the floating up-chevron (.scroll-to-top) sits
 *  OVER the centered transcript (it was pulled in from the right gutter in fix
 *  956b47519). A deep-link / turn-nav landing scrolls a `.chat-exchange` to its
 *  own `scroll-margin-top` below the container top — so that clearance MUST drop
 *  the landed element (and its .nav-focus-stuck outline, which reaches
 *  --nav-focus-reach beyond the box) below the chevron's footprint, or the
 *  element tucks right under the chevron with no room.
 *
 *  This asserts the CSS invariant directly from computed values rather than a
 *  flaky pixel-scroll, so it can't drift: shrink the clearance, or move/enlarge
 *  the chevron without tracking it, and this fails. Desktop-only (the chevron
 *  overlaps content only at the desktop layout; mobile lands far below a header
 *  stack that already dwarfs the chevron). */
test.describe('Deep-link landing clears the scroll-to-top chevron (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('.chat-exchange scroll-margin-top clears the up-chevron footprint + focus-marker reach', async ({ page }) => {
    test.skip(isMobileViewport(page), 'the chevron overlaps transcript content only on desktop');

    await navigateToApp(page);
    // One turn is enough — we read layout metrics, not scroll position.
    await sendMessage(page, 'Say OK.');
    await waitForResponse(page);

    const metrics = await page.evaluate(() => {
      const exchange = document.querySelector('.chat-exchange');
      const chevron = document.querySelector('.scroll-to-top');
      if (!exchange || !chevron) return null;
      const px = (v: string) => parseFloat(v) || 0;
      // Custom properties don't resolve to px via getComputedStyle, so probe one.
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
    // The landed element's own box must reach at or below the chevron's bottom.
    expect(
      scrollMarginTop,
      `landed element (scroll-margin-top=${scrollMarginTop}px) tucks under the chevron (bottom=${chevronBottom}px)`,
    ).toBeGreaterThanOrEqual(chevronBottom);
    // ...and its focus-marker outline (reaches navFocusReach above the box) must clear it too.
    expect(
      scrollMarginTop,
      `focus-marker outline lands behind the chevron (footprint=${chevronBottom + navFocusReach}px)`,
    ).toBeGreaterThanOrEqual(chevronBottom + navFocusReach);
  });
});
