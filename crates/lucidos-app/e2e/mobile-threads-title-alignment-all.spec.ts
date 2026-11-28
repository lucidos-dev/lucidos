import { test, expect, Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { assertHealthy, navigateToApp, openThreadDrawer, ensureMobileView, gotoWithRetry, watchPreferenceReads } from './helpers';

test.use({ reducedMotion: 'no-preference' });

const TRAILING_LEFTMOST = '.brand-mark-slot';

interface Metrics {
  center: number;
  rowCenter: number;
  left: number;
  right: number;
  leadingRight: number;
  trailingLeft: number;
  clientWidth: number;
  scrollWidth: number;
  maxWidth: string;
}

async function measure(
  page: Page,
  headerSel: string,
  titleSel: string,
  leadingSel: string,
  trailingSel: string,
  text?: string,
): Promise<Metrics | null> {
  return page.evaluate(({ headerSel, titleSel, leadingSel, trailingSel, text }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    const leading = header.querySelector(leadingSel) as HTMLElement | null;
    const trailing = header.querySelector(trailingSel) as HTMLElement | null;
    if (!row || !leading || leading.getBoundingClientRect().width === 0) return null;
    if (!trailing || trailing.getBoundingClientRect().width === 0) return null;
    const rowRect = row.getBoundingClientRect();
    for (const el of header.querySelectorAll(titleSel)) {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0) continue;
      if (text != null && (el.textContent ?? '').trim() !== text) continue;
      const h = el as HTMLElement;
      return {
        center: rect.left + rect.width / 2,
        rowCenter: rowRect.left + rowRect.width / 2,
        left: rect.left,
        right: rect.right,
        leadingRight: leading.getBoundingClientRect().right,
        trailingLeft: trailing.getBoundingClientRect().left,
        clientWidth: h.clientWidth,
        scrollWidth: h.scrollWidth,
        maxWidth: getComputedStyle(h).maxWidth,
      };
    }
    return null;
  }, { headerSel, titleSel, leadingSel, trailingSel, text });
}

async function navGeometry(page: Page, headerSel: string): Promise<
  { arm: 'floor' | 'reserve'; reserve: number; box: number } | null
> {
  return page.evaluate((sel) => {
    const header = document.querySelector(sel) as HTMLElement | null;
    const row = header?.querySelector('.mobile-header-row') as HTMLElement | null;
    const cluster = header?.querySelector('.header-nav-cluster') as HTMLElement | null;
    if (!row || !cluster || cluster.getBoundingClientRect().width === 0) return null;
    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.visibility = 'hidden';
    row.appendChild(probe);
    const widthOf = (value: string): number => {
      probe.style.width = value;
      return probe.getBoundingClientRect().width;
    };
    const floor = widthOf('var(--header-nav-min-span)');
    const reserve = widthOf('var(--header-nav-edge-reserve)');
    const box = widthOf('var(--mobile-header-icon-box)');
    probe.remove();
    const afterReserve = row.getBoundingClientRect().width - 2 * reserve;
    return { arm: afterReserve < floor ? 'floor' : 'reserve', reserve, box };
  }, headerSel);
}

async function edgeGaps(page: Page, headerSel: string, leadSel: string, trailSel: string): Promise<
  { lead: number; trail: number } | null
> {
  return page.evaluate(({ headerSel, leadSel, trailSel }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    const cluster = header?.querySelector('.header-nav-cluster') as HTMLElement | null;
    const lead = header?.querySelector(leadSel) as HTMLElement | null;
    const trail = header?.querySelector(trailSel) as HTMLElement | null;
    if (!cluster || !lead || !trail) return null;
    const c = cluster.getBoundingClientRect();
    const l = lead.getBoundingClientRect();
    const t = trail.getBoundingClientRect();
    if (c.width === 0 || l.width === 0 || t.width === 0) return null;
    return { lead: c.left - l.right, trail: t.left - c.right };
  }, { headerSel, leadSel, trailSel });
}

async function setScale(page: Page, scale: number, rootPx: number): Promise<void> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      { timeout: 5_000, message: `the root never settled at ui-scale ${scale}` })
    .toBe(`${rootPx}px`);
}

