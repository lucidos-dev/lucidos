/**
 * The Lucidos menu on DESKTOP, opened from the mark in the thread pane's header.
 *
 * This exists because of what the menu replaced. The `[Lucidos • workspace]`
 * label used to open the workspace switcher, an anchored popover that carried
 * the peer list, the Refresh glyph and the Restart confirm; retiring the
 * switcher left that label as the only opener the viewport had, and then the
 * label itself became the mark. The regression it invites is that desktop
 * silently loses the route to Refresh, Restart and Workspaces entirely. Mobile
 * would not notice: it reaches the same menu from its own marks.
 *
 * Desktop's menu is deliberately SHORTER than mobile's. New thread, Search
 * everywhere and Setup interview are icons in this very header row, so repeating
 * them below would make the menu a second copy of the row it hangs from. What is
 * left is the pair desktop can reach nowhere else. Both halves are asserted:
 * every row the menu owes is present, and every row the row above already
 * carries is absent WITH its icon proven present, so a trim can never be the
 * reason an action became unreachable.
 *
 * Desktop-only by viewport: the mobile layout has no `.desktop-header` at all.
 */
import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

/** An action the menu deliberately does not carry is still reachable in the row
 *  itself: as its own icon, or inside the row's ⋯ overflow menu once the pane is
 *  too narrow to show it (see ThreadHeaderActions). Both renderings carry the
 *  action's class, which is what makes one check cover both. */
async function assertReachableInRow(page: Page, actionClass: string): Promise<void> {
  if (await page.locator(`.desktop-header ${actionClass}`).count() > 0) return;
  const more = page.locator('.desktop-header .thread-header-more');
  await expect(more, `${actionClass} is neither in the row nor folded into a ⋯ menu`).toHaveCount(1);
  await more.click();
  await expect(page.locator(`.thread-overflow-item${actionClass}`)).toHaveCount(1);
  await page.keyboard.press('Escape');
}

