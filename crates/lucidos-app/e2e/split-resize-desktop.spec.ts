import { test, expect, type Page } from './fixtures';
import { assertHealthy, hideDashboardRail, minThreadPane, navigateToApp, openThreadDrawer, stampOverlayBuild, DRAWER_TOGGLE_LABEL } from './helpers';
import { THREAD_ROW_LEAD_CONTROLS } from '../src/store/paneMinimums';

test.use({ reducedMotion: 'no-preference' });


const SETTLE_MS = 1200;
const MIN_CONTENT_PANE = 360;

const BELOW_FLOOR_X = 150;
const OPEN_DRAWER_WIDTH = 400;

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

async function threadHeaderGapX(page: Page): Promise<number> {
  const gap = await page.evaluate((leadSelector) => {
    const header = document.querySelector('.desktop-header');
    if (!header) return null;
    const shown = (els: Iterable<Element | null>) => [...els]
      .map((el) => el?.getBoundingClientRect())
      .filter((r): r is DOMRect => !!r && r.width > 0);
    const leadingRight = Math.max(0, ...shown([header.querySelector('.thread-toggle-slot'), ...document.querySelectorAll(leadSelector)]).map((r) => r.right));
    const threadSide = header.querySelector('.pane-header-brand')?.getBoundingClientRect();
    const cluster = header.querySelector('.pane-header-brand-label')?.getBoundingClientRect();
    if (!cluster || cluster.width === 0 || !threadSide) return null;
    const trailingLeft = Math.min(threadSide.right, ...shown(header.querySelectorAll('button, a'))
      .filter((r) => r.left >= cluster.right && r.left < threadSide.right)
      .map((r) => r.left));
    const before = { from: Math.max(leadingRight, threadSide.left), to: cluster.left };
    const after = { from: cluster.right, to: trailingLeft };
    return before.to - before.from >= after.to - after.from ? before : after;
  }, THREAD_ROW_LEAD_CONTROLS);
  expect(gap, 'desktop header: leading cluster / brand cluster not laid out').not.toBeNull();
  expect(
    gap!.to - gap!.from,
    `no non-interactive gap left on the thread header (the widest runs from `
      + `${gap!.from.toFixed(1)} to ${gap!.to.toFixed(1)}): `
      + `header dblclick has nowhere left to land`,
  ).toBeGreaterThan(16);
  return (gap!.from + gap!.to) / 2;
}

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
    await hideDashboardRail(page);
    await navigateToApp(page);
  });

  test('drag above the minimum lands exactly where released and stays', async ({ page }) => {
    await dragDivider(page, '.split-divider', 560);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);

    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);
  });

  test('the thread pane stops AT its minimum, during the drag and after it', async ({ page }) => {
    await dragDivider(page, '.split-divider', 2, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const during = await threadPaneWidth(page);
    const floor = await minThreadPane(page);
    expect(Math.abs(during - floor),
      `mid-drag the thread pane sat at ${during}, not its ${floor} minimum`)
      .toBeLessThanOrEqual(2);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs(await threadPaneWidth(page) - floor)).toBeLessThanOrEqual(2);
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
    await page.locator('.split-divider').dblclick();
    await expect(page.locator('.pane-thread')).toHaveClass(/pane-collapsed/, { timeout: 5_000 });
    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeLessThanOrEqual(1);
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

    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.move(820, 400, { steps: 2 });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.up();
  });

  test('the drawer stops AT its floor, during the drag and after it', async ({ page }) => {
    await openDrawerAndSettle(page);
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

    await dragDivider(page, '.drawer-divider', -200);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator('.thread-drawer')).not.toHaveClass(/thread-drawer-collapsed/);
    expect(await drawerWidth(page)).toBe(floor);

    await page.locator(`button[aria-label^="${DRAWER_TOGGLE_LABEL}"]:visible`).first().click();
    await expect(page.locator('.thread-drawer')).toHaveClass(/thread-drawer-collapsed/, { timeout: 5_000 });
    await openThreadDrawer(page);
    await expect.poll(() => drawerWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(floor - 1);
  });

  test('widening the drawer stops at the thread pane\'s minimum, not past it', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await openDrawerAndSettle(page);
    await dragDivider(page, '.split-divider', 1200);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator('.pane-content'), 'the Canvas yielded before the drag')
      .not.toHaveClass(/pane-collapsed/);

    await dragDivider(page, '.drawer-divider', 1500, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const during = await threadPaneWidth(page);
    const floor = await minThreadPane(page);
    expect(during, `mid-drag the thread pane sat at ${during}`)
      .toBeGreaterThanOrEqual(floor - 2);
    expect(await drawerWidth(page), 'the drawer never widened, so no clamp was reached')
      .toBeGreaterThan(OPEN_DRAWER_WIDTH + 100);
    const content = await page.evaluate(() =>
      document.querySelector('.pane-content')!.getBoundingClientRect().width);
    expect(content, `the drawer took its width from the Canvas, which sat at ${content}`)
      .toBeGreaterThanOrEqual(MIN_CONTENT_PANE);

    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(await threadPaneWidth(page)).toBeGreaterThanOrEqual(floor - 2);
  });

  test('drawer toggle slides the drawer header shut with the drawer — no pop', async ({ page }) => {
    await openDrawerAndSettle(page);

    await page.locator(`button[aria-label^="${DRAWER_TOGGLE_LABEL}"]:visible`).first().click();
    await page.waitForFunction(() => {
      const header = document.querySelector('.threads-header');
      if (!header) return false;
      const w = header.getBoundingClientRect().width;
      return w > 20 && w < 280;
    }, undefined, { timeout: 2_000 });

    await expect.poll(
      () => page.evaluate(() => {
        const header = document.querySelector('.threads-header');
        if (!header) return -1;
        return header.getBoundingClientRect().width;
      }),
      { timeout: 5_000 },
    ).toBeLessThanOrEqual(1);
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.threads-header')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the content pane fades the thread-side header instead of popping it', async ({ page }) => {
    const divider = page.locator('.split-divider');
    await divider.dblclick();

    await page.waitForFunction(() => {
      const brand = document.querySelector('.desktop-header .pane-header-brand');
      if (!brand) return false;
      const opacity = parseFloat(getComputedStyle(brand).opacity);
      return opacity > 0.05 && opacity < 0.95;
    }, undefined, { timeout: 2_000 });

    await expect.poll(
      () => page.evaluate(() => {
        const brand = document.querySelector('.desktop-header .pane-header-brand');
        if (!brand) return -1;
        return parseFloat(getComputedStyle(brand).opacity);
      }),
      { timeout: 5_000 },
    ).toBe(0);
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.desktop-header .pane-header-brand')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the thread pane animates the pane body, not just the header', async ({ page }) => {
    const startWidth = await threadPaneWidth(page);
    expect(startWidth).toBeLessThan(700);

    const header = page.locator('.app-header');
    const headerBox = await header.boundingBox();
    if (!headerBox) throw new Error('.app-header not visible');
    const gapX = await threadHeaderGapX(page);
    await header.dblclick({ position: { x: gapX - headerBox.x, y: 20 } });

    await page.waitForFunction(() => {
      const w = document.querySelector('.pane-thread')!.getBoundingClientRect().width;
      return w > 650 && w < 1150;
    }, undefined, { timeout: 2_000 });

    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeGreaterThan(1150);
  });
});

