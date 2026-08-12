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
 * The panel's other job is answering for the mark: when the connection light
 * recedes, the menu is where the state is spelled out, and the third test here
 * is the only check that a real drop reaches the real panel.
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

    // Workspaces names the workspace you are in, in all three of the shapes
    // that row can take. THIS harness is the middle one, and knowing which is
    // load-bearing: `scripts/lib/e2e.sh` runs the engine standalone and lets it
    // serve `dist/` at `/` (there is no Vite dev server in an e2e run, whatever
    // this comment used to say), so the page is a direct engine port. Its shell
    // carries the gateway-port meta but base `/`, which means the picker is
    // addressable absolutely while `/~/api/v1/control/*` would resolve to the
    // engine and 404. So the row links out here; it becomes the in-app
    // switcher only under `/<slug>/`, which this suite has no way to reach.
    // The three-way derivation is left to `computeGatewayPickerHref`'s unit
    // tests and the switcher's own body tests; this pins what must hold in
    // EVERY shape, plus the link shape itself.
    const workspaces = menu.locator('.brand-menu-item', { hasText: 'Workspaces' });
    await expect(workspaces.locator('.brand-menu-value-name')).toHaveCount(1);
    await expect(workspaces.locator('.brand-menu-value-name')).not.toBeEmpty();

    // ...and it must be able to SPELL it. The panel is fixed-width, so the room
    // the name gets is arithmetic: the row's glyph, the word "Workspaces", the
    // gaps and the pill's own frame all come out of that width first, and what
    // is left is rationed again by the pill's `max-width`. At 15rem the
    // remainder held ~7 monospace characters, so a workspace called
    // "development" rendered "develo…", which identifies nothing.
    //
    // The word is SPLICED IN rather than asserted on this suite's own workspace
    // name: the property under test is the panel's width budget, not how long
    // `e2e-test` happens to be, and renaming the workspace to prove it would
    // couple a layout guard to the harness. Measured in the real font at the
    // real root size, which is the half a static reading of the CSS cannot do.
    const spelled = await workspaces.locator('.brand-menu-value-name').evaluate((el: HTMLElement) => {
      const original = el.textContent;
      el.textContent = 'development';
      // `clientWidth > 0` is not belt-and-braces: this span only HAS a box
      // because the pill around it is `inline-flex`, and a bare inline box
      // answers 0 to both metrics, so `scroll <= client` would read 0 <= 0 and
      // pass while the name overflowed unclipped.
      //
      // A pixel of tolerance for the same reason the containment probe below
      // takes one: both metrics are integers, so neither can express the
      // sub-pixel rounding of a text run measured against a `calc`ed box. The
      // budget leaves roughly two characters past this word, so a pixel cannot
      // hide a truncation, which would be tens of pixels.
      const fits = el.clientWidth > 0 && el.scrollWidth - el.clientWidth <= 1;
      el.textContent = original;
      return fits;
    });
    expect(spelled, 'the Workspaces pill ellipsises a name as ordinary as "development"').toBe(true);

    // The control plane is NOT reachable from this origin, so the row must not
    // offer the switcher: an expander here answers a tap with a 404 against
    // routes the engine does not serve. It links to the picker instead, which
    // is also the menu's one <a> and therefore the one row that would otherwise
    // wear the user agent's link underline.
    await expect(workspaces).toHaveJSProperty('tagName', 'A');
    await expect(workspaces).toHaveAttribute('href', /\/~\/\?pick$/);
    await expect(workspaces.locator('.brand-menu-value-chevron')).toHaveCount(0);
    await expect(workspaces.locator('.brand-menu-value-check')).toHaveCount(1);
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

  test('says why the mark is dim, and stops saying it on reconnect', async ({ page }) => {
    // The one check in this file that is not about the panel's contents but
    // about the WIRING. Everything else the notice promises is pinned cheaply
    // (`connection-notice.test.ts` over the pure row, `header-mark-geometry`
    // over its dressing), and none of it proves that a real drop reaches the
    // real panel.
    //
    // It runs in one project, which is the file's `-desktop` suffix doing its
    // job (playwright.config.ts ignores those in both mobile projects) rather
    // than anything this test asks for. Worth knowing here, because the cost is
    // real and paying it three times would not be: the dot flips only after
    // MAX_SUPPRESSED_FAILURES + 1 consecutive failed polls at 5s
    // (store/actions/connection.ts, the tolerance that stops an iOS radio nap
    // painting red), so this spends ~20s going down and ~10s coming back
    // (MIN_RECONNECT_SUCCESSES) and cannot be hurried from outside.
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

  test('the brand label never overflows the region that clips it, at any mark size', async ({ page }) => {
    // The test above pins the SYMPTOM and could not reproduce it, because the
    // symptom needs a webview no WebDriver reaches (ADR 0016). This pins the
    // PRECONDITION, which every engine can see.
    //
    // `.pane-header-brand` declares no height, so it is as tall as its one
    // in-flow child, the actions cluster. The brand label it centres is out of
    // flow and as tall as the taller of the chevrons and the mark's TAP TARGET.
    // Let the label grow past the region and it hangs out of a box whose
    // `overflow-x: clip` beside `overflow-y: visible` the packaged macOS webview
    // does not honour: there the clip behaves as a scroll container, so the
    // overflow is not hidden but SCROLLABLE, and a click on the mark scrolls it
    // away and back. That is the reported jump, and it takes the whole label
    // with it, which is why the user sees the mark and both chevrons move
    // together while the rest of the row stands still.
    //
    // Run at the shipped tap target and at a RETUNED one. Both matter: the
    // shipped pair (2.25rem chevrons, 2.1rem mark) fits exactly, so a run at the
    // default alone can never fail, and it is the retune that made this
    // reproducible on one machine and nowhere else. `--header-mark-tap` is a
    // style-remote tunable and the remote sets it by writing the property inline
    // on <html> (see the header of styles/header-mark.css), so this is the real
    // mechanism rather than a test-only hook.
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