test.describe('Lucidos menu from the desktop mark', () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('opens with every control it owes, names the workspace, and closes on a second click', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');

    // The mark is the connection light as well as the opener, and it says so in
    // more than colour: the state is in the accessible name.
    await expect(toggle).toHaveAttribute('data-conn', /connected|connecting|disconnected/);
    await expect(toggle).toHaveAttribute('aria-label', /^Lucidos menu · connect/);

    // The actions the menu drops are here in the row, which is the whole
    // premise of dropping them.
    await assertReachableInRow(page, '.brand-compose-btn');
    await assertReachableInRow(page, '.search-everywhere-btn');

    await toggle.click();
    await expect(menu).toBeVisible();

    // The two unconditional rows. Refresh is the one the retired switcher owned,
    // so it is the one most likely to go missing in a move like this.
    for (const label of ['Workspaces', 'Refresh']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(1);
    }

    // ...and the three the row carries are NOT repeated here. The setup
    // interview (labelled "Setup guide" on the row) is gated on a configured
    // LLM provider, which is not a property of the workspace this suite boots,
    // so its own control is not asserted above; the row must be absent either
    // way, which is what makes it safe to check.
    for (const label of ['New thread', 'Search everywhere', 'Setup guide']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(0);
    }

    // Workspaces names the workspace you are in, whether or not it can link
    // anywhere. It links to the gateway picker when there is one, and this
    // suite runs against the VITE dev server, whose shell the engine never
    // stamped with a `<base href>` or a gateway-port meta, so here there is
    // not: the row renders static instead. Asserting the href would be
    // asserting a property of the harness, not of the app, so the href
    // derivation is left to `computeGatewayPickerHref`'s own unit tests and
    // this pins the half that must hold in EITHER context.
    const workspaces = menu.locator('.brand-menu-item', { hasText: 'Workspaces' });
    await expect(workspaces.locator('.brand-menu-value-name')).toHaveCount(1);
    await expect(workspaces.locator('.brand-menu-value-name')).not.toBeEmpty();

    // That row is the menu's one <a>, so it is the one that inherits the user
    // agent's link underline and reads as a different kind of row.
    await expect(workspaces).toHaveCSS('text-decoration-line', 'none');

    // The panel belongs to the thread pane's mark, so it is centred on that
    // pane, not on the window: a window-centred panel hangs over the content
    // pane and drifts further off the mark the narrower the split gets.
    // Measured against the header's own brand region, which spans exactly the
    // pane the mark sits in (drawer edge to split divider), so the assertion
    // holds with the thread drawer open as well as closed.
    const centres = await page.evaluate(() => {
      const mid = (el: Element): number => {
        const r = el.getBoundingClientRect();
        return (r.left + r.right) / 2;
      };
      return {
        panel: mid(document.querySelector('.brand-menu')!),
        pane: mid(document.querySelector('.desktop-header .pane-header-brand')!),
        window: window.innerWidth / 2,
      };
    });
    expect(centres.panel).toBeCloseTo(centres.pane, 0);
    expect(
      Math.abs(centres.panel - centres.window),
      'the panel landed on the window axis, so pane centring is not in force',
    ).toBeGreaterThan(1);

    // The dim is up, and it is click-through so the outside-click contract can
    // resolve its target against the app underneath.
    await expect(page.locator('.brand-menu-scrim')).toHaveCount(1);

    // Re-activating the toggle closes via its own handler. With the anchor
    // exemption broken this reopens instead, which is the documented bug shape.
    await toggle.click();
    await expect(menu).toHaveCount(0);
    await expect(page.locator('.brand-menu-scrim')).toHaveCount(0);
  });

  test('opening and closing the menu does not move the mark or the workspace name', async ({ page }) => {
    // Reported against the packaged macOS build: clicking the mark "makes icon
    // and ws name jump a little". Nothing about the menu is supposed to touch
    // the row it hangs from, and both halves of the report move TOGETHER, which
    // means their shared box re-laid out rather than the mark reacting on its
    // own. So this pins the property directly.
    //
    // Run twice, the second time with what `titlebar_inset_script` stamps on
    // that build: no CSS keys off Tauri itself, so the attribute plus the inset
    // IS the packaged geometry (a 28px band above a correspondingly shorter
    // header, with the leading controls raised into it).
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    const name = page.locator('.desktop-header .workspace-name-label');
    await expect(name).toBeVisible();

    const measure = () => page.evaluate(() => {
      const box = (sel: string) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.left, y: r.top, w: r.width, h: r.height };
      };
      return {
        mark: box('.desktop-header [data-role="brand-menu-toggle"]'),
        name: box('.desktop-header .workspace-name-label'),
      };
    });

    for (const overlay of [false, true]) {
      await page.evaluate((on) => {
        const root = document.documentElement;
        if (on) {
          root.setAttribute('data-titlebar-overlay', '');
          root.style.setProperty('--titlebar-inset', '28px');
        } else {
          root.removeAttribute('data-titlebar-overlay');
          root.style.removeProperty('--titlebar-inset');
        }
      }, overlay);
      // Past the header's own var(--duration-slow) geometry transitions.
      await page.waitForTimeout(500);

      const build = overlay ? 'packaged macOS' : 'web';
      const before = await measure();
      await toggle.click();
      await expect(menu).toBeVisible();
      const open = await measure();
      await toggle.click();
      await expect(menu).toHaveCount(0);
      const after = await measure();

      for (const part of ['mark', 'name'] as const) {
        for (const axis of ['x', 'y', 'w', 'h'] as const) {
          // Sub-pixel layout rounding is unavoidable and invisible; a jump the
          // user can see is not.
          expect(
            Math.abs(open[part]![axis] - before[part]![axis]),
            `${build}: the ${part}'s ${axis} moved while the menu was open`,
          ).toBeLessThan(0.5);
          expect(
            Math.abs(after[part]![axis] - before[part]![axis]),
            `${build}: the ${part}'s ${axis} did not return after closing`,
          ).toBeLessThan(0.5);
        }
      }
    }
  });
});