const WIDE_DRAWER = 500;

async function openDrawerAtWidth(page: Page, width: number): Promise<void> {
  await openThreadDrawer(page);
  await page.waitForFunction((w) => {
    const drawer = document.querySelector('.thread-drawer');
    return !!drawer && Math.abs(drawer.getBoundingClientRect().width - w) < 1;
  }, width, { timeout: 5_000 });
}

async function threadsHeaderGeometry(page: Page) {
  const geo = await page.evaluate(() => {
    const band = document.querySelector('.desktop-header .threads-header');
    const title = band?.querySelector('.threads-header-title');
    const toggle = document.querySelector('.desktop-header .thread-toggle-slot .thread-toggle');
    const filter = band?.querySelector('button[aria-label="Filter threads"]');
    const grouping = band?.querySelector('.grouping-btn');
    if (!band || !title || !toggle || !filter || !grouping) return null;
    const b = band.getBoundingClientRect();
    const t = title.getBoundingClientRect();
    return {
      bandCentre: b.left + b.width / 2,
      titleCentre: t.left + t.width / 2,
      titleLeft: t.left,
      titleRight: t.right,
      titleWidth: t.width,
      toggleRight: toggle.getBoundingClientRect().right,
      filterRight: filter.getBoundingClientRect().right,
      groupingLeft: grouping.getBoundingClientRect().left,
    };
  });
  expect(geo, 'desktop threads header not laid out').not.toBeNull();
  return geo!;
}

test.describe('Threads header: a pane-centred title, and a band that answers no double-click', () => {
  test.use({ viewport: { width: 1440, height: 800 } });

  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript((w) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(w));
    }, WIDE_DRAWER);
    await hideDashboardRail(page);
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
    expect(web.titleRight, 'the title runs under the Grouping button').toBeLessThanOrEqual(web.groupingLeft);

    await stampOverlayBuild(page);
    await page.waitForTimeout(SETTLE_MS);

    const overlay = await threadsHeaderGeometry(page);
    expect(overlay.toggleRight, 'the lights reserve did not apply, so this proves nothing')
      .toBeGreaterThan(web.toggleRight + 40);
    expect(Math.abs(overlay.titleCentre - overlay.bandCentre),
      `with the lights reserve applied the title centred at ${overlay.titleCentre.toFixed(1)}, `
        + `the drawer pane at ${overlay.bandCentre.toFixed(1)}`)
      .toBeLessThanOrEqual(1);
    expect(overlay.titleWidth, 'the title clamped away to nothing').toBeGreaterThan(20);
    expect(overlay.titleLeft, 'the title runs under the Filter button')
      .toBeGreaterThanOrEqual(overlay.filterRight);
    expect(overlay.titleRight, 'the title runs under the Grouping button')
      .toBeLessThanOrEqual(overlay.groupingLeft);
  });

  test('double-clicking the drawer header changes no pane geometry', async ({ page }) => {
    await openDrawerAtWidth(page, WIDE_DRAWER);

    const ratio = () => page.evaluate(() =>
      document.documentElement.style.getPropertyValue('--split-ratio'));
    const before = await ratio();
    expect(before, 'SplitLayout publishes the ratio to CSS; nothing to compare against').not.toBe('');

    await page.locator('.desktop-header .threads-header-title').dblclick();
    await page.waitForTimeout(SETTLE_MS);

    expect(await ratio(), 'the drawer header maximized a pane group').toBe(before);
    await expect(page.locator('.pane-content')).not.toHaveClass(/pane-collapsed/);
    await expect(page.locator('.pane-thread')).not.toHaveClass(/pane-collapsed/);
    expect(Math.abs(await drawerWidth(page) - WIDE_DRAWER)).toBeLessThanOrEqual(1);

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
