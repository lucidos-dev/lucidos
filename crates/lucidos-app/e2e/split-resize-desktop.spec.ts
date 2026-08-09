import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, DRAWER_TOGGLE_LABEL } from './helpers';

/** Clamped divider drags (SplitLayout / DrawerDivider + splitHelpers; ADR 0056):
 *  a drag is clamped to the pane minimums as it moves, so it stops at the wall
 *  while the pointer keeps going, and nothing corrects it on release. While the
 *  drag is live the header regions track the panes 1:1 (data-pane-resizing
 *  disables their geometry transitions), the old always-on eases having left the
 *  header visibly disconnected from the panels mid-drag.
 *
 *  Desktop-only: the split layout and its header regions only exist on
 *  desktop (mobile swipes between full-screen panes). */

// Generous settle wait. Nothing is deferred any more, but a toggle or a reopen
// still animates for var(--duration-slow) (300ms).
const SETTLE_MS = 1200;
// All three pane floors are derived from the root font size now
// (store/paneMinimums.ts). These projects run at a 16px root, where the two
// split-pane floors ARE the 300 / 360 they were written as; the drawer's is not
// restated at all, since this spec MEASURES where the clamp puts it.
const MIN_THREAD_PANE = 300;
const MIN_CONTENT_PANE = 360;
// Well below the drawer's floor: the clamp must refuse to follow the pointer
// there rather than land and be corrected.
const BELOW_FLOOR_X = 150;
// Seeded before every drawer test so a narrowing drag has somewhere to travel
// FROM. It has to clear the drawer's floor by a wide margin, and the floor is
// 312 at these projects' 16px root now that it no longer varies by client
// (ADR 0058): opening at the default would put the drawer on its wall already,
// and a drag that cannot move proves nothing about a clamp.
const OPEN_DRAWER_WIDTH = 420;

test.use({ viewport: { width: 1280, height: 800 } });

async function dragDivider(page: Page, selector: string, toX: number, opts: { release?: boolean } = {}) {
  const divider = page.locator(selector);
  const box = await divider.boundingBox();
  if (!box) throw new Error(`${selector} not visible`);
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  if (opts.release !== false) await page.mouse.up();
}

async function threadPaneWidth(page: Page): Promise<number> {
  return page.evaluate(() => document.querySelector('.pane-thread')!.getBoundingClientRect().width);
}

async function drawerWidth(page: Page): Promise<number> {
  return page.evaluate(() => document.querySelector('.thread-drawer')!.getBoundingClientRect().width);
}

/** Viewport x of a point on the thread side of the desktop header that
 *  `AppHeader.isInteractive()` treats as a plain GAP — the empty run between the
 *  leading icon cluster and the centered Lucidos mark.
 *
 *  Derived, not hardcoded, and that is the point. This used to be a literal
 *  `x: 200`, which silently stopped being a gap once the brand label's content
 *  grew left over it: `isInteractive` gates out that content (the chevrons and
 *  the mark are buttons), so the dblclick was ignored, the pane never moved,
 *  and the failure surfaced as an *animation* timeout in a test about animation
 *  — pointing at the wrong subsystem entirely. Reading the live geometry keeps
 *  the point valid as the header evolves, and if the gap ever really does close
 *  the explicit width check below says so instead.
 *
 *  Scoped to `.desktop-header`: `MobileAppHeader` renders FIRST and carries
 *  copies of these classes, so an unscoped `querySelector` returns the hidden
 *  mobile one (the 0x0-rect trap in .claude/rules/frontend.md). */
