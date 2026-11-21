/**
 * The header brand badge (the background-activity spinner, the update-ready
 * "!") rides the Lucidos mark's top-trailing corner, and does so on every
 * viewport: the desktop `[Lucidos • workspace]` label it used to superscript is
 * gone, so the icon convention is the only shape left.
 *
 * Out of flow is the load-bearing part, not the prettier part. The mark is
 * centred (in the mobile nav cluster, and in the desktop brand box), so an
 * in-flow badge widens its slot and slides the mark sideways for exactly as long
 * as an engine build or an update is pending: a mark that moves for reasons the
 * user cannot see.
 *
 * Overlaying it costs one thing back, which is the second half of the contract.
 * A positioned element is a hit target even with no listener. The badge is the
 * mark's SIBLING, not its ancestor, so a tap on it has nothing to bubble to.
 * So every badge state is a plain, click-through span. A tap anywhere on the
 * mark opens the Lucidos menu, whose activity group explains the badge.
 *
 * The badge only renders while something is in flight or waiting, so a probe
 * carrying its classes is spliced into the live header and read off the same
 * cascade a real badge would get, then removed.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

test.describe('Header brand badge', () => {
  test('rides the mark it belongs to, out of flow and click-through', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);

    // One polled block: the header renders a copy per layout, so it waits for
    // the one with real width and measures against that same laid-out frame.
    const handle = await page.waitForFunction(() => {
      const host = [...document.querySelectorAll<HTMLElement>('.brand-mark-slot')]
        .reverse()
        .find((el) => el.getBoundingClientRect().width > 0);
      if (!host) return null;
      const mark = host.querySelector<HTMLElement>('.brand-mark, .brand-mark-row');
      if (!mark) return null;

      const widthBefore = host.getBoundingClientRect().width;
      const probe = document.createElement('span');
      probe.className = 'badge brand-badge';
      probe.textContent = '!';
      host.appendChild(probe);
      const p = probe.getBoundingClientRect();
      const m = mark.getBoundingClientRect();
      // Read every computed value BEFORE the probe leaves the document: the
      // declaration is live, so a detached element answers "" to all of it.
      const cs = getComputedStyle(probe);
      const position = cs.position;
      const pointerEvents = cs.pointerEvents;
      probe.remove();
      if (p.height === 0) return null;

      // How much of the badge lands on the mark. "Beside it" scores 0, however
      // close beside; an offset corner nudge still scores most of the box. A
      // pair of edge comparisons would pass a badge floating anywhere above the
      // mark, which is the same "somewhere up there" the old margin lift was.
      const over = (a: DOMRect, b: DOMRect): number =>
        Math.max(0, Math.min(a.right, b.right) - Math.max(a.left, b.left)) *
        Math.max(0, Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top));

      return {
        position,
        pointerEvents,
        widthBefore,
        widthAfter: host.getBoundingClientRect().width,
        onMarkFraction: p.width * p.height > 0 ? over(p, m) / (p.width * p.height) : 0,
        // Which corner of the mark it took, read off the centres.
        above: (p.top + p.bottom) / 2 < (m.top + m.bottom) / 2,
        trailing: (p.left + p.right) / 2 > (m.left + m.right) / 2,
      };
    });
    const m = await handle.jsonValue();

    // Out of flow, so the centred mark cannot be pushed sideways by a badge
    // appearing and disappearing under it.
    expect(m.position, 'the badge must not take a flex slot beside the mark').toBe('absolute');
    expect(
      m.widthAfter - m.widthBefore,
      `the badge widened the mark's slot by ${m.widthAfter - m.widthBefore}px, which moves the mark off its axis`,
    ).toBeLessThan(0.5);

    // ...and it lands ON the mark, in its top-right corner.
    expect(
      m.onMarkFraction,
      `only ${Math.round(m.onMarkFraction * 100)}% of the badge is on the mark; it is sitting beside it`,
    ).toBeGreaterThan(0.5);
    expect(m.above, 'the badge must take the mark\'s TOP corner').toBe(true);
    expect(m.trailing, 'the badge must take the mark\'s trailing corner').toBe(true);

    // The probe carries the READY badge's classes, i.e. the plain span with no
    // handler. Overlaid on the mark it is a hit target that swallows taps on
    // that corner, and what is under it is the mark's BUTTON, a sibling, so the
    // tap has nothing to bubble to and the menu never opens.
    expect(m.pointerEvents, 'the badge with no handler must not eat taps on the mark')
      .toBe('none');
  });

  /** The mark always opens the menu, whatever its badge shows. */
  for (const [state, classes] of [
    ['busy', 'badge brand-badge'],
    ['pending', 'badge brand-badge brand-badge-dot'],
  ] as const) {
    test(`a tap on the ${state} badge opens the Lucidos menu`, async ({ page }) => {
      await assertHealthy(page);
      await navigateToApp(page);

      const box = await page.waitForFunction((cls: string) => {
        const host = [...document.querySelectorAll<HTMLElement>('.brand-mark-slot')]
          .reverse()
          .find((el) => el.getBoundingClientRect().width > 0);
        if (!host) return null;
        const probe = document.createElement('span');
        probe.className = cls;
        probe.dataset.role = 'badge-probe';
        host.appendChild(probe);
        const r = probe.getBoundingClientRect();
        if (r.height === 0) { probe.remove(); return null; }
        return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
      }, classes);
      const { x, y } = (await box.jsonValue())!;

      await page.mouse.click(x, y);
      await expect(page.locator('[role="menu"][aria-label="Lucidos menu"]')).toBeVisible();

      await page.evaluate(() => document.querySelector('[data-role="badge-probe"]')?.remove());
    });
  }
});
