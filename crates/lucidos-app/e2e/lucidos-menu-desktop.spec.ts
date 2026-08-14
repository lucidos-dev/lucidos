/**
 * The Lucidos menu on DESKTOP, opened from the mark in the thread pane's header.
 *
 * The regression it guards is desktop silently losing its only route to
 * Refresh, Restart and Workspaces. Mobile would not notice, reaching the same
 * menu from its own marks.
 *
 * Desktop's menu is deliberately SHORTER than mobile's: New thread, Search
 * everywhere and Setup interview are icons in this header row already. Both
 * halves are asserted, so a trim can never be why an action became unreachable.
 * Every row the menu owes is present, and every row the header carries is
 * absent WITH its icon proven present.
 *
 * The panel also answers for the mark. When the connection light recedes, the
 * menu is where the state is spelled out. The third test here is the only check
 * that a real drop reaches the real panel.
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

    // The two unconditional rows.
    for (const label of ['Workspaces', 'Refresh']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(1);
    }

    // ...and the three the row carries are NOT repeated here. The setup
    // interview (labelled "Setup guide" on the row) is gated on a configured LLM
    // provider, so its own control is not asserted above. Its menu row must be
    // absent either way, which is what makes this safe to check.
    for (const label of ['New thread', 'Search everywhere', 'Setup guide']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(0);
    }

    // Workspaces names the workspace you are in, in all three shapes that row
    // can take. THIS harness is the middle one: `scripts/lib/e2e.sh` runs the
    // engine standalone and serves `dist/` at `/`, so the page is a direct
    // engine port. Its shell carries the gateway-port meta but base `/`, so the
    // picker is addressable absolutely while `/~/api/v1/control/*` would resolve
    // to the engine and 404. The row therefore links out here, becoming the
    // in-app switcher only under `/<slug>/`, which this suite cannot reach.
    // `computeGatewayPickerHref`'s unit tests own the three-way derivation; this
    // pins what must hold in EVERY shape.
    const workspaces = menu.locator('.brand-menu-item', { hasText: 'Workspaces' });
    await expect(workspaces.locator('.brand-menu-value-name')).toHaveCount(1);
    await expect(workspaces.locator('.brand-menu-value-name')).not.toBeEmpty();

    // ...and it must be able to SPELL it. The panel is fixed-width, so the room
    // the name gets is arithmetic: the glyph, the word "Workspaces", the gaps
    // and the pill's frame take that width first, and the pill's `max-width`
    // rations what is left. At 15rem the remainder held ~7 monospace
    // characters, so "development" rendered "develo…".
    //
    // The word is SPLICED IN rather than read off this suite's own workspace
    // name: the property under test is the panel's width budget, not how long
    // `e2e-test` happens to be. Measured in the real font at the real root size,
    // which a static reading of the CSS cannot do.
    const spelled = await workspaces.locator('.brand-menu-value-name').evaluate((el: HTMLElement) => {
      const original = el.textContent;
      el.textContent = 'development';
      // `clientWidth > 0` is not belt-and-braces. This span only HAS a box
      // because the pill around it is `inline-flex`. A bare inline box answers 0
      // to both metrics, so `scroll <= client` would read 0 <= 0 and pass while
      // the name overflowed unclipped.
      //
      // A pixel of tolerance: both metrics are integers, so neither can express
      // the sub-pixel rounding of a text run measured against a `calc`ed box. A
      // truncation would be tens of pixels, so a pixel cannot hide one.
      const fits = el.clientWidth > 0 && el.scrollWidth - el.clientWidth <= 1;
      el.textContent = original;
      return fits;
    });
    expect(spelled, 'the Workspaces pill ellipsises a name as ordinary as "development"').toBe(true);

    // The control plane is NOT reachable from this origin, so the row must not
    // offer the switcher: an expander here answers a tap with a 404 against
    // routes the engine does not serve. It links to the picker instead, which
    // makes it the menu's one anchor and the one row a user agent would
    // underline.
    await expect(workspaces).toHaveJSProperty('tagName', 'A');
    await expect(workspaces).toHaveAttribute('href', /\/~\/\?pick$/);
    await expect(workspaces.locator('.brand-menu-value-chevron')).toHaveCount(0);
    await expect(workspaces.locator('.brand-menu-value-check')).toHaveCount(1);
    await expect(workspaces).toHaveCSS('text-decoration-line', 'none');

    // The panel belongs to the thread pane's mark, so it is centred on that
    // pane, not on the window: a window-centred panel hangs over the content
    // pane and drifts further off the mark the narrower the split gets.
    // Measured against the header's own brand region, which spans exactly the
    // pane the mark sits in. So the assertion holds with the thread drawer open
    // as well as closed.
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

  test('says why the mark is dim, and stops saying it on reconnect', async ({ page }) => {
    // The one check in this file about the WIRING rather than the panel's
    // contents. Everything else the notice promises is pinned cheaply:
    // `connection-notice.test.ts` over the pure row, `header-mark-geometry` over
    // its dressing. None of it proves a real drop reaches the real panel.
    //
    // It runs in one project, the file's `-desktop` suffix keeping it out of
    // both mobile ones (playwright.config.ts). The cost is why that matters: the
    // dot flips only after MAX_SUPPRESSED_FAILURES + 1 consecutive failed polls
    // at 5s (store/actions/connection.ts, the tolerance that stops an iOS radio
    // nap painting red). So this spends ~20s going down and ~10s coming back,
    // and cannot be hurried from outside.
    test.slow();

    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    const notice = page.locator('.brand-menu-notice');
    const open = async () => {
      await toggle.click();
      await expect(menu).toBeVisible();
    };
    const close = async () => {
      await toggle.click();
      await expect(menu).toHaveCount(0);
    };

    // Connected: the mark is at full strength and the panel has nothing to
    // explain. Asserted first, so a notice found later is the state change and
    // not something the panel always carried.
    await expect(toggle).toHaveAttribute('data-conn', 'connected', { timeout: 30_000 });
    await open();
    await expect(notice).toHaveCount(0);
    await close();

    // The engine goes unreachable exactly as it does when it is down: the probe
    // never lands. `page.route` is scoped to THIS page, and Playwright gives
    // every test its own, so no sibling in this file inherits the outage.
    await page.route('**/api/v1/health', (route) => route.abort());
    await expect(toggle).toHaveAttribute('data-conn', 'disconnected', { timeout: 60_000 });

    await open();
    await expect(notice).toHaveCount(1);
    // The state in words, with the workspace it is about, and the dot carrying
    // the state so the shared `.status-dot` scale can colour it.
    await expect(notice).toContainText(/^Disconnected from \S/);
    await expect(notice.locator('.status-dot.disconnected')).toHaveCount(1);
    // A statement, not a control: the panel's rows are what answer a tap.
    await expect(notice.locator('button')).toHaveCount(0);
    await close();

    // ...and it retracts on its own the moment the engine answers again. A
    // notice that outlived the outage would be worse than none: it would report
    // an outage the mark says is over.
    await page.unroute('**/api/v1/health');
    await expect(toggle).toHaveAttribute('data-conn', 'connected', { timeout: 60_000 });
    await open();
    await expect(notice).toHaveCount(0);
    await close();
  });

  test('opening and closing the menu does not move the mark or the workspace name', async ({ page }) => {
    // Nothing about the menu is supposed to touch the row it hangs from. The
    // reported jump moved the mark and the workspace name TOGETHER, so their
    // shared box re-laid out, not the mark alone.
    //
    // Run twice, the second time with what `titlebar_inset_script` stamps on
    // the packaged macOS build. No CSS keys off Tauri itself, so the attribute
    // plus the inset IS that geometry: a 28px band above a shorter header, with
    // the leading controls raised into it.
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

  test('the brand label never overflows the region that clips it, at any mark size', async ({ page }) => {
    // The test above pins the SYMPTOM and could not reproduce it, because the
    // symptom needs a webview no WebDriver reaches (ADR 0016). This pins the
    // PRECONDITION, which every engine can see.
    //
    // `.pane-header-brand` declares no height, so it is as tall as its one
    // in-flow child, the actions cluster. The brand label it centres is out of
    // flow and as tall as the taller of the chevrons and the mark's TAP TARGET.
    // Let the label grow past the region and it hangs out of a box whose
    // `overflow-x: clip` beside `overflow-y: visible` the packaged webview does
    // not honour. There the clip behaves as a scroll container. A click on the
    // mark then scrolls the overflow away and back, taking the whole label with
    // it.
    //
    // Run at the shipped tap target and at a RETUNED one. The shipped pair
    // (2.25rem chevrons, 2.1rem mark) fits exactly, so a run at the default
    // alone can never fail. `--header-mark-tap` is a style-remote tunable that
    // the remote sets inline on <html> (see styles/header-mark.css), so this is
    // the real mechanism, not a test-only hook.
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    await expect(page.locator('.desktop-header .pane-header-brand')).toBeVisible();
    await expect(page.locator('.desktop-header .pane-header-brand-label')).toBeVisible();

    const fits = () => page.evaluate(() => {
      const region = document.querySelector('.desktop-header .pane-header-brand') as HTMLElement;
      const label = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
      const r = region.getBoundingClientRect();
      const l = label.getBoundingClientRect();
      return {
        // Positive means the label sticks out of that edge.
        above: r.top - l.top,
        below: l.bottom - r.bottom,
        // The direct statement of "there is nothing here for a webview to
        // scroll". Only block-end overflow is ever scrollable, so this catches
        // the same fact from the other side.
        scrollable: region.scrollHeight - region.clientHeight,
      };
    });

    for (const tap of [null, '2.6rem']) {
      await page.evaluate((value) => {
        const root = document.documentElement;
        if (value) root.style.setProperty('--header-mark-tap', value);
        else root.style.removeProperty('--header-mark-tap');
      }, tap);
      await page.waitForTimeout(100);

      const at = tap ? `--header-mark-tap: ${tap}` : 'the shipped mark';
      // At rest, and with the menu up: the region is what clips the label
      // whatever the menu is doing, and the click is when the webview reveals
      // the control it scrolled for.
      for (const state of ['closed', 'open'] as const) {
        if (state === 'open') {
          await toggle.click();
          await expect(menu).toBeVisible();
        }
        const box = await fits();
        // Sub-pixel rounding is unavoidable; anything a webview could scroll is
        // not.
        expect(box.above, `${at} (${state}): the label hangs above the region`).toBeLessThan(0.5);
        expect(box.below, `${at} (${state}): the label hangs below the region`).toBeLessThan(0.5);
        // A whole pixel here, not a sub-pixel: `scrollHeight` and `clientHeight`
        // are integers, so this difference cannot express the rounding the two
        // float measurements above can. The overflow it exists to catch is the
        // retuned mark's, which is several pixels.
        expect(box.scrollable, `${at} (${state}): the region has scrollable overflow`)
          .toBeLessThanOrEqual(1);
        if (state === 'open') {
          await toggle.click();
          await expect(menu).toHaveCount(0);
        }
      }
    }
  });
});