async function threadHeaderGapX(page: Page): Promise<number> {
  const gap = await page.evaluate(() => {
    const header = document.querySelector('.desktop-header');
    if (!header) return null;
    // The leading icon host. One element in both drawer states: it used to be a
    // pair crossfading on data-thread-drawer-open, and is a single slot that
    // travels between the two positions now.
    const leading = header.querySelector('.thread-toggle-slot')?.getBoundingClientRect();
    const leadingRight = leading && leading.width > 0 ? leading.right : 0;
    // The whole centred cluster, not the mark inside it: the chevrons take its
    // ends, so the mark's left edge is well inside the interactive run.
    const cluster = header.querySelector('.pane-header-brand-label')?.getBoundingClientRect();
    if (!cluster || cluster.width === 0) return null;
    return { from: leadingRight, to: cluster.left };
  });
  expect(gap, 'desktop header: leading cluster / brand cluster not laid out').not.toBeNull();
  expect(
    gap!.to - gap!.from,
    `no non-interactive gap left on the thread header (leading cluster ends at `
      + `${gap!.from.toFixed(1)}, the brand cluster starts at ${gap!.to.toFixed(1)}): `
      + `header dblclick has nowhere left to land`,
  ).toBeGreaterThan(16);
  return (gap!.from + gap!.to) / 2;
}

/** openThreadDrawer returns as soon as the drawer has ANY width, but the open
 *  animates over var(--duration-slow): grabbing the divider mid-slide reads a
 *  stale bounding box and the drag silently misses it. Wait for the seeded
 *  OPEN_DRAWER_WIDTH to settle. */
async function openDrawerAndSettle(page: Page): Promise<void> {
  await openThreadDrawer(page);
  await page.waitForFunction((want) => {
    const drawer = document.querySelector('.thread-drawer');
    return !!drawer && Math.abs(drawer.getBoundingClientRect().width - want) < 1;
  }, OPEN_DRAWER_WIDTH, { timeout: 5_000 });
}