test.describe('Mobile header titles are centered and clear of the leading icons', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('titles sit on the row middle and never overlap the leading cluster', async ({ page }) => {
    await navigateToApp(page);

    const brand = await measure(
      page, '.mobile-thread-header', '.header-nav-cluster', '.thread-toggle', '.hamburger-panel',
    );
    expect(brand, 'nav cluster / drawer toggle / hamburger not found').not.toBeNull();
    expect(
      Math.abs(brand!.center - brand!.rowCenter),
      `cluster center=${brand!.center} vs row center=${brand!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      brand!.left,
      `cluster left=${brand!.left} overlaps drawer toggle right=${brand!.leadingRight}`,
    ).toBeGreaterThanOrEqual(brand!.leadingRight - 0.5);
    expect(
      brand!.right,
      `cluster right=${brand!.right} overlaps hamburger left=${brand!.trailingLeft}`,
    ).toBeLessThanOrEqual(brand!.trailingLeft + 0.5);

    const markLabel = await page.evaluate(() => {
      const el = document.querySelector('.mobile-thread-header [data-role="brand-menu-toggle"]');
      return el?.getAttribute('aria-label') ?? null;
    });
    expect(markLabel, 'the mark has no accessible name').not.toBeNull();
    expect(markLabel!.toLowerCase()).toMatch(/connect/);

    await openThreadDrawer(page);
    const threads = await measure(
      page, '.mobile-threads-header', '.pane-header-title', '.filter-slot', TRAILING_LEFTMOST, 'Folders',
    );
    expect(threads, 'Threads title / filter slot not found').not.toBeNull();
    expect(
      Math.abs(threads!.center - threads!.rowCenter),
      `Threads center=${threads!.center} vs row center=${threads!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      threads!.left,
      `Threads title left=${threads!.left} overlaps filter slot right=${threads!.leadingRight}`,
    ).toBeGreaterThanOrEqual(threads!.leadingRight - 0.5);
    expect(
      threads!.right,
      `Threads title right=${threads!.right} overlaps trailing cluster left=${threads!.trailingLeft}`,
    ).toBeLessThanOrEqual(threads!.trailingLeft + 0.5);
    expect(
      threads!.clientWidth,
      `Threads title truncated (clientWidth=${threads!.clientWidth} < scrollWidth=${threads!.scrollWidth})`,
    ).toBeGreaterThanOrEqual(threads!.scrollWidth);

    expect(
      threads!.maxWidth,
      'the Threads title reserve was dropped, so nothing bounds the title',
    ).not.toBe('none');
  });

  async function chevrons(
    page: Page, headerSel: string, backSel: string, forwardSel: string,
  ): Promise<{ backLeft: number; forwardRight: number; backTop: number } | null> {
    return page.evaluate(({ headerSel, backSel, forwardSel }) => {
      const header = document.querySelector(headerSel) as HTMLElement | null;
      const back = header?.querySelector(backSel) as HTMLElement | null;
      const forward = header?.querySelector(forwardSel) as HTMLElement | null;
      if (!back || !forward) return null;
      const b = back.getBoundingClientRect();
      const f = forward.getBoundingClientRect();
      if (b.width === 0 || f.width === 0) return null;
      return { backLeft: b.left, forwardRight: f.right, backTop: b.top };
    }, { headerSel, backSel, forwardSel });
  }

  async function threadsMarkRight(page: Page): Promise<number | null> {
    return page.evaluate(() => {
      const el = document.querySelector('.mobile-threads-header .header-nav-cluster .brand-mark-slot');
      const rect = el?.getBoundingClientRect();
      return rect && rect.width > 0 ? rect.right : null;
    });
  }

  test('nav edges agree until content must shrink below the brand floor', async ({ page }) => {
    const preferencesSettled = watchPreferenceReads(page);
    await navigateToApp(page);
    await preferencesSettled();

    const threadArgs = [
      '.mobile-thread-header', 'button[aria-label="Previous thread"]', 'button[aria-label="Next thread"]',
    ] as const;
    const contentArgs = ['.mobile-content-header', '.content-back-btn', '.content-forward-btn'] as const;

    for (const [scale, rootPx, expected] of [[100, 16, 'reserve'], [125, 20, 'reserve'], [200, 32, 'floor']] as const) {
      await ensureMobileView(page, 'thread');
      await setScale(page, scale, rootPx);
      await expect
        .poll(() => chevrons(page, ...threadArgs), { timeout: 10_000, message: `thread chevrons never laid out at ${scale}%` })
        .not.toBeNull();
      const thread = (await chevrons(page, ...threadArgs))!;
      const geometry = (await navGeometry(page, '.mobile-thread-header'))!;
      expect(geometry.arm, `ui-scale ${scale} was meant to sit on the ${expected} arm`).toBe(expected);

      if (geometry.arm === 'reserve') {
        const gaps = (await edgeGaps(page, '.mobile-thread-header', '.thread-toggle', '.hamburger-panel'))!;
        const slot = geometry.reserve - geometry.box;
        for (const [side, gap] of [['leading', gaps.lead], ['trailing', gaps.trail]] as const) {
          expect(gap, `ui-scale ${scale} ${side} gap ${gap.toFixed(1)} is not the held slot ${slot.toFixed(1)}`)
            .toBeCloseTo(slot, 0);
          expect(gap, `ui-scale ${scale} ${side} slot is under one ${geometry.box.toFixed(1)}px button`)
            .toBeGreaterThanOrEqual(geometry.box);
        }
      }

      await ensureMobileView(page, 'content');
      await setScale(page, scale, rootPx);
      await expect
        .poll(() => chevrons(page, ...contentArgs), { timeout: 10_000, message: `content chevrons never laid out at ${scale}%` })
        .not.toBeNull();
      const content = (await chevrons(page, ...contentArgs))!;

      if (expected === 'floor') {
        expect(content.backLeft).toBeGreaterThan(thread.backLeft);
        expect(content.forwardRight).toBeLessThan(thread.forwardRight);
      } else {
        expect(
          Math.abs(thread.backLeft - content.backLeft),
          `ui-scale ${scale} back chevron: thread ${thread.backLeft.toFixed(1)}, content ${content.backLeft.toFixed(1)}`,
        ).toBeLessThan(1);
        expect(
          Math.abs(thread.forwardRight - content.forwardRight),
          `ui-scale ${scale} forward chevron: thread ${thread.forwardRight.toFixed(1)}, content ${content.forwardRight.toFixed(1)}`,
        ).toBeLessThan(1);
      }

      await openThreadDrawer(page);
      await setScale(page, scale, rootPx);
      await expect
        .poll(() => threadsMarkRight(page), { timeout: 10_000, message: `the threads mark never laid out at ${scale}%` })
        .not.toBeNull();
      const markRight = (await threadsMarkRight(page))!;
      expect(
        Math.abs(markRight - thread.forwardRight),
        `ui-scale ${scale} mark at ${markRight.toFixed(1)}, forward chevron at ${thread.forwardRight.toFixed(1)}`,
      ).toBeLessThan(1);
    }
  });

  test('the chevrons sit on the same LINE on the thread and content panes', async ({ page }) => {
    const preferencesSettled = watchPreferenceReads(page);
    await navigateToApp(page);
    await preferencesSettled();

    await page.evaluate(() => {
      document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
    });

    await ensureMobileView(page, 'thread');
    const threadArgs = [
      '.mobile-thread-header', 'button[aria-label="Previous thread"]', 'button[aria-label="Next thread"]',
    ] as const;
    await expect
      .poll(() => chevrons(page, ...threadArgs), { timeout: 10_000, message: 'thread pane chevrons never laid out' })
      .not.toBeNull();
    const thread = (await chevrons(page, ...threadArgs))!;

    await ensureMobileView(page, 'content');
    const contentArgs = ['.mobile-content-header', '.content-back-btn', '.content-forward-btn'] as const;
    await expect
      .poll(() => chevrons(page, ...contentArgs), { timeout: 10_000, message: 'content pane chevrons never laid out' })
      .not.toBeNull();
    const content = (await chevrons(page, ...contentArgs))!;

    expect(
      await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      'the root left the scaled size, so the measurements above guard nothing',
    ).toBe('18px');

    expect(
      Math.abs(thread.backTop - content.backTop),
      `chevron top: thread pane at ${thread.backTop.toFixed(3)}, content pane at ${content.backTop.toFixed(3)}`,
    ).toBeLessThan(0.02);
  });

  test('all three header rows are the same height', async ({ page }) => {
    await navigateToApp(page);

    const rowHeight = async (headerSel: string): Promise<number> => page.evaluate((sel) => {
      const row = document.querySelector(`${sel} .mobile-header-row`) as HTMLElement | null;
      return row ? row.getBoundingClientRect().height : 0;
    }, headerSel);

    await ensureMobileView(page, 'thread');
    const thread = await rowHeight('.mobile-thread-header');
    await ensureMobileView(page, 'content');
    const content = await rowHeight('.mobile-content-header');
    await openThreadDrawer(page);
    const threads = await rowHeight('.mobile-threads-header');

    expect(thread, 'thread row has no height').toBeGreaterThan(0);
    const heights = { thread, content, threads };
    expect(Math.abs(thread - content), `rows differ: ${JSON.stringify(heights)}`).toBeLessThan(0.5);
    expect(Math.abs(thread - threads), `rows differ: ${JSON.stringify(heights)}`).toBeLessThan(0.5);

    const markHeight = await page.evaluate(() => {
      const el = document.querySelector('.mobile-threads-header [data-role="brand-menu-toggle"]') as HTMLElement | null;
      return el ? el.getBoundingClientRect().height : 0;
    });
    expect(markHeight, 'the drawer mark was not measurable').toBeGreaterThan(0);
    expect(threads, `row ${threads} is shorter than the mark ${markHeight} it holds`)
      .toBeGreaterThanOrEqual(markHeight - 0.5);
  });

  test('connected is the mark at full light, and nothing else', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');

    const mark = '.mobile-thread-header [data-role="brand-menu-toggle"]';
    await expect
      .poll(() => page.evaluate((sel) => document.querySelector(sel)?.getAttribute('data-conn') ?? null, mark),
        { timeout: 15_000, message: 'the mark never reported a connected engine' })
      .toBe('connected');

    const readPaint = () => page.evaluate((sel) => {
      const tile = document.querySelector(`${sel} .brand-mark-glyph`) as HTMLElement | null;
      if (!tile) return null;
      const own = getComputedStyle(tile);
      const ring = getComputedStyle(tile, '::after');
      const swatch = (token: string): string => {
        const probe = document.createElement('span');
        probe.style.color = `var(${token})`;
        tile.appendChild(probe);
        const resolved = getComputedStyle(probe).color;
        probe.remove();
        return resolved;
      };
      return {
        color: own.color,
        light: swatch('--header-fg'),
        muted: swatch('--header-fg-muted'),
        opacity: own.opacity,
        animationName: own.animationName,
        ringContent: ring.content,
      };
    }, mark);

    const first = await readPaint();
    expect(first, 'the mark tile was not found').not.toBeNull();

    await expect
      .poll(async () => (await readPaint())?.color ?? null,
        { timeout: 5_000, message: 'the mark never settled on the header foreground at full light' })
      .toBe(first!.light);

    const paint = await readPaint();
    expect(paint, 'the mark tile was not found').not.toBeNull();
    expect(paint!.color, 'connected must not sit at the muted base the receded states use')
      .not.toBe(paint!.muted);
    expect(paint!.opacity, 'connected must not be dimmed').toBe('1');
    expect(paint!.animationName, 'connected must not animate').toBe('none');
    expect(paint!.ringContent, 'connected must carry no ring').toBe('none');
  });
});

