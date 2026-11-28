import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

test.describe('Header brand badge', () => {
  test('rides the mark it belongs to, out of flow and click-through', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);

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
      const cs = getComputedStyle(probe);
      const position = cs.position;
      const pointerEvents = cs.pointerEvents;
      probe.remove();
      if (p.height === 0) return null;

      const over = (a: DOMRect, b: DOMRect): number =>
        Math.max(0, Math.min(a.right, b.right) - Math.max(a.left, b.left)) *
        Math.max(0, Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top));

      return {
        position,
        pointerEvents,
        widthBefore,
        widthAfter: host.getBoundingClientRect().width,
        onMarkFraction: p.width * p.height > 0 ? over(p, m) / (p.width * p.height) : 0,
        above: (p.top + p.bottom) / 2 < (m.top + m.bottom) / 2,
        trailing: (p.left + p.right) / 2 > (m.left + m.right) / 2,
      };
    });
    const m = await handle.jsonValue();

    expect(m.position, 'the badge must not take a flex slot beside the mark').toBe('absolute');
    expect(
      m.widthAfter - m.widthBefore,
      `the badge widened the mark's slot by ${m.widthAfter - m.widthBefore}px, which moves the mark off its axis`,
    ).toBeLessThan(0.5);

    expect(
      m.onMarkFraction,
      `only ${Math.round(m.onMarkFraction * 100)}% of the badge is on the mark; it is sitting beside it`,
    ).toBeGreaterThan(0.5);
    expect(m.above, 'the badge must take the mark\'s TOP corner').toBe(true);
    expect(m.trailing, 'the badge must take the mark\'s trailing corner').toBe(true);

    expect(m.pointerEvents, 'the badge with no handler must not eat taps on the mark')
      .toBe('none');
  });

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