test.describe('Split layout: dividers clamp at the pane minimums', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript((width) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(width));
    }, OPEN_DRAWER_WIDTH);
    await navigateToApp(page);
  });

  test('drag above the minimum lands exactly where released and stays', async ({ page }) => {
    await dragDivider(page, '.split-divider', 560);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);

    // Nothing corrects it: no deferred anything.
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);
  });

  test('the thread pane stops AT its minimum, during the drag and after it', async ({ page }) => {
    // Mid-gesture, hard against the left edge: the divider is already at the
    // wall, not somewhere illegal awaiting a correction.
    await dragDivider(page, '.split-divider', 2, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const during = await threadPaneWidth(page);
    expect(Math.abs(during - MIN_THREAD_PANE),
      `mid-drag the thread pane sat at ${during}, not its ${MIN_THREAD_PANE} minimum`)
      .toBeLessThanOrEqual(2);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs(await threadPaneWidth(page) - MIN_THREAD_PANE)).toBeLessThanOrEqual(2);
  });

  test('the content pane stops at ITS minimum too, at the other end', async ({ page }) => {
    const total = await page.evaluate(() =>
      document.querySelector('.split-layout')!.getBoundingClientRect().width);
    await dragDivider(page, '.split-divider', Math.round(total) + 400, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const contentPx = total - await threadPaneWidth(page);
    expect(Math.abs(contentPx - MIN_CONTENT_PANE),
      `mid-drag the content pane sat at ${contentPx}, not its ${MIN_CONTENT_PANE} minimum`)
      .toBeLessThanOrEqual(3);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs((total - await threadPaneWidth(page)) - MIN_CONTENT_PANE)).toBeLessThanOrEqual(3);
  });

  test('a drag NEVER collapses a pane, at either extreme', async ({ page }) => {
    // The collapse-state attributes swap header icon groups between hosts, so a
    // flip while the pointer wiggles across a pane edge is the "icons dance
    // between the headers" bug. A clamped drag cannot reach ratio 0 or 1, which
    // is what makes the flip unreachable rather than merely postponed (ADR 0056).
    const flags = () => page.evaluate(() => ({
      thread: document.documentElement.hasAttribute('data-thread-collapsed'),
      content: document.documentElement.hasAttribute('data-content-collapsed'),
      drawer: document.documentElement.hasAttribute('data-thread-drawer-open'),
    }));
    const before = await flags();

    for (const x of [2, 4000, 2]) {
      await dragDivider(page, '.split-divider', x, { release: false });
      await page.evaluate(() => new Promise(requestAnimationFrame));
      expect(await flags(), `mid-drag at x=${x}`).toEqual(before);
      await page.mouse.up();
      await page.waitForTimeout(SETTLE_MS);
      expect(await flags(), `after releasing at x=${x}`).toEqual(before);
    }
    await expect(page.locator('.pane-thread')).not.toHaveClass(/pane-collapsed/);
    await expect(page.locator('.pane-content')).not.toHaveClass(/pane-collapsed/);
  });

  test('collapse is still reachable, by double-clicking the divider', async ({ page }) => {
    // The drag lost the ability; every other route must keep it, or the change
    // removed a capability rather than moving it.
    await page.locator('.split-divider').dblclick();
    await expect(page.locator('.pane-thread')).toHaveClass(/pane-collapsed/, { timeout: 5_000 });
    await page.locator('.split-divider').dblclick();
    await expect(page.locator('.pane-thread')).not.toHaveClass(/pane-collapsed/, { timeout: 5_000 });
  });

  test('header regions track the panes 1:1 while dragging', async ({ page }) => {
    await dragDivider(page, '.split-divider', 500, { release: false });

    const measure = () => page.evaluate(() => {
      const region = document.querySelector('.content-header-elements')!.getBoundingClientRect();
      const pane = document.querySelector('.pane-content')!.getBoundingClientRect();
      return Math.abs(region.left - pane.left);
    });

    // Mid-drag, after a paint: the header region's left edge must sit on the
    // content pane's left edge. With the old always-on 300ms ease the region
    // would still be far from the pane right after a move.
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.move(820, 400, { steps: 2 });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.up();
  });

  test('the drawer stops AT its floor, during the drag and after it', async ({ page }) => {
    await openDrawerAndSettle(page);
    // Mid-gesture: already at the wall, not below it awaiting a correction.
    await dragDivider(page, '.drawer-divider', BELOW_FLOOR_X, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const during = await drawerWidth(page);
    expect(during, `mid-drag the drawer sat at ${during}, below where it can rest`)
      .toBeGreaterThan(BELOW_FLOOR_X + 2);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    const settled = await drawerWidth(page);
    expect(settled, 'the drawer moved after release').toBe(during);
    expect(settled, 'the drag never moved the drawer, so this is not the floor')
      .toBeLessThan(OPEN_DRAWER_WIDTH - 40);
    // The floor is what the drawer's own header row needs, so the row has to fit
    // in it: that is the property the number was standing in for.
    const fits = await page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find(h => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const search = header.querySelector('button[aria-label="Search threads"]') as HTMLElement;
      const title = header.querySelector('.threads-header-title') as HTMLElement;
      return {
        searchInside: search.getBoundingClientRect().right
          <= header.getBoundingClientRect().right + 1,
        titleWidth: title.getBoundingClientRect().width,
      };
    });
    expect(fits, 'visible threads-header').not.toBeNull();
    expect(fits!.searchInside, 'the Search button overflows the drawer at its floor').toBe(true);
    expect(fits!.titleWidth, 'no room left for the title at the floor').toBeGreaterThan(20);
  });

  test('dragging the drawer hard shut does NOT close it; the toggle still does', async ({ page }) => {
    await openDrawerAndSettle(page);
    await dragDivider(page, '.drawer-divider', BELOW_FLOOR_X);
    await page.waitForTimeout(SETTLE_MS);
    const floor = await drawerWidth(page);

    // All the way to the window edge and past it. The drawer holds its floor.
    await dragDivider(page, '.drawer-divider', -200);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator('.thread-drawer')).not.toHaveClass(/thread-drawer-collapsed/);
    expect(await drawerWidth(page)).toBe(floor);

    // Closing is the toggle's job, and it still works.
    await page.locator(`button[aria-label^="${DRAWER_TOGGLE_LABEL}"]:visible`).first().click();
    await expect(page.locator('.thread-drawer')).toHaveClass(/thread-drawer-collapsed/, { timeout: 5_000 });
    await openThreadDrawer(page);
    await expect.poll(() => drawerWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(floor - 1);
  });

  test('widening the drawer stops at the thread pane\'s minimum, not past it', async ({ page }) => {
    await openDrawerAndSettle(page);
    // The drawer drag holds the content pane at a constant pixel width, so every
    // pixel the drawer gains comes out of the THREAD pane. That is the less
    // obvious end of the drawer's clamp, and the one a ceiling-less version
    // squeezed to nothing.
    await dragDivider(page, '.drawer-divider', 900, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const during = await threadPaneWidth(page);
    expect(during, `mid-drag the thread pane sat at ${during}`)
      .toBeGreaterThanOrEqual(MIN_THREAD_PANE - 2);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(await threadPaneWidth(page)).toBeGreaterThanOrEqual(MIN_THREAD_PANE - 2);
  });

  test('drawer toggle slides the drawer header shut with the drawer — no pop', async ({ page }) => {
    await openDrawerAndSettle(page);

    // openDrawerAndSettle opened the drawer; the toggle is a plain show/hide, so
    // this second click simply hides it (no focus stage in between).
    // Prefix match: the label carries a " (N needing attention)" suffix whenever
    // the thread list is hidden and something is waiting on the user.
    await page.locator(`button[aria-label^="${DRAWER_TOGGLE_LABEL}"]:visible`).first().click();
    // The drawer header must pass through intermediate widths: it stays
    // mounted and its width rides --content-offset through the transition.
    // The old conditional render unmounted it at toggle time — width would
    // jump straight to 0 and this wait would time out at an intermediate
    // sample... by never seeing one (it polls every frame).
    await page.waitForFunction(() => {
      const header = document.querySelector('.threads-header');
      if (!header) return false; // unmounting is exactly the pop we're pinning against
      const w = header.getBoundingClientRect().width;
      return w > 20 && w < 280;
    }, undefined, { timeout: 2_000 });

    // After the transition: still mounted, slid to zero, out of the tab order.
    await expect.poll(
      () => page.evaluate(() => {
        const header = document.querySelector('.threads-header');
        if (!header) return -1;
        return header.getBoundingClientRect().width;
      }),
      { timeout: 5_000 },
    ).toBeLessThanOrEqual(1);
    // Polled: the visibility transition flips to hidden right as the width
    // lands — a single sample could race it by a frame.
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.threads-header')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the content pane fades the thread-side header instead of popping it', async ({ page }) => {
    // Double-click the divider → thread pane collapses (content maximized).
    const divider = page.locator('.split-divider');
    await divider.dblclick();

    // The brand must fade through intermediate opacities — display:none or
    // unmount would make it vanish at the start of the pane animation.
    // .desktop-header scoping matters: MobileAppHeader renders an earlier
    // .pane-header-brand copy that is display:none on desktop, where
    // transitions don't run and opacity flips instantly.
    await page.waitForFunction(() => {
      const brand = document.querySelector('.desktop-header .pane-header-brand');
      if (!brand) return false;
      const opacity = parseFloat(getComputedStyle(brand).opacity);
      return opacity > 0.05 && opacity < 0.95;
    }, undefined, { timeout: 2_000 });

    // Settled: still mounted, fully faded, hidden from interaction.
    await expect.poll(
      () => page.evaluate(() => {
        const brand = document.querySelector('.desktop-header .pane-header-brand');
        if (!brand) return -1;
        return parseFloat(getComputedStyle(brand).opacity);
      }),
      { timeout: 5_000 },
    ).toBe(0);
    // Polled: visibility flips at the opacity transition's end — a single
    // sample could race it by a frame.
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.desktop-header .pane-header-brand')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the thread pane animates the pane body, not just the header', async ({ page }) => {
    // Double-click the thread side of the header → content pane collapses, the
    // thread pane maximizes to full width. The pane body must animate to full
    // width over var(--duration-slow) like its header does — the bug was that
    // the pane jumped instantly (its flex switched from basis-driven to
    // grow-driven, and .pane-animate only transitioned flex-basis) while the
    // header slid smoothly, so they read as disconnected.
    const startWidth = await threadPaneWidth(page); // ~512 at ratio 0.4 / 1280
    expect(startWidth).toBeLessThan(700);

    // Land on a real non-interactive gap on the thread side, so
    // onHeaderDblClick actually maximizes the thread pane (see
    // threadHeaderGapX — a hardcoded x silently drifted onto the brand text).
    const header = page.locator('.app-header');
    const headerBox = await header.boundingBox();
    if (!headerBox) throw new Error('.app-header not visible');
    const gapX = await threadHeaderGapX(page);
    await header.dblclick({ position: { x: gapX - headerBox.x, y: 20 } });

    // The pane width must pass through an intermediate value during the
    // transition. With the bug it snaps straight to full width and this never
    // samples an in-between value (waitForFunction would time out).
    await page.waitForFunction(() => {
      const w = document.querySelector('.pane-thread')!.getBoundingClientRect().width;
      return w > 650 && w < 1150;
    }, undefined, { timeout: 2_000 });

    // Settled: thread fills the row (only the divider + its margin remain).
    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeGreaterThan(1150);
  });
});

/** Comfortably above the drawer's floor on BOTH desktop builds, so the title has
 *  room to be a title and nothing below is measuring a clamp instead. */
const WIDE_DRAWER = 500;

async function openDrawerAtWidth(page: Page, width: number): Promise<void> {
  await openThreadDrawer(page);
  await page.waitForFunction((w) => {
    const drawer = document.querySelector('.thread-drawer');
    return !!drawer && Math.abs(drawer.getBoundingClientRect().width - w) < 1;
  }, width, { timeout: 5_000 });
}

/** Where the drawer header's title actually landed, against the band it is
 *  centred on and the two buttons it must clear. Scoped to `.desktop-header`:
 *  `MobileAppHeader` renders first and carries its own copies (the 0x0-rect trap
 *  in .claude/rules/frontend.md). */
async function threadsHeaderGeometry(page: Page) {
  const geo = await page.evaluate(() => {
    const band = document.querySelector('.desktop-header .threads-header');
    const title = band?.querySelector('.threads-header-title');
    const filter = band?.querySelector('button[aria-label="Filter threads"]');
    const search = band?.querySelector('button[aria-label="Search threads"]');
    if (!band || !title || !filter || !search) return null;
    const b = band.getBoundingClientRect();
    const t = title.getBoundingClientRect();
    return {
      bandCentre: b.left + b.width / 2,
      titleCentre: t.left + t.width / 2,
      titleLeft: t.left,
      titleRight: t.right,
      titleWidth: t.width,
      filterRight: filter.getBoundingClientRect().right,
      searchLeft: search.getBoundingClientRect().left,
    };
  });
  expect(geo, 'desktop threads header not laid out').not.toBeNull();
  return geo!;
}

test.describe('Threads header: a pane-centred title, and a band that answers no double-click', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript((w) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(w));
    }, WIDE_DRAWER);
    await navigateToApp(page);
  });

  test('the title centres on the drawer pane, not on the gap between the icons', async ({ page }) => {
    await openDrawerAtWidth(page, WIDE_DRAWER);

    const web = await threadsHeaderGeometry(page);
    expect(Math.abs(web.titleCentre - web.bandCentre),
      `title centred at ${web.titleCentre.toFixed(1)}, the drawer pane at ${web.bandCentre.toFixed(1)}`)
      .toBeLessThanOrEqual(1);
    expect(web.titleWidth, 'the title clamped away to nothing').toBeGreaterThan(20);
    expect(web.titleLeft, 'the title runs under the Filter button').toBeGreaterThanOrEqual(web.filterRight);
    expect(web.titleRight, 'the title runs under the Search button').toBeLessThanOrEqual(web.searchLeft);

    // Now the packaged macOS layout, which is where this was reported: the row
    // starts after --titlebar-lights-reserve there, and a title centred on the
    // GAP between the two buttons lands (reserve - 0.5rem) / 2 to the right of
    // the pane's middle. The real build cannot be driven by a browser test
    // (ADR 0016: WKWebView exposes no WebDriver), but the layout is switched by
    // an attribute and the reserve is a flat px, so stamping the attribute
    // reproduces exactly the geometry that was wrong.
    await page.evaluate(() => document.documentElement.setAttribute('data-titlebar-overlay', ''));
    await page.waitForTimeout(SETTLE_MS); // the row's padding transitions to the reserve

    const overlay = await threadsHeaderGeometry(page);
    expect(overlay.filterRight, 'the lights reserve did not apply, so this proves nothing')
      .toBeGreaterThan(web.filterRight + 40);
    expect(Math.abs(overlay.titleCentre - overlay.bandCentre),
      `with the lights reserve applied the title centred at ${overlay.titleCentre.toFixed(1)}, `
        + `the drawer pane at ${overlay.bandCentre.toFixed(1)}`)
      .toBeLessThanOrEqual(1);
    expect(overlay.titleWidth, 'the title clamped away to nothing').toBeGreaterThan(20);
    expect(overlay.titleLeft, 'the title runs under the Filter button')
      .toBeGreaterThanOrEqual(overlay.filterRight);
    expect(overlay.titleRight, 'the title runs under the Search button')
      .toBeLessThanOrEqual(overlay.searchLeft);
  });

  test('double-clicking the drawer header changes no pane geometry', async ({ page }) => {
    await openDrawerAtWidth(page, WIDE_DRAWER);

    const ratio = () => page.evaluate(() =>
      document.documentElement.style.getPropertyValue('--split-ratio'));
    const before = await ratio();
    expect(before, 'SplitLayout publishes the ratio to CSS; nothing to compare against').not.toBe('');

    // The title is the segment's largest non-interactive surface, so this is the
    // click a user makes. Without the fence, onHeaderDblClick reads everything
    // left of the split divider as "the thread side" and maximizes that pane
    // group. The Conversation side still does exactly that, which the
    // "maximizing the thread pane" test above covers.
    await page.locator('.desktop-header .threads-header-title').dblclick();
    await page.waitForTimeout(SETTLE_MS);

    expect(await ratio(), 'the drawer header maximized a pane group').toBe(before);
    await expect(page.locator('.pane-content')).not.toHaveClass(/pane-collapsed/);
    await expect(page.locator('.pane-thread')).not.toHaveClass(/pane-collapsed/);
    // And the drawer it belongs to is untouched.
    expect(Math.abs(await drawerWidth(page) - WIDE_DRAWER)).toBeLessThanOrEqual(1);

    // The bar is taller than the 2.25rem control row inside it, so a press a
    // couple of px from its bottom edge lands on the bar rather than on
    // `.threads-header`. That sliver is still the drawer's segment: it is why
    // the attribution is geometry (headerDblClickRegion) and not a
    // `closest('.threads-header')` fence, which would let exactly this through.
    const header = page.locator('.app-header');
    const box = await header.boundingBox();
    if (!box) throw new Error('.app-header not visible');
    await header.dblclick({ position: { x: WIDE_DRAWER / 2, y: box.height - 2 } });
    await page.waitForTimeout(SETTLE_MS);

    expect(await ratio(), 'the sliver above/below the control row still moved the split')
      .toBe(before);
    await expect(page.locator('.pane-content')).not.toHaveClass(/pane-collapsed/);
  });
});