test.describe('the reserve clears the widest trailing cluster by less than a button', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  const APP_ID = 'e2e-nav-reserve-clearance';
  let fixture: { cleanup: () => void };

  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Half Marathon', description: 'e2e fixture' },
      html: '<!DOCTYPE html><html><head><meta charset="UTF-8"><title>x</title></head>'
        + '<body><div id="ready">ready</div></body></html>',
      js: '',
    });
  });

  test.afterAll(() => fixture.cleanup());

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the forward chevron clears the overflow trigger, and only just', async ({ page }) => {
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    const preferencesSettled = watchPreferenceReads(page);
    await gotoWithRetry(page, '/');
    await preferencesSettled();
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .content-header-more')).toBeVisible({ timeout: 10_000 });

    await setScale(page, 125, 20);
    const geometry = (await navGeometry(page, '.mobile-content-header'))!;
    expect(geometry.arm, 'the reserve is what places the chevrons here').toBe('reserve');

    const gaps = (await edgeGaps(
      page, '.mobile-content-header', '.hamburger-panel', '.content-header-actions',
    ))!;
    const trailingBoxes = await page.evaluate(() => document.querySelectorAll(
      '.mobile-content-header .content-header-actions .icon-btn.header-icon',
    ).length);

    expect(trailingBoxes, 'the trailing cluster is meant to be the trigger plus the bell').toBe(2);

    const box = geometry.box;
    expect(
      gaps.trail,
      `the trigger overlaps the forward chevron by ${(-gaps.trail).toFixed(1)}px`,
    ).toBeGreaterThanOrEqual(-0.5);
    expect(
      gaps.trail,
      `a whole button of clearance (${gaps.trail.toFixed(1)}px of a ${box.toFixed(1)}px box) is a slot too far in`,
    ).toBeLessThan(box);

    expect(
      gaps.lead,
      `the leading gap ${gaps.lead.toFixed(1)}px is under one ${box.toFixed(1)}px box`,
    ).toBeGreaterThanOrEqual(box);
    expect(
      gaps.lead,
      `the leading gap ${gaps.lead.toFixed(1)}px is two boxes or more`,
    ).toBeLessThan(2 * box);
  });
});
