/**
 * The header brand badge (the background-activity spinner / the update-ready
 * "!") rides above the "Lucidos" baseline like a superscript, and it gets there
 * through a bottom MARGIN, never a paint-time `position: relative; top:` offset.
 * Two separate bugs came from that distinction, one per viewport, and both are
 * pinned here.
 *
 * DESKTOP: the badge's host, `.pane-header-brand-label`, pairs
 * `overflow-x: clip` (its horizontal non-overlap guarantee at narrow pane
 * widths) with `overflow-y: visible`. The packaged macOS webview does not honour
 * that pairing: it clips both axes as soon as one of them is `clip`. A lifted
 * `top` put the badge partly outside the box, so it rendered whole in Chromium
 * and in Playwright's WebKit but reached the Tauri app with its top sliced flat
 * off, which is invisible to every engine this suite can drive. What IS
 * engine-independent is that the badge no longer overflows the box at all,
 * because `align-items: center` centers a flex item's MARGIN box and leaves the
 * border box inside the line.
 *
 * MOBILE: `mobile.css` re-corners every `.badge` for the smaller mobile icon
 * buttons, and being equal-specificity and later in import order it used to win
 * the brand badge's `top` too. Once the lift became a margin, that leftover
 * paint-time offset stacked on it and the badge climbed half a step. The `top`
 * reset is doubled up (`.badge.brand-badge`) to win on specificity instead.
 *
 * The badge only renders while background activity is in flight, so a probe
 * carrying its classes is spliced into the live header and read off the same
 * cascade a real badge would get, then removed before anything can paint.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

test.describe('Header brand badge', () => {
  test('rides high on a margin, not on an offset that escapes its box', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);

    // One polled block: the header renders a copy per layout, so it waits for
    // the one with real width and measures against that same laid-out frame.
    const handle = await page.waitForFunction(() => {
      const label = [...document.querySelectorAll<HTMLElement>('.pane-header-brand-label')]
        .reverse()
        .find((el) => el.getBoundingClientRect().width > 0);
      if (!label) return null;

      const probe = document.createElement('span');
      probe.className = 'badge brand-badge';
      probe.textContent = '!';
      label.appendChild(probe);
      const p = probe.getBoundingClientRect();
      const l = label.getBoundingClientRect();
      const cs = getComputedStyle(probe);
      // `auto` on a relatively positioned box is 0; engines disagree on which of
      // the two the resolved value reports, so read both as the same thing.
      const topOffset = cs.top === 'auto' ? 0 : parseFloat(cs.top);
      const marginBottom = parseFloat(cs.marginBottom);
      probe.remove();
      if (p.height === 0) return null;

      return {
        probeTop: p.top,
        labelTop: l.top,
        topOffset,
        marginBottom,
        // Positive = the badge sits above the label's centre line, which is the
        // superscript lift itself.
        lift: (l.top + l.bottom) / 2 - (p.top + p.bottom) / 2,
      };
    });
    const m = await handle.jsonValue();

    // The mobile regression: a leftover paint-time offset stacking on the margin.
    expect(
      m.topOffset,
      `brand badge carries a ${m.topOffset}px paint-time top offset on top of its margin lift`,
    ).toBe(0);

    // The desktop regression: a paint-time lift put the badge's top ABOVE the
    // label's, where an engine that clips both axes cropped it flat.
    expect(
      m.probeTop - m.labelTop,
      `badge top ${m.probeTop} sits ${m.labelTop - m.probeTop}px above its clipping host's top ${m.labelTop}`,
    ).toBeGreaterThanOrEqual(-0.5);

    // ...and it is still a superscript, lifted by exactly half its bottom
    // margin, which is what centring the margin box buys.
    expect(m.lift, 'badge no longer rides above the title baseline').toBeGreaterThan(0);
    expect(m.lift, 'badge lift no longer comes from its bottom margin').toBeCloseTo(
      m.marginBottom / 2,
      1,
    );
  });
});
